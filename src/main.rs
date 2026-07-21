#![deny(warnings, clippy::all)]

mod completions;
mod config;
mod extension;
mod init;
mod migrate;
mod primitives;
mod remote;
mod session;
mod state;
mod switcher;
mod tmux;
mod tool;
mod ui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::Config;
use session::{
    cmd_archive, cmd_migrate_layout, cmd_open, cmd_open_dispatch, cmd_recycle, cmd_reorient,
    cmd_shard, parse_session,
};
use tmux::Tmux;

#[derive(Parser)]
#[command(
    name = "muxr",
    version,
    about = "Tmux session manager for AI coding workflows"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Override the default tool (e.g., --tool opencode)
    #[arg(long)]
    tool: Option<String>,

    /// Tmux server name for socket isolation (env: MUXR_TMUX_SERVER)
    #[arg(long, env = "MUXR_TMUX_SERVER")]
    server: Option<String>,

    /// Open a FRESH conversation that rehydrates from the campaign pointer
    /// (campaign.md + log.md) instead of resuming the last conversation.
    /// The recycle model: prefer this over compacting a long session.
    #[arg(long)]
    fresh: bool,

    /// Repo name (e.g., work, personal, oss)
    #[arg(num_args = 0..)]
    args: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a default config file
    Init,
    /// List active tmux sessions
    Ls {
        /// Show only sessions with a running tool (claude) process. Hides
        /// panes sitting at a shell prompt with no tool attached.
        #[arg(long)]
        active: bool,
    },
    /// Snapshot sessions before reboot
    Save,
    /// Restore sessions after reboot
    Restore,
    /// Generate tmux status-left config from configured repos
    #[command(name = "tmux-status")]
    TmuxStatus,
    /// Print the merged config as JSON for extensions (statusline, glyph): each
    /// repo's color + open `ext` namespace. Reflects discovered fragments, so a
    /// preference (chrome, glyph) lives in config, never compiled into muxr.
    /// `config migrate` rewrites the current repo's fragment to the v4 schema.
    #[command(name = "config")]
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Rename the current session
    Rename {
        /// New name for the current session
        name: String,
    },
    /// Kill a session
    Kill {
        /// Session name (e.g., work/default) or "all"
        name: String,
    },
    /// Retire a session: gracefully /exit the tool, kill the tmux
    /// session. Drops the session from the saved state so future
    /// `muxr restore` won't recreate it.
    Retire {
        /// Session name (e.g. work/old-experiment) or "all" to retire every
        /// tmux session.
        name: String,
    },
    /// Move running sessions onto a newly installed tool binary, in place.
    ///
    /// Use this after upgrading Claude Code (or any tool) to migrate your
    /// long-running sessions onto the new version WITHOUT losing their
    /// conversation, HARNESS.md rules, or working dirs. For each target: graceful
    /// `/exit`, then relaunch with the full composed command (HARNESS prompt +
    /// campaign --add-dir paths + --resume) on the binary the tool now
    /// resolves to. Aliased as `migrate`.
    #[command(visible_alias = "migrate")]
    Upgrade {
        /// Session name to upgrade (e.g. work/retrieval-precision). Omit to
        /// upgrade every session running the selected tool.
        name: Option<String>,
        /// Tool to upgrade (default: claude).
        #[arg(long, default_value = "claude")]
        tool: String,
        /// Also switch model on relaunch (passes --model).
        #[arg(long)]
        model: Option<String>,
        /// Print what would be upgraded without touching any session.
        #[arg(long)]
        dry_run: bool,
        /// Skip the interactive confirmation and upgrade every matched session
        /// (scripting/CI). Without it, muxr lists the sessions and asks first.
        #[arg(long)]
        force: bool,
    },
    /// Interactive session switcher (TUI)
    Switch,
    /// Broadcast a slash command to every tool session.
    ///
    /// Default command is `/reload` -- useful when you've shipped an
    /// extension change and want every running Pi session to pick it
    /// up without manually relaunching each one.
    ///
    /// Filters: by default applies to every active tool session.
    /// Pass --tool to limit to one runtime (e.g. --tool pi),
    /// --repo to limit to one repo (e.g. --repo work).
    /// Pass --dry-run to print the targets without sending keys.
    Broadcast {
        /// Slash command to send (with leading /). Defaults to "/reload".
        #[arg(default_value = "/reload")]
        cmd: String,
        /// Limit to sessions running this tool (e.g. "pi", "claude").
        #[arg(long)]
        tool: Option<String>,
        /// Limit to sessions in this repo (e.g. "work", "personal").
        #[arg(long)]
        repo: Option<String>,
        /// List targets without sending the command.
        #[arg(long)]
        dry_run: bool,
    },
    /// Generate shell completions (zsh, bash, fish)
    Completions {
        /// Shell to generate completions for
        shell: String,
    },
    /// Emit the muxr skill file: the two-level `<repo>/<campaign>` launch
    /// grammar, the chooser, sharding, and the lifecycle verbs. Install it as
    /// a project skill so an agent learns how to drive muxr. Compiled in, so
    /// it always matches this binary's surface.
    Skill,
    /// Shard the current campaign into a new sibling campaign.
    ///
    /// Run from inside a campaign session, `muxr shard <new>` spins a topic
    /// that crystallized in the current campaign out into its own sibling,
    /// inheriting the hub's category and recording `sharded_from` lineage so
    /// the chooser groups the shard under its hub. Then launches the new
    /// campaign. Out of session, pass `--repo` and `--from` to name the hub
    /// explicitly.
    Shard {
        /// Slug for the new shard campaign (kebab-case).
        campaign: String,
        /// Repo to shard within (default: inferred from the current session).
        #[arg(long)]
        repo: Option<String>,
        /// Hub campaign to shard from (default: the current session's campaign).
        #[arg(long)]
        from: Option<String>,
    },
    /// Archive a campaign: move it to campaigns/archive/ so it drops out of
    /// the chooser while staying on disk (reversible). Prunes the launcher
    /// sprawl without deleting anything. Refuses a campaign with a live
    /// session. No --repo infers the repo from the current session.
    Archive {
        /// Campaign slug to archive.
        campaign: String,
        /// Repo it lives in (default: inferred from the current session).
        #[arg(long)]
        repo: Option<String>,
    },
    /// Re-anchor a live session to its current on-disk state.
    ///
    /// Reads the session's campaign + log paths and injects a one-line nudge
    /// into the pane telling the agent to re-read them NOW before continuing.
    /// The explicit, on-demand companion to the standing re-read pointer baked
    /// into the system prompt: run it right after a `/compact` to re-orient
    /// from the current files in seconds instead of a lossy conversation
    /// summary. No NAME uses the current session.
    Reorient {
        /// Session to reorient (e.g. storr/deploy). Omit to use the current one.
        name: Option<String>,
    },
    /// Recycle a session: exit, then reopen it FRESH from the pointer.
    ///
    /// The deliberate alternative to compacting a long session, and the primary
    /// token-burn lever. muxr gracefully exits the runtime and reopens a FRESH
    /// conversation that rehydrates from campaign.md + log.md -- no accumulated
    /// compaction drift. The FLUSH is the agent's job (the `/recycle` skill), not
    /// muxr's: muxr cannot observe "flush done" from outside the runtime, so it
    /// does not try (ADR 0008). Flush BEFORE recycling. The previous conversation
    /// stays on disk, recoverable via --resume. No NAME uses the current session.
    Recycle {
        /// Session to recycle (e.g. storr/deploy). Omit to use the current one.
        name: Option<String>,
    },
    /// Migrate a repo's campaigns/ tree from the old 3-level layout
    /// (campaigns/<category>/sessions/<topic>.md) to the 2-level repo/campaign
    /// model (campaigns/<campaign>/{campaign.md,log.md}).
    ///
    /// Filesystem-only and reversible via git: it does NOT touch state.json or
    /// live sessions. A real run prints the session-name rewrites for the
    /// human-gated cutover (save -> migrate -> edit config -> restore).
    #[command(name = "migrate-layout")]
    MigrateLayout {
        /// Repo to migrate (config key). Omit when using --dir.
        repo: Option<String>,
        /// Operate directly on this repo dir, bypassing config -- good for
        /// dry-running on a copy.
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
        /// Show the plan without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Move sessions/archive/* into a top-level archive/ instead of dropping.
        #[arg(long)]
        keep_archives: bool,
    },

    /// Tool subcommands (dynamic, from config): `muxr <tool> upgrade|model`
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Rewrite the CURRENT repo's muxr.toml fragment to the v4 schema: rename
    /// deprecated keys (`companion` -> `viewer`, `harnesses` -> `repos`) and
    /// stamp `schema_version`. Dry-run by default (prints the diff); `--write`
    /// applies. Scoped to the current repo only -- commit the change per-repo.
    /// NOTE: a migrated fragment uses v4-only keys, so only `--write` once every
    /// machine is on muxr >= 4.0.0 (the cross-machine rollout gate).
    Migrate {
        /// Apply the changes (default: print the diff and change nothing).
        #[arg(long)]
        write: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let tmux = Tmux::new(cli.server);

    match cli.command {
        Some(Commands::Init) => init::init(),
        Some(Commands::Ls { active }) => cmd_ls(&tmux, active),
        Some(Commands::Save) => {
            // Refuse to save when no tmux server is running: `list_sessions`
            // reports an empty list for a dead server, so an accidental `save`
            // (e.g. post-reboot, before `restore`) would atomically overwrite
            // state.json with zero sessions and lose every resume id. `retire`'s
            // internal state refresh calls `SavedState::save` directly and is
            // unaffected -- its server is live (it just killed sessions on it).
            if !tmux.server_running() {
                anyhow::bail!(
                    "no tmux server running -- nothing to save (refusing to overwrite the saved \
                     state with an empty one). Did you mean `muxr restore`?"
                );
            }
            let config = Config::load()?;
            state::SavedState::save(&config, &tmux)
        }
        Some(Commands::Restore) => {
            let config = Config::load()?;
            state::SavedState::restore(&tmux, &config)
        }
        Some(Commands::TmuxStatus) => cmd_tmux_status(&tmux),
        Some(Commands::Config { action }) => match action {
            None => cmd_config(),
            Some(ConfigAction::Migrate { write }) => cmd_config_migrate(write),
        },
        Some(Commands::Upgrade {
            name,
            tool,
            model,
            dry_run,
            force,
        }) => {
            let config = Config::load()?;
            let harness = config
                .tool_for(&tool)
                .with_context(|| format!("Unknown tool: {tool}"))?;
            tool::upgrade(
                &tmux,
                &config,
                &tool,
                &harness,
                tool::UpgradeOpts {
                    model: model.as_deref(),
                    name_filter: name.as_deref(),
                    dry_run,
                    force,
                },
            )
        }
        Some(Commands::Switch) => cmd_switch(&tmux),
        Some(Commands::Broadcast {
            cmd,
            tool,
            repo,
            dry_run,
        }) => cmd_broadcast(&tmux, &cmd, tool.as_deref(), repo.as_deref(), dry_run),
        Some(Commands::Rename { name }) => cmd_rename(&tmux, &name, cli.tool.as_deref()),
        Some(Commands::Kill { name }) => cmd_kill(&tmux, &name),
        Some(Commands::Retire { name }) => cmd_retire(&tmux, &name),
        Some(Commands::Completions { shell }) => completions::generate(&shell),
        Some(Commands::Skill) => {
            print!("{}", include_str!("../resources/skill.md"));
            Ok(())
        }
        Some(Commands::Shard {
            campaign,
            repo,
            from,
        }) => cmd_shard(&tmux, &campaign, repo.as_deref(), from.as_deref()),
        Some(Commands::Reorient { name }) => cmd_reorient(&tmux, name.as_deref()),
        Some(Commands::Recycle { name }) => cmd_recycle(&tmux, name.as_deref()),
        Some(Commands::Archive { campaign, repo }) => {
            cmd_archive(&tmux, &campaign, repo.as_deref())
        }
        Some(Commands::MigrateLayout {
            repo,
            dir,
            dry_run,
            keep_archives,
        }) => cmd_migrate_layout(repo.as_deref(), dir.as_deref(), dry_run, keep_archives),
        Some(Commands::External(args)) => {
            let config = Config::load()?;
            cmd_tool_dispatch(&tmux, &config, &args)
        }
        None => {
            if cli.args.is_empty() {
                cmd_control_plane(&tmux)
            } else {
                // Check if first arg is a tool name (e.g. `muxr claude upgrade`)
                // before treating it as a repo to open.
                let first = &cli.args[0];
                let config = Config::load().ok();
                let is_tool = config
                    .as_ref()
                    .map(|c| c.tool_names().contains(&first.to_string()))
                    .unwrap_or(false);

                if is_tool {
                    let config = config.unwrap();
                    cmd_tool_dispatch(&tmux, &config, &cli.args)
                } else {
                    cmd_open_dispatch(&tmux, &cli.args, cli.tool.as_deref(), cli.fresh)
                }
            }
        }
    }
}

