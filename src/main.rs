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

    /// Harness name (e.g., work, personal, oss)
    #[arg(num_args = 0..)]
    args: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a default config file
    Init,
    /// List active tmux sessions
    Ls {
        /// Show only sessions with a running harness (claude) process. Hides
        /// panes sitting at a shell prompt with no harness attached.
        #[arg(long)]
        active: bool,
    },
    /// Snapshot sessions before reboot
    Save,
    /// Restore sessions after reboot
    Restore,
    /// Generate tmux status-left config from harnesses
    #[command(name = "tmux-status")]
    TmuxStatus,
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
    /// Retire a session: gracefully /exit the harness, kill the tmux
    /// session. Drops the session from the saved state so future
    /// `muxr restore` won't recreate it.
    Retire {
        /// Session name (e.g. work/old-experiment) or "all" to retire every
        /// tmux session.
        name: String,
    },
    /// Move running sessions onto a newly installed harness binary, in place.
    ///
    /// Use this after upgrading Claude Code (or any harness) to migrate your
    /// long-running sessions onto the new version WITHOUT losing their
    /// conversation, harness rules, or working dirs. For each target: graceful
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
        /// Bypass readiness gate and upgrade unconditionally (today's behavior).
        #[arg(long)]
        force: bool,
        /// Poll readiness for up to this many seconds before skipping.
        #[arg(long)]
        wait: Option<u64>,
        /// Minimum seconds of tmux inactivity to consider a session safe.
        #[arg(long, default_value_t = state::DEFAULT_MIN_IDLE_SECS)]
        min_idle: u64,
    },
    /// Show readiness status for all sessions.
    ///
    /// For every tmux session (except `muxr`), resolves the tool, discovers
    /// the session id, runs the readiness classifier, and prints a summary table.
    Status {
        /// Minimum seconds of inactivity to consider a session safe.
        #[arg(long, default_value_t = state::DEFAULT_MIN_IDLE_SECS)]
        min_idle: u64,
    },
    /// Interactive session switcher (TUI)
    Switch,
    /// Broadcast a slash command to every harness session.
    ///
    /// Default command is `/reload` -- useful when you've shipped an
    /// extension change and want every running Pi session to pick it
    /// up without manually relaunching each one.
    ///
    /// Filters: by default applies to every active harness session.
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
    /// Recycle a session: serialize, then reopen it FRESH from the pointer.
    ///
    /// The deliberate alternative to compacting a long session. Sends
    /// `/serialize` (so log.md is current), gracefully exits, then reopens a
    /// FRESH conversation that rehydrates from campaign.md + log.md -- no
    /// accumulated compaction drift. The previous conversation stays on disk,
    /// recoverable via --resume. No NAME uses the current session.
    Recycle {
        /// Session to recycle (e.g. storr/deploy). Omit to use the current one.
        name: Option<String>,
        /// Skip the flush-to-disk step; just exit and reopen fresh (use only
        /// if the pointer/log.md is already current).
        #[arg(long)]
        no_serialize: bool,
        /// Max seconds to wait for the agent to flush + exit before forcing it.
        /// muxr returns as soon as the agent exits; this is just the safety cap.
        #[arg(long, default_value = "600")]
        wait: u64,
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

    /// Harness subcommands (dynamic, from config)
    #[command(external_subcommand)]
    External(Vec<String>),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let tmux = Tmux::new(cli.server);

    match cli.command {
        Some(Commands::Init) => init::init(),
        Some(Commands::Ls { active }) => cmd_ls(&tmux, active),
        Some(Commands::Save) => {
            let config = Config::load()?;
            state::SavedState::save(&config, &tmux)
        }
        Some(Commands::Restore) => {
            let config = Config::load()?;
            state::SavedState::restore(&tmux, &config)
        }
        Some(Commands::TmuxStatus) => cmd_tmux_status(&tmux),
        Some(Commands::Upgrade {
            name,
            tool,
            model,
            dry_run,
            force,
            wait,
            min_idle,
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
                    wait,
                    min_idle,
                },
            )
        }
        Some(Commands::Status { min_idle }) => cmd_status(&tmux, min_idle),
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
        Some(Commands::Recycle {
            name,
            no_serialize,
            wait,
        }) => cmd_recycle(&tmux, name.as_deref(), no_serialize, wait),
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
            cmd_harness_dispatch(&tmux, &config, &args)
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
                    cmd_harness_dispatch(&tmux, &config, &cli.args)
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

/// Rename the current tmux session and flow through to the harness.
fn cmd_rename(tmux: &Tmux, name: &str, tool_override: Option<&str>) -> Result<()> {
    let old_name = tmux.current_session().unwrap_or_default();
    rename_session_by_name(tmux, &old_name, name, tool_override)
}

/// Rename a specific tmux session by name and flow through to its harness.
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
            let new_target = Tmux::target(new);
            let _ = std::process::Command::new("tmux")
                .args(["send-keys", "-t", &new_target, &cmd, "Enter"])
                .status();
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
/// 1. If a harness is running in the pane, send `/exit` and wait for the
///    process to terminate (up to 10s, then SIGKILL).
/// 2. Kill the tmux session.
/// 3. Drop the session from `state.json` so `muxr restore` won't resurrect it.
///
/// This is the counterpart to `new`: retire deletes everything new creates.
/// Broadcast a slash command to every harness session.
///
/// Use case: ship an extension change in pi-stack, then
/// `muxr broadcast` (defaults to /reload) to make every running Pi
/// session pick it up without manually relaunching each one.
///
/// Targets every tmux session whose first segment is a configured
/// harness AND that has the harness binary running in the pane.
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
        let target = Tmux::target(sname);
        let status = std::process::Command::new("tmux")
            .args(["send-keys", "-t", &target, cmd, "Enter"])
            .status();
        match status {
            Ok(s) if s.success() => {}
            _ => {
                eprintln!("  send-keys failed: {sname}");
                errors += 1;
            }
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
                let target = Tmux::target(sname);
                let _ = std::process::Command::new("tmux")
                    .args(["send-keys", "-t", &target, "/exit", "Enter"])
                    .status();

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
            if let Err(e) = cmd_recycle(tmux, Some(&session), false, 600) {
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

/// Dispatch harness subcommands: muxr claude upgrade --model X
fn cmd_harness_dispatch(tmux: &Tmux, config: &Config, args: &[String]) -> Result<()> {
    let harness_name = args.first().context("Missing harness name")?;

    let harness = config
        .tool_for(harness_name)
        .with_context(|| format!("Unknown harness: {harness_name}"))?;

    let sub = args.get(1).map(|s| s.as_str()).unwrap_or("status");

    match sub {
        "upgrade" => {
            let model = find_flag_value(&args[2..], "--model");
            let name = find_flag_value(&args[2..], "--name");
            let dry_run = args[2..].iter().any(|a| a == "--dry-run");
            let force = args[2..].iter().any(|a| a == "--force");
            let wait = find_flag_value(&args[2..], "--wait").and_then(|v| v.parse().ok());
            let min_idle = find_flag_value(&args[2..], "--min-idle")
                .and_then(|v| v.parse().ok())
                .unwrap_or(state::DEFAULT_MIN_IDLE_SECS);
            tool::upgrade(
                tmux,
                config,
                harness_name,
                &harness,
                tool::UpgradeOpts {
                    model: model.as_deref(),
                    name_filter: name.as_deref(),
                    dry_run,
                    force,
                    wait,
                    min_idle,
                },
            )
        }
        "model" => {
            let model = args.get(2).map(|s| s.as_str());
            tool::model_switch(tmux, config, harness_name, &harness, model)
        }
        other => {
            anyhow::bail!("Unknown {harness_name} subcommand: {other}\nAvailable: model, upgrade")
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
fn fmt_age(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// Print readiness status for every session. `min_idle` is the quiet period
/// (seconds) used by the classifier; the AGE column shows seconds since the
/// session's last tmux activity (a freshness cue independent of the verdict).
fn cmd_status(tmux: &Tmux, min_idle: u64) -> Result<()> {
    let config = Config::load()?;
    let sessions = tmux.list_sessions()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // One tmux round-trip for activity timestamps, reused for every row.
    let activity: std::collections::HashMap<String, u64> = tmux
        .list_sessions_detailed()
        .unwrap_or_default()
        .into_iter()
        .map(|s| (s.name, s.activity))
        .collect();

    let mut any = false;
    for (name, _) in &sessions {
        if name == "muxr" {
            continue;
        }
        any = true;
        let harness = name.split('/').next().unwrap_or(name);
        let tool_name = config.resolve_tool(harness, None);
        let tool = config.tool_for(&tool_name);

        let session_id = state::discover_session_id(tmux, name, tool.as_ref())
            .unwrap_or_else(|| "-".to_string());

        let readiness_str = if let Some(ref t) = tool {
            let r = state::session_readiness(
                tmux,
                name,
                t,
                &session_id,
                min_idle,
                activity.get(name).copied(),
            );
            match r {
                state::Readiness::Safe => "SAFE".to_string(),
                state::Readiness::Busy(reason) => format!("BUSY({reason})"),
                state::Readiness::Unknown(reason) => format!("UNKNOWN({reason})"),
            }
        } else {
            "UNKNOWN(no tool configured)".to_string()
        };

        let age = activity
            .get(name)
            .map(|a| fmt_age(now.saturating_sub(*a)))
            .unwrap_or_else(|| "-".to_string());

        println!("{name}  {tool_name}  {readiness_str}  quiet {age}");
    }

    if !any {
        eprintln!("No active sessions.");
    }
    Ok(())
}

/// Generate tmux status-left format string from config harnesses.
/// Used by tmux.conf: set -g status-left "#(muxr tmux-status)"
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
    use crate::session::{compose_launch_command, compose_recycle_message};

    #[test]
    fn recycle_message_always_appends_exit_directive() {
        // A make_durable extension that forgets to instruct an exit must still
        // get muxr's exit directive appended, or recycle hangs to SIGKILL.
        let m = compose_recycle_message("flush your state", "/exit");
        assert!(m.starts_with("flush your state"), "flush content kept: {m}");
        assert!(m.contains("run /exit"), "exit directive appended: {m}");
        // Honors a custom exit command (e.g. Pi's /quit).
        assert!(compose_recycle_message("do the thing", "/quit").contains("run /quit"));
    }

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