/// Start or attach to the muxr control plane shell.
fn cmd_control_plane(tmux: &Tmux) -> Result<()> {
    let session = "muxr";
    let home = dirs::home_dir().context("Could not determine home directory")?;

    if tmux.session_exists(session) {
        tmux.attach(session)?;
    } else {
        tmux.create_session(session, &home, "", &[], None)?;
        tmux.attach(session)?;
    }

    Ok(())
}

/// Rename the current tmux session and flow through to the tool.
fn cmd_rename(tmux: &Tmux, name: &str, tool_override: Option<&str>) -> Result<()> {
    let old_name = tmux.current_session().unwrap_or_default();
    rename_session_by_name(tmux, &old_name, name, tool_override)
}

/// Rename a specific tmux session by name and flow through to its tool.
/// Shared between the CLI `muxr rename` and the TUI switcher.
pub(crate) fn rename_session_by_name(
    tmux: &Tmux,
    old: &str,
    new: &str,
    tool_override: Option<&str>,
) -> Result<()> {
    if new.is_empty() {
        anyhow::bail!("New name cannot be empty");
    }
    if new == old {
        return Ok(());
    }
    if tmux.session_exists(new) {
        anyhow::bail!("Session '{new}' already exists");
    }

    let harness = old.split('/').next().unwrap_or("default");

    tmux.rename_session(Some(old), new)?;
    eprintln!("Renamed {old} -> {new}");

    // Move the session file on disk so muxr stays comprehensive about
    // harness state. Best-effort: if the file or harness dir can't be
    // resolved, the tmux rename still landed.
    if let Ok(config) = Config::load() {
        try_move_session_file(&config, old, new);

        // Flow rename through to the harness if configured
        let tool = config.resolve_tool(harness, tool_override);
        if let Some(harness) = config.tool_for(&tool)
            && let Some(cmd) = harness.build_rename_command(new)
        {
            // send_text so it honors `-L` (a raw tmux would hit the default
            // server under `--server`) and is fold-safe for a TUI agent.
            let _ = tmux.send_text(new, &cmd);
            eprintln!("Sent rename to {tool}");
        }
    }

    Ok(())
}

/// Move the campaign directory on disk to match a tmux rename.
///
/// A session is `<repo>/<campaign>`, backed by `campaigns/<campaign>/`.
/// Renaming `<repo>/<old>` to `<repo>/<new>` renames the directory
/// `campaigns/<old>/` -> `campaigns/<new>/`.
///
/// Both old and new names must follow `<repo>/<campaign>` and share the same
/// repo for the move to fire. Anything else (bare names, cross-repo renames,
/// missing source dir) is silently skipped -- this is a hint, not a
/// correctness requirement.
fn try_move_session_file(config: &Config, old: &str, new: &str) {
    let Some((old_repo, old_campaign)) = parse_session(old) else {
        return;
    };
    let Some((new_repo, new_campaign)) = parse_session(new) else {
        return;
    };
    if old_repo != new_repo {
        // We don't move directories across repos from a rename.
        return;
    }
    let dir = match config.resolve_dir(&old_repo) {
        Ok(p) => p,
        Err(_) => return,
    };
    let old_path = config.layout.campaign_dir(&dir, &old_campaign);
    let new_path = config.layout.campaign_dir(&dir, &new_campaign);
    if !old_path.exists() {
        return;
    }
    if new_path.exists() {
        eprintln!(
            "Campaign dir at {} already exists; not overwriting",
            new_path.display()
        );
        return;
    }
    match std::fs::rename(&old_path, &new_path) {
        Ok(()) => eprintln!(
            "Moved campaign dir: {} -> {}",
            old_path.display(),
            new_path.display()
        ),
        Err(e) => eprintln!(
            "Could not move campaign dir {} -> {}: {e}",
            old_path.display(),
            new_path.display()
        ),
    }
}

/// Kill a session or all sessions.
fn cmd_kill(tmux: &Tmux, name: &str) -> Result<()> {
    let kill_one = |sname: &str| {
        tmux.kill_session(sname).ok();
        eprintln!("Killed {sname}");
    };

    if name == "all" {
        let sessions = tmux.list_sessions()?;
        for (sname, _) in &sessions {
            kill_one(sname);
        }
    } else if tmux.session_exists(name) {
        kill_one(name);
    } else {
        eprintln!("Session not found: {name}");
    }
    Ok(())
}

/// Retire a session cleanly:
/// 1. If a tool is running in the pane, send `/exit` and wait for the
///    process to terminate (up to 10s, then SIGKILL).
/// 2. Kill the tmux session.
/// 3. Drop the session from `state.json` so `muxr restore` won't resurrect it.
///
/// This is the counterpart to `new`: retire deletes everything new creates.
/// Broadcast a slash command to every tool session.
///
/// Use case: ship an extension change in pi-stack, then
/// `muxr broadcast` (defaults to /reload) to make every running Pi
/// session pick it up without manually relaunching each one.
///
/// Targets every tmux session whose first segment is a configured
/// repo AND that has the tool binary running in the pane.
/// Skips the muxr control plane. Filters: --tool, --repo.
fn cmd_broadcast(
    tmux: &Tmux,
    cmd: &str,
    tool_filter: Option<&str>,
    repo_filter: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let config = Config::load()?;

    if !cmd.starts_with('/') {
        anyhow::bail!(
            "Broadcast command must start with '/'. Got: {cmd:?}\n\
             Example: muxr broadcast /reload"
        );
    }

    let sessions = tmux.list_sessions()?;
    let mut targets: Vec<(String, String)> = Vec::new(); // (session_name, tool)

    for (sname, _) in &sessions {
        if sname == "muxr" {
            continue;
        }
        let repo_name = sname.split('/').next().unwrap_or(sname);
        if let Some(want) = repo_filter
            && want != repo_name
        {
            continue;
        }
        let tool = config.resolve_tool(repo_name, None);
        if let Some(want) = tool_filter
            && want != tool
        {
            continue;
        }
        let Some(harness) = config.tool_for(&tool) else {
            continue;
        };
        if !state::has_harness_process(tmux, sname, &harness.bin) {
            continue;
        }
        targets.push((sname.clone(), tool.clone()));
    }

    if targets.is_empty() {
        eprintln!("No matching harness sessions found.");
        return Ok(());
    }

    eprintln!("Broadcasting {cmd:?} to {} session(s):", targets.len());
    for (sname, tool) in &targets {
        eprintln!("  {sname} [{tool}]");
    }

    if dry_run {
        eprintln!("(dry-run; not sending keys)");
        return Ok(());
    }

    let mut errors = 0;
    for (sname, _) in &targets {
        // send_text (not a raw `tmux`) so it honors `-L` -- a raw tmux under
        // `--server` would send to the DEFAULT server, i.e. a production session
        // -- and is fold-safe (settle + separate Enter) for a TUI agent.
        if tmux.send_text(sname, cmd).is_err() {
            eprintln!("  send failed: {sname}");
            errors += 1;
        }
    }

    if errors > 0 {
        anyhow::bail!("{errors} session(s) failed to receive the broadcast");
    }
    eprintln!("Done.");
    Ok(())
}

fn cmd_retire(tmux: &Tmux, name: &str) -> Result<()> {
    let config = Config::load().ok();

    let retire_one = |sname: &str| {
        // 1. Graceful harness exit if something is running in the pane.
        if let Some(ref cfg) = config {
            let harness = sname.split('/').next().unwrap_or(sname);
            let tool = cfg.resolve_tool(harness, None);
            if let Some(harness) = cfg.tool_for(&tool)
                && state::has_harness_process(tmux, sname, &harness.bin)
            {
                // Graceful exit via the fold-safe `send_text` (settle + separate
                // Enter, and it honors `-L` -- a raw `tmux` would hit the default
                // server under `--server`, i.e. a production session). Use the
                // tool's own exit command (Pi's `/quit`), not a hardcoded `/exit`.
                let exit_cmd = harness
                    .exit_command
                    .clone()
                    .unwrap_or_else(|| "/exit".to_string());
                let _ = tmux.send_text(sname, &exit_cmd);

                // Wait briefly for the harness process to exit before we kill
                // the tmux session out from under it. Claude persists state
                // continuously, so a few seconds is plenty; not waiting risks
                // losing the last tool-call worth of working memory.
                let shell_pid = tmux.pane_pid(sname).ok().flatten();
                let harness_pid = shell_pid.and_then(|sp| {
                    state::descendant_pids(sp)
                        .into_iter()
                        .find(|pid| state::pid_runs_bin(*pid, &harness.bin))
                });
                if let Some(pid) = harness_pid {
                    wait_for_pid_exit(pid, 10);
                }
            }
        }

        // 2. Kill the tmux session.
        tmux.kill_session(sname).ok();

        eprintln!("Retired {sname}");
    };

    if name == "all" {
        let sessions = tmux.list_sessions()?;
        for (sname, _) in &sessions {
            retire_one(sname);
        }
    } else if tmux.session_exists(name) {
        retire_one(name);
    } else {
        eprintln!("Session not found: {name}");
        return Ok(());
    }

    // Refresh state.json from post-retire tmux state. Retired sessions no
    // longer exist in tmux, so `save` naturally excludes them -- no manual
    // list-editing required.
    if let Some(ref cfg) = config
        && let Err(e) = state::SavedState::save(cfg, tmux)
    {
        eprintln!("  state.json refresh: {e}");
    }

    Ok(())
}

/// Wait up to `timeout_secs` for a PID to exit. Escalates to SIGKILL if
/// the process is still alive when the timeout elapses. Stderr from
/// `kill -0` polls is suppressed -- when the pid is gone the helper prints
/// "No such process" which is not an error condition here.
fn wait_for_pid_exit(pid: u32, timeout_secs: u32) {
    use std::process::Stdio;
    for _ in 0..timeout_secs * 10 {
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !alive {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    eprintln!("  process {pid} did not exit, sending SIGKILL");
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .stderr(Stdio::null())
        .status();
}

fn cmd_ls(tmux: &Tmux, active_only: bool) -> Result<()> {
    let config = Config::load().ok();
    let sessions = tmux.list_sessions()?;

    // Pre-resolve harness for each harness so we can detect running harness
    // processes when --active is requested. Done once outside the loop since
    // Config::tool_for is a map lookup but feels cheaper to cache.
    let tool_for = |harness: &str| -> Option<config::Tool> {
        let cfg = config.as_ref()?;
        let tool = cfg.resolve_tool(harness, None);
        cfg.tool_for(&tool)
    };

    let mut shown = 0;
    for (name, path) in &sessions {
        let harness = name.split('/').next().unwrap_or(name);

        if active_only {
            // Skip muxr control-plane sessions and any session without a
            // running harness process. The detection mirrors
            // tool::upgrade's check, so --active and `upgrade` target
            // the same set.
            if name == "muxr" {
                continue;
            }
            let Some(harness) = tool_for(harness) else {
                continue;
            };
            if !state::has_harness_process(tmux, name, &harness.bin) {
                continue;
            }
        }

        let is_remote = config
            .as_ref()
            .map(|c| c.is_remote(harness))
            .unwrap_or(false);

        if is_remote {
            println!("  {name}  (remote)");
        } else {
            println!("  {name}  ({path})");
        }
        shown += 1;
    }

    if shown == 0 {
        if active_only {
            eprintln!("No active harness sessions.");
        } else {
            eprintln!("No active tmux sessions.");
        }
    }
    Ok(())
}

/// Interactive TUI chooser: switch to a live session, open a dormant
/// campaign, or create a new one. When `[chooser].command` is set, delegate
/// selection to that external picker (e.g. sesh) instead of the built-in TUI.
fn cmd_switch(tmux: &Tmux) -> Result<()> {
    if let Ok(config) = Config::load()
        && let Some(cmd) = config.chooser.command.as_deref()
    {
        // Hand the terminal to the external picker; it owns listing + attach.
        let status = std::process::Command::new("sh")
            .args(["-c", cmd])
            .status()
            .with_context(|| format!("running chooser command: {cmd}"))?;
        if !status.success() {
            anyhow::bail!("chooser command `{cmd}` exited {status}");
        }
        return Ok(());
    }
    match switcher::run(tmux)? {
        switcher::Action::Switch(session) => tmux.attach(&session),
        switcher::Action::Open(repo, campaign) => {
            let config = Config::load()?;
            cmd_open(tmux, &config, &repo, &campaign, false)
        }
        switcher::Action::Archive(repo, campaign) => {
            if let Err(e) = cmd_archive(tmux, &campaign, Some(&repo)) {
                eprintln!("archive failed: {e}");
            }
            cmd_switch(tmux)
        }
        switcher::Action::Recycle(session) => {
            if let Err(e) = cmd_recycle(tmux, Some(&session)) {
                eprintln!("recycle failed: {e}");
            }
            cmd_switch(tmux)
        }
        switcher::Action::Kill(session) => {
            tmux.kill_session(&session)?;
            eprintln!("Killed {session}");
            // Re-enter the switcher after kill
            cmd_switch(tmux)
        }
        switcher::Action::Rename(old, new) => {
            if let Err(e) = rename_session_by_name(tmux, &old, &new, None) {
                eprintln!("rename failed: {e}");
            }
            cmd_switch(tmux)
        }
        switcher::Action::None => Ok(()),
    }
}

/// Dispatch tool subcommands: muxr claude upgrade --model X
fn cmd_tool_dispatch(tmux: &Tmux, config: &Config, args: &[String]) -> Result<()> {
    let tool_name = args.first().context("Missing tool name")?;

    let tool = config
        .tool_for(tool_name)
        .with_context(|| format!("Unknown tool: {tool_name}"))?;

    let sub = args.get(1).map(|s| s.as_str()).unwrap_or("status");

    match sub {
        "upgrade" => {
            let model = find_flag_value(&args[2..], "--model");
            let name = find_flag_value(&args[2..], "--name");
            let dry_run = args[2..].iter().any(|a| a == "--dry-run");
            let force = args[2..].iter().any(|a| a == "--force");
            tool::upgrade(
                tmux,
                config,
                tool_name,
                &tool,
                tool::UpgradeOpts {
                    model: model.as_deref(),
                    name_filter: name.as_deref(),
                    dry_run,
                    force,
                },
            )
        }
        "model" => {
            let model = args.get(2).map(|s| s.as_str());
            tool::model_switch(tmux, config, tool_name, &tool, model)
        }
        other => {
            anyhow::bail!("Unknown {tool_name} subcommand: {other}\nAvailable: model, upgrade")
        }
    }
}

/// Extract a flag value from args (e.g., --model opus-4-7 -> Some("opus-4-7")).
fn find_flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Format a seconds duration compactly, e.g. `12s`, `4m`, `2h3m`.
pub(crate) fn fmt_age(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// Generate tmux status-left format string from configured repos.
/// Used by tmux.conf: set -g status-left "#(muxr tmux-status)"
/// Print the merged config as JSON for extensions to consume (statusline
/// glyph/color, glyph builder): `{ "repos": { "<name>": { "color", "ext" } } }`.
/// `ext` is the repo's open namespace verbatim, so preference data (chrome) is
/// config muxr carries, never compiled-in. Reflects any discovered fragments.
fn cmd_config() -> Result<()> {
    let config = Config::load()?;
    let mut repos = serde_json::Map::new();
    for (name, repo) in &config.repos {
        let mut o = serde_json::Map::new();
        o.insert(
            "color".into(),
            serde_json::Value::String(repo.color.clone()),
        );
        o.insert("ext".into(), serde_json::to_value(&repo.ext)?);
        repos.insert(name.clone(), serde_json::Value::Object(o));
    }
    let out = serde_json::json!({ "repos": serde_json::Value::Object(repos) });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// Write `content` to `path` atomically: write a sibling `<path>.tmp` then rename
/// it over the target (rename within a dir is atomic on POSIX), so a crash or
/// power loss mid-write can never leave a truncated fragment. The rename also
/// replaces a symlink target with a real file rather than writing THROUGH the
/// symlink to an arbitrary destination.
pub(crate) fn write_atomic(path: &std::path::Path, content: &str) -> Result<()> {
    use std::io::Write;
    // Unique temp name (pid + nanos): not a predictable, pre-creatable path, so a
    // hostile repo cannot pre-seed `<file>.tmp` as a symlink to redirect the write
    // (`config migrate` runs inside a repo dir an attacker may control).
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(format!(".tmp.{}.{stamp}", std::process::id()));
    let tmp = std::path::PathBuf::from(tmp);
    // create_new = O_CREAT|O_EXCL: refuses an existing path, so a pre-planted
    // symlink at the temp name is rejected rather than written through.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .with_context(|| format!("creating temp file {}", tmp.display()))?;
    if let Err(e) = f
        .write_all(content.as_bytes())
        .and_then(|()| f.sync_all())
    {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::Error::new(e).context(format!("writing {}", tmp.display())));
    }
    drop(f);
    // Preserve the target's permissions: the temp was created with the default
    // `0666 & umask` (typically 0644), so without this a `chmod 600` fragment /
    // state.json would come back 0644 after the rename. Best-effort -- a mode we
    // can't read or set is non-fatal.
    #[cfg(unix)]
    if let Ok(meta) = std::fs::metadata(path) {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::Error::new(e).context(format!("replacing {}", path.display())));
    }
    Ok(())
}

/// The git top-level of the current directory, via `git rev-parse`. Used to
/// locate the current repo's `muxr.toml` fragment for `config migrate`.
fn git_toplevel() -> Result<std::path::PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("running git rev-parse --show-toplevel")?;
    if !out.status.success() {
        anyhow::bail!("not inside a git repository -- run `muxr config migrate` from a repo");
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(std::path::PathBuf::from(path))
}

/// `muxr config migrate`: rewrite the CURRENT repo's `muxr.toml` fragment to the
/// v4 schema (see `config::migrate_fragment`). Dry-run by default (prints the
/// per-line diff); `--write` applies. Scoped to the current repo's fragment only
/// -- the operator commits per-repo (no bulk multi-repo write).
fn cmd_config_migrate(write: bool) -> Result<()> {
    let fragment = git_toplevel()?.join("muxr.toml");
    if !fragment.exists() {
        anyhow::bail!(
            "no muxr.toml fragment at {} -- run this from a repo that carries one",
            fragment.display()
        );
    }
    let content = primitives::read_text(&fragment)?;
    let (new_content, changes) = config::migrate_fragment(&content);
    let where_ = ui::abbreviate_home(&fragment.display().to_string());
    if changes.is_empty() {
        ui::ok(&format!("{where_} is already current"));
        return Ok(());
    }
    if write {
        // Never write a migration that produced invalid TOML. The migrator is
        // line-oriented and can, in pathological multi-line-string shapes, break
        // structure; parse-validate first so a bad rewrite fails loud instead of
        // landing a corrupt fragment. (Dry-run -- the default -- always prints the
        // exact per-line diff, so any surprising rewrite is visible before this.)
        toml::from_str::<toml::Table>(&new_content).with_context(|| {
            format!(
                "migration of {where_} produced invalid TOML -- refusing to write \
                 (re-run without --write to inspect the diff)"
            )
        })?;
        write_atomic(&fragment, &new_content)?;
        ui::ok(&format!("migrated {where_} ({} change(s))", changes.len()));
    } else {
        ui::note(&format!(
            "dry run: {} change(s) for {where_} -- re-run with --write to apply",
            changes.len()
        ));
        for c in &changes {
            println!("{c}");
        }
    }
    Ok(())
}

fn cmd_tmux_status(tmux: &Tmux) -> Result<()> {
    let session_name = tmux.display_message("#{session_name}")?;

    let harness = session_name.split('/').next().unwrap_or(&session_name);

    let config = Config::load().ok();
    let color = config
        .as_ref()
        .map(|c| c.color_for(harness).to_string())
        .unwrap_or_else(|| "#8a7f83".to_string());

    // Output tmux format string
    print!("#[fg={color}]● #[fg=#E8DDD0]{session_name} #[fg=#3B3639]│ ");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::compose_launch_command;

    #[test]
    fn resolve_tool_uses_override() {
        let config: Config = toml::from_str("[repos]").unwrap();
        assert_eq!(config.resolve_tool("work", Some("opencode")), "opencode");
    }

    #[test]
    fn resolve_tool_falls_back_to_config() {
        let config: Config = toml::from_str("[repos]").unwrap();
        assert_eq!(config.resolve_tool("work", None), "claude");
    }

    #[test]
    fn parse_session_splits_two_part() {
        assert_eq!(
            parse_session("work/in-place-updates"),
            Some(("work".to_string(), "in-place-updates".to_string()))
        );
    }

    #[test]
    fn parse_session_handles_switchboard() {
        // The switchboard is just a campaign named `switchboard`; no special
        // collapse/inverse mapping is needed under the two-level model.
        assert_eq!(
            parse_session("work/switchboard"),
            Some(("work".to_string(), "switchboard".to_string()))
        );
    }

    #[test]
    fn parse_session_rejects_one_part_and_three_part() {
        assert_eq!(parse_session("solo"), None);
        // A third slash means the campaign component contains a slash, which
        // is never valid (campaigns are validated kebab-case).
        assert_eq!(parse_session("work/factory/in-place-updates"), None);
    }

    #[test]
    fn parse_session_rejects_empty_components() {
        assert_eq!(parse_session("/campaign"), None);
        assert_eq!(parse_session("repo/"), None);
    }

    #[test]
    fn compose_launch_command_carries_prompt_and_add_dirs() {
        // Regression guard for the convergence fix: a resumed launch (restore /
        // upgrade) must carry the composed system prompt AND the campaign's
        // --add-dir paths, not just --name/--resume. The pre-convergence
        // restore_command/launch_command paths dropped both. Layout is now
        // dir-per-campaign: campaigns/<campaign>/{campaign.md,log.md}.
        let dir = tempfile::tempdir().unwrap();
        let campaign_dir = dir.path().join("campaigns/factory");
        std::fs::create_dir_all(&campaign_dir).unwrap();
        let extra = dir.path().join("extra-workdir");
        std::fs::create_dir_all(&extra).unwrap();

        std::fs::write(
            campaign_dir.join("campaign.md"),
            format!(
                "---\ncategory: \"\"\nsynthesist_trees: []\npaths:\n  - {}\n---\n\n# factory\nbody\n",
                extra.display()
            ),
        )
        .unwrap();
        std::fs::write(
            campaign_dir.join("log.md"),
            "---\nentrypoint: \"\"\n---\n\n# factory\nlog body\n",
        )
        .unwrap();

        let toml = format!(
            "[repos.nomograph]\ndir = {dir:?}\ncolor = \"#fff\"\n",
            dir = dir.path()
        );
        let config: Config = toml::from_str(&toml).unwrap();

        let (cmd, session_dir) =
            compose_launch_command(&config, "nomograph/factory", Some("ABC123"), None, false)
                .unwrap();

        // Flag values are shell-quoted, so assert flag and value separately.
        assert!(cmd.contains("--name"));
        assert!(cmd.contains("nomograph/factory"), "name missing: {cmd}");
        assert!(cmd.contains("--resume"), "resume flag missing: {cmd}");
        assert!(cmd.contains("ABC123"), "resume id missing: {cmd}");
        assert!(
            cmd.contains("--append-system-prompt-file"),
            "system prompt dropped: {cmd}"
        );
        assert!(cmd.contains("--add-dir"), "add-dir dropped: {cmd}");
        assert!(
            cmd.contains(&extra.display().to_string()),
            "campaign path missing: {cmd}"
        );
        assert!(session_dir.join("campaigns/factory/campaign.md").is_file());
        assert!(session_dir.join("campaigns/factory/log.md").is_file());
    }

    #[test]
    fn resolver_extension_overrides_dir_and_adds_dirs() {
        // The 3.0 resolver contract: a configured [extensions].resolver is
        // invoked with the launch intent on stdin and returns layout facts on
        // stdout. Here it relocates `dir` to a second tree and contributes an
        // extra --add-dir; muxr must launch in the resolved dir (so the
        // campaign/log paths follow it) and carry the extra working dir.
        let real = tempfile::tempdir().unwrap();
        let campaign_dir = real.path().join("campaigns/factory");
        std::fs::create_dir_all(&campaign_dir).unwrap();
        let extra = real.path().join("resolver-extra");
        std::fs::create_dir_all(&extra).unwrap();
        std::fs::write(
            campaign_dir.join("campaign.md"),
            "---\ncategory: \"\"\nsynthesist_trees: []\npaths: []\n---\n\n# factory\nbody\n",
        )
        .unwrap();
        std::fs::write(
            campaign_dir.join("log.md"),
            "---\nentrypoint: \"\"\n---\n\n# factory\nlog body\n",
        )
        .unwrap();

        // The repo's configured dir is a DECOY: an empty tree with no campaign.
        // Only the resolver's override makes the launch succeed, proving it ran.
        let decoy = tempfile::tempdir().unwrap();
        // `cat >/dev/null` drains the intent JSON so the writer never sees a
        // broken pipe; then we emit the outcome on stdout.
        let resolver = format!(
            "cat >/dev/null; printf '%s' '{{\"dir\":{real:?},\"add_dirs\":[{extra:?}]}}'",
            real = real.path().to_string_lossy(),
            extra = extra.display().to_string(),
        );
        let toml = format!(
            "[extensions]\nresolver = {resolver:?}\n\n[repos.nomograph]\ndir = {decoy:?}\ncolor = \"#fff\"\n",
            decoy = decoy.path(),
        );
        let config: Config = toml::from_str(&toml).unwrap();

        let (cmd, session_dir) =
            compose_launch_command(&config, "nomograph/factory", None, None, false).unwrap();

        assert_eq!(
            session_dir,
            real.path(),
            "resolver dir override not applied: {session_dir:?}"
        );
        assert!(
            cmd.contains(&extra.display().to_string()),
            "resolver add_dir missing: {cmd}"
        );
        assert!(
            session_dir.join("campaigns/factory/campaign.md").is_file(),
            "campaign path did not follow the resolved dir"
        );
    }

    #[test]
    fn resolver_extension_failure_is_fatal() {
        // Fail closed: a configured resolver that errors must abort the launch,
        // not silently fall back to the default layout (which could attach to
        // the wrong campaign).
        let dir = tempfile::tempdir().unwrap();
        let toml = format!(
            "[extensions]\nresolver = \"exit 3\"\n\n[repos.nomograph]\ndir = {dir:?}\ncolor = \"#fff\"\n",
            dir = dir.path(),
        );
        let config: Config = toml::from_str(&toml).unwrap();
        let err = compose_launch_command(&config, "nomograph/factory", None, None, false)
            .expect_err("resolver failure should be fatal");
        assert!(
            err.to_string().contains("resolver extension"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn compose_launch_command_folds_plural_prompt_files_with_campaign() {
        // Regression guard: a repo configured with the PLURAL
        // append_system_prompt_files array (base + overlay) must still get the
        // campaign + log composed into the launch prompt. The bug: compose read
        // only the singular field and left the plural array set, so
        // launch_command_with_settings preferred the array and dropped the
        // composed campaign/log temp file entirely.
        let dir = tempfile::tempdir().unwrap();
        let campaign_dir = dir.path().join("campaigns/factory");
        std::fs::create_dir_all(&campaign_dir).unwrap();
        let base = dir.path().join("HARNESS-base.md");
        let overlay = dir.path().join("HARNESS.md");
        std::fs::write(&base, "BASE_HARNESS_MARKER").unwrap();
        std::fs::write(&overlay, "OVERLAY_HARNESS_MARKER").unwrap();
        std::fs::write(
            campaign_dir.join("campaign.md"),
            "---\ncategory: \"\"\nsynthesist_trees: []\npaths: []\n---\n\n# factory\nCAMPAIGN_BODY_MARKER\n",
        )
        .unwrap();
        std::fs::write(
            campaign_dir.join("log.md"),
            "---\nentrypoint: \"ENTRYPOINT_MARKER\"\n---\n\n# factory\nLOG_BODY_MARKER\n",
        )
        .unwrap();

        // Use a unique repo key so the deterministic temp-prompt path
        // (muxr-prompt-<repo>-<campaign>.md) can't collide with another
        // test's under parallel execution.
        let toml = format!(
            "[repos.pluralrepo]\ndir = {dir:?}\ncolor = \"#fff\"\n\
             [repos.pluralrepo.launch]\nappend_system_prompt_files = [{base:?}, {overlay:?}]\n",
            dir = dir.path(),
            base = base,
            overlay = overlay,
        );
        let config: Config = toml::from_str(&toml).unwrap();

        let (cmd, _) =
            compose_launch_command(&config, "pluralrepo/factory", Some("X"), None, false).unwrap();

        // Exactly one --append-system-prompt-file (the composed temp), and the
        // temp must carry BOTH HARNESS files, the campaign body, and the
        // entrypoint pointer -- but NOT the (growing) log body.
        assert_eq!(
            cmd.matches("--append-system-prompt-file").count(),
            1,
            "expected a single composed prompt file: {cmd}"
        );
        let tmp = std::env::temp_dir().join("muxr-prompt-pluralrepo-factory.md");
        let composed = std::fs::read_to_string(&tmp).unwrap();
        for marker in [
            "BASE_HARNESS_MARKER",
            "OVERLAY_HARNESS_MARKER",
            "CAMPAIGN_BODY_MARKER",
            "ENTRYPOINT_MARKER",
        ] {
            assert!(
                composed.contains(marker),
                "composed prompt missing {marker}:\n{composed}"
            );
        }
        assert!(
            !composed.contains("LOG_BODY_MARKER"),
            "log body must NOT be snapshotted into the prompt:\n{composed}"
        );
    }

    #[test]
    fn compose_launch_command_degrades_when_log_file_missing() {
        // Archived-but-running session: campaign.md (with paths) exists, but
        // log.md is gone. The relaunch must still carry the campaign --add-dir
        // paths and a prompt file -- NOT collapse to bare name+resume -- so an
        // upgrade doesn't strip a live session's harness rules.
        let dir = tempfile::tempdir().unwrap();
        let campaign_dir = dir.path().join("campaigns/factory");
        std::fs::create_dir_all(&campaign_dir).unwrap();
        let extra = dir.path().join("extra-workdir");
        std::fs::create_dir_all(&extra).unwrap();
        std::fs::write(
            campaign_dir.join("campaign.md"),
            format!(
                "---\ncategory: \"\"\nsynthesist_trees: []\npaths:\n  - {}\n---\n\n# factory\nbody\n",
                extra.display()
            ),
        )
        .unwrap();
        // Deliberately do NOT write log.md.

        let toml = format!(
            "[repos.nomograph]\ndir = {dir:?}\ncolor = \"#fff\"\n",
            dir = dir.path()
        );
        let config: Config = toml::from_str(&toml).unwrap();

        let (cmd, _) =
            compose_launch_command(&config, "nomograph/factory", Some("ZID9"), None, false)
                .expect("missing log file should degrade, not error");

        assert!(cmd.contains("--resume"), "resume flag missing: {cmd}");
        assert!(cmd.contains("ZID9"), "resume id missing: {cmd}");
        // Repo/campaign-level context survives even though the log body file
        // is gone:
        assert!(
            cmd.contains("--append-system-prompt-file"),
            "prompt dropped on missing log: {cmd}"
        );
        assert!(
            cmd.contains("--add-dir") && cmd.contains(&extra.display().to_string()),
            "campaign --add-dir dropped on missing log: {cmd}"
        );
    }

    #[test]
    fn compose_launch_command_fails_loud_on_unparseable_log() {
        // Issue #11: a PRESENT-but-unparseable log must FAIL the relaunch loud,
        // not degrade to name+resume (which silently strips a live session's
        // composed prompt + campaign --add-dir paths). Contrast the ABSENT case
        // above, which degrades cleanly.
        let dir = tempfile::tempdir().unwrap();
        let campaign_dir = dir.path().join("campaigns/factory");
        std::fs::create_dir_all(&campaign_dir).unwrap();
        std::fs::write(
            campaign_dir.join("campaign.md"),
            "---\ncategory: \"\"\nsynthesist_trees: []\npaths: []\n---\n\n# factory\nbody\n",
        )
        .unwrap();
        // A single unescaped inner `"` inside the double-quoted entrypoint scalar
        // makes the YAML unparseable -- the exact one-character typo from the
        // field report.
        std::fs::write(
            campaign_dir.join("log.md"),
            "---\nentrypoint: \"he said \"hi\" here\"\n---\n\nbody\n",
        )
        .unwrap();

        let toml = format!(
            "[repos.nomograph]\ndir = {dir:?}\ncolor = \"#fff\"\n",
            dir = dir.path()
        );
        let config: Config = toml::from_str(&toml).unwrap();

        let err = compose_launch_command(&config, "nomograph/factory", Some("ZID9"), None, false)
            .expect_err("unparseable log frontmatter must fail loud, not degrade");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("log file present but unparseable"),
            "error should state the fail-loud reason: {msg}"
        );
        assert!(msg.contains("log.md"), "error should name the file: {msg}");
    }

    #[test]
    fn compose_launch_command_fails_loud_on_unparseable_campaign() {
        // Issue #11, campaign side: a PRESENT-but-unparseable campaign.md must
        // also fail loud (surfaced before the log is even read).
        let dir = tempfile::tempdir().unwrap();
        let campaign_dir = dir.path().join("campaigns/factory");
        std::fs::create_dir_all(&campaign_dir).unwrap();
        // Unterminated double-quoted scalar -> YAML scanner error.
        std::fs::write(
            campaign_dir.join("campaign.md"),
            "---\ncategory: \"unterminated\nsynthesist_trees: []\npaths: []\n---\n\nbody\n",
        )
        .unwrap();

        let toml = format!(
            "[repos.nomograph]\ndir = {dir:?}\ncolor = \"#fff\"\n",
            dir = dir.path()
        );
        let config: Config = toml::from_str(&toml).unwrap();

        let err = compose_launch_command(&config, "nomograph/factory", Some("ZID9"), None, false)
            .expect_err("unparseable campaign frontmatter must fail loud, not degrade");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("campaign file present but unparseable"),
            "error should state the fail-loud reason: {msg}"
        );
    }

    #[test]
    fn compose_launch_command_continue_fallback_when_no_id() {
        // restore passes continue_fallback=true; with no discovered id the
        // command must re-attach via --continue rather than starting cold.
        let dir = tempfile::tempdir().unwrap();
        let campaign_dir = dir.path().join("campaigns/factory");
        std::fs::create_dir_all(&campaign_dir).unwrap();
        std::fs::write(
            campaign_dir.join("campaign.md"),
            "---\ncategory: \"\"\nsynthesist_trees: []\npaths: []\n---\n\n# factory\nbody\n",
        )
        .unwrap();
        std::fs::write(
            campaign_dir.join("log.md"),
            "---\nentrypoint: \"\"\n---\n\nbody\n",
        )
        .unwrap();

        let toml = format!(
            "[repos.nomograph]\ndir = {dir:?}\ncolor = \"#fff\"\n",
            dir = dir.path()
        );
        let config: Config = toml::from_str(&toml).unwrap();

        let (cmd, _) =
            compose_launch_command(&config, "nomograph/factory", None, None, true).unwrap();

        assert!(
            cmd.contains("--continue"),
            "continue fallback missing: {cmd}"
        );
        assert!(
            !cmd.contains("--resume"),
            "should not resume without id: {cmd}"
        );
    }

    #[test]
    fn try_move_session_file_moves_campaign_dir_when_source_exists() {
        let dir = tempfile::tempdir().unwrap();
        let old_dir = dir.path().join("campaigns/old-campaign");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("log.md"), "log body").unwrap();

        let toml = format!(
            "[repos.work]\ndir = {dir:?}\ncolor = \"#fff\"\n",
            dir = dir.path()
        );
        let config: Config = toml::from_str(&toml).unwrap();

        try_move_session_file(&config, "work/old-campaign", "work/new-campaign");

        assert!(!old_dir.exists(), "old dir should be gone");
        let new_dir = dir.path().join("campaigns/new-campaign");
        assert!(new_dir.exists(), "new dir should exist");
        assert_eq!(
            std::fs::read_to_string(new_dir.join("log.md")).unwrap(),
            "log body"
        );
    }

    #[test]
    fn try_move_session_file_silent_when_source_missing() {
        let dir = tempfile::tempdir().unwrap();
        let toml = format!(
            "[repos.work]\ndir = {dir:?}\ncolor = \"#fff\"\n",
            dir = dir.path()
        );
        let config: Config = toml::from_str(&toml).unwrap();
        // No campaign dir at the expected location. Should not panic, should
        // not create anything.
        try_move_session_file(&config, "work/old-campaign", "work/new-campaign");
        let campaigns = dir.path().join("campaigns");
        assert!(!campaigns.exists() || std::fs::read_dir(&campaigns).unwrap().next().is_none());
    }

    #[test]
    fn try_move_session_file_refuses_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let old_dir = dir.path().join("campaigns/old-campaign");
        let new_dir = dir.path().join("campaigns/new-campaign");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::create_dir_all(&new_dir).unwrap();
        std::fs::write(old_dir.join("log.md"), "old").unwrap();
        std::fs::write(new_dir.join("log.md"), "existing").unwrap();

        let toml = format!(
            "[repos.work]\ndir = {dir:?}\ncolor = \"#fff\"\n",
            dir = dir.path()
        );
        let config: Config = toml::from_str(&toml).unwrap();

        try_move_session_file(&config, "work/old-campaign", "work/new-campaign");

        assert!(old_dir.exists(), "old must not have been clobbered");
        assert_eq!(
            std::fs::read_to_string(new_dir.join("log.md")).unwrap(),
            "existing"
        );
    }

    #[test]
    fn try_move_session_file_skips_cross_repo() {
        let dir = tempfile::tempdir().unwrap();
        let old_dir = dir.path().join("campaigns/old-campaign");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("log.md"), "x").unwrap();
        let toml = format!(
            "[repos.work]\ndir = {dir:?}\ncolor = \"#fff\"\n",
            dir = dir.path()
        );
        let config: Config = toml::from_str(&toml).unwrap();

        try_move_session_file(&config, "work/old-campaign", "different/old-campaign");

        // Cross-repo move is not supported; old dir stays.
        assert!(old_dir.exists());
    }
}
