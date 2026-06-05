#![deny(warnings, clippy::all)]

mod claude_status;
mod completions;
mod config;
mod init;
mod migrate;
mod primitives;
mod remote;
mod state;
mod switcher;
mod tmux;
mod tool;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::Config;
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
    /// Claude Code statusline (reads JSON from stdin, outputs ANSI)
    #[command(name = "claude-status")]
    ClaudeStatus,
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
        Some(Commands::ClaudeStatus) => claude_status::run(&tmux),
        Some(Commands::Upgrade {
            name,
            tool,
            model,
            dry_run,
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
                model.as_deref(),
                name.as_deref(),
                dry_run,
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
                    cmd_open_dispatch(&tmux, &cli.args, cli.tool.as_deref())
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
        tmux.create_session(session, &home, "")?;
        tmux.attach(session)?;
    }

    Ok(())
}

/// Route a bare `muxr <repo> [<campaign>]` invocation.
///
/// `muxr <repo>` opens the repo switchboard; `muxr <repo> <campaign>` opens
/// (or scaffolds) that campaign. Remotes are dispatched to their own handler.
fn cmd_open_dispatch(tmux: &Tmux, args: &[String], tool_override: Option<&str>) -> Result<()> {
    let config = Config::load()?;
    let name = &args[0];

    // Route to remote handler if this is a remote.
    if config.is_remote(name) {
        return cmd_open_remote(tmux, &config, name, args);
    }

    if !config.repos.contains_key(name) {
        let names = config.all_names().join(", ");
        anyhow::bail!("Unknown repo or remote: {name}\nKnown: {names}");
    }

    let dir = config.resolve_dir(name)?;
    let _ = tool_override;

    // No campaign arg -> route to the per-repo switchboard.
    if args.get(1).is_none() {
        primitives::scaffold_switchboard(&dir)?;
        return cmd_open(tmux, &config, name, primitives::SWITCHBOARD);
    }

    let campaign = args[1].as_str();
    primitives::validate_topic(campaign)?;
    if args.len() > 2 {
        let extras = args[2..].join(" ");
        anyhow::bail!(
            "Unexpected extra args: {extras}\n\
             Launch shape is: muxr <repo> <campaign>."
        );
    }

    cmd_open(tmux, &config, name, campaign)
}

/// Open or attach to a campaign session: muxr <repo> <campaign>
///
/// Resolves `campaigns/<campaign>/{campaign.md,log.md}`, scaffolding a stub
/// if missing. Composes the system prompt from the repo HARNESS prompt +
/// campaign body + log body; passes each campaign `paths:` entry as
/// `--add-dir`. The session is named `<repo>/<campaign>`.
fn cmd_open(tmux: &Tmux, config: &Config, repo_name: &str, campaign: &str) -> Result<()> {
    let repo_dir = config.resolve_dir(repo_name)?;
    // If the campaign doesn't exist yet, scaffold a stub so the human can
    // onboard it in-flow. Keeps the launch single-command from the control
    // plane.
    if !primitives::campaign_md_path(&repo_dir, campaign).is_file() {
        primitives::scaffold_campaign_stub(&repo_dir, campaign)?;
    }
    let campaign_md = primitives::campaign_file(&repo_dir, campaign)?;
    let log_path = primitives::resolve_or_scaffold_session(&repo_dir, campaign)?;

    let session_name = format!("{repo_name}/{campaign}");

    if tmux.session_exists(&session_name) {
        eprintln!("Attaching to {session_name}");
        tmux.attach(&session_name)?;
        return Ok(());
    }

    let tool = config.resolve_tool(repo_name, None);

    let (log_data, _log_body) = primitives::load_log(&log_path)?;
    if !log_data.entrypoint.is_empty() {
        eprintln!("  entrypoint: {}", log_data.entrypoint);
    }

    // A dormant campaign is resumable: if `muxr save` recorded a conversation
    // id for this session name, relaunch with --resume so opening it picks up
    // where it left off instead of starting cold. Fresh/never-run campaigns
    // have no recorded id and launch new. (Running sessions already attached
    // above.)
    let resume_id = state::SavedState::session_id_for(&session_name);
    if resume_id.is_some() {
        eprintln!("  resuming previous conversation");
    }

    // Build the launch command through the single composer so that a freshly
    // opened session, a restored session, and an upgraded session all receive
    // an identical command (modulo the resume id). This is the one place that
    // knows how to materialise a session's full launch: harness settings +
    // composed HARNESS/campaign/session prompt + campaign --add-dir paths.
    let (tool_cmd, session_dir) =
        compose_launch_command(config, &session_name, resume_id.as_deref(), None, false)?;

    // Campaign metadata, loaded only for the launch banner below.
    let (campaign_data, _campaign_body) = primitives::load_campaign(&campaign_md)?;

    config.run_pre_create_hooks(&session_dir);

    eprintln!(
        "Creating {session_name} in {} ({})",
        session_dir.display(),
        tool
    );
    if !campaign_data.synthesist_trees.is_empty() {
        eprintln!(
            "  synthesist trees: {}",
            campaign_data.synthesist_trees.join(", ")
        );
    }
    if !campaign_data.paths.is_empty() {
        eprintln!("  paths: {} added as --add-dir", campaign_data.paths.len());
    }
    tmux.create_session(&session_name, &session_dir, &tool_cmd)?;
    tmux.attach(&session_name)?;
    Ok(())
}

/// Shard the current (or named) campaign into a new sibling campaign, then
/// open it: muxr shard <new> [--repo <repo>] [--from <hub>]
///
/// The hub repo/campaign default to the session this is run from (via tmux
/// `#{session_name}`); `--repo`/`--from` override for out-of-session use. The
/// new campaign inherits the hub's category and records `sharded_from`
/// lineage, then launches as `<repo>/<new>`.
fn cmd_shard(tmux: &Tmux, new: &str, repo: Option<&str>, from: Option<&str>) -> Result<()> {
    let config = Config::load()?;

    // Resolve hub repo + campaign. Flags win; otherwise infer from the
    // current tmux session. Running from the control plane (or any non-campaign
    // session) without flags can't infer a hub -- guide the human to pass them.
    let (repo_name, hub) = match (repo, from) {
        (Some(r), Some(f)) => (r.to_string(), f.to_string()),
        _ => {
            let current = tmux.display_message("#{session_name}").unwrap_or_default();
            let inferred = parse_session(&current);
            match inferred {
                Some((r, c)) => (
                    repo.map(str::to_string).unwrap_or(r),
                    from.map(str::to_string).unwrap_or(c),
                ),
                None => anyhow::bail!(
                    "Cannot infer the hub campaign from here (current session: '{current}').\n\
                     Run `muxr shard <new>` from inside a campaign session, or pass\n\
                     `muxr shard <new> --repo <repo> --from <hub>` explicitly."
                ),
            }
        }
    };

    if new == hub {
        anyhow::bail!("Shard slug '{new}' is the same as the hub. Pick a distinct slug.");
    }
    primitives::validate_topic(new)?;

    if !config.repos.contains_key(&repo_name) {
        let names = config.all_names().join(", ");
        anyhow::bail!("Unknown repo: {repo_name}\nKnown: {names}");
    }
    let repo_dir = config.resolve_dir(&repo_name)?;

    let new_md = primitives::scaffold_shard(&repo_dir, &hub, new)?;
    eprintln!();
    eprintln!("Sharded '{hub}' -> '{new}' ({})", new_md.display());
    eprintln!();

    cmd_open(tmux, &config, &repo_name, new)
}

/// Re-anchor a live session to its current on-disk campaign + log: muxr reorient [name]
///
/// Sends a single-line nudge into the pane telling the agent to re-read its
/// campaign.md + log.md now. The system prompt already carries a standing
/// re-read pointer; this is the explicit, on-demand trigger (e.g. right after
/// a `/compact`). It does not relaunch and does not touch the conversation.
fn cmd_reorient(tmux: &Tmux, name: Option<&str>) -> Result<()> {
    let config = Config::load()?;

    let session = match name {
        Some(n) => n.to_string(),
        None => tmux.current_session().context(
            "Not inside a muxr session. Run `muxr reorient <repo>/<campaign>` \
             or run it from within the session's pane.",
        )?,
    };

    let (repo_name, campaign) = parse_session(&session)
        .with_context(|| format!("'{session}' is not a <repo>/<campaign> session"))?;
    let repo_dir = config.resolve_dir(&repo_name)?;
    let campaign_md = primitives::campaign_md_path(&repo_dir, &campaign);
    let log_md = primitives::log_md_path(&repo_dir, &campaign);

    if !campaign_md.is_file() {
        anyhow::bail!(
            "No campaign on disk for {session} (expected {}).",
            campaign_md.display()
        );
    }

    // One line (send-keys can't carry newlines reliably). The agent pulls the
    // current full state itself by re-reading the files.
    let nudge = format!(
        "[muxr reorient] Re-read your durable state NOW before continuing -- \
         the conversation context may be stale or compacted. Read {} and {}, \
         then resume from the current entrypoint.",
        campaign_md.display(),
        log_md.display()
    );

    tmux.send_text(&session, &nudge)?;
    eprintln!("Reoriented {session} (nudged to re-read campaign.md + log.md).");
    Ok(())
}

/// Migrate a repo's campaigns/ tree to the 2-level layout.
///
/// Resolves the repo dir from `--dir` (explicit, config-free) or the config
/// repo key, builds the plan, prints it, and applies it unless `--dry_run`.
fn cmd_migrate_layout(
    repo: Option<&str>,
    dir: Option<&std::path::Path>,
    dry_run: bool,
    keep_archives: bool,
) -> Result<()> {
    let repo_dir = match (dir, repo) {
        (Some(d), _) => d.to_path_buf(),
        (None, Some(r)) => {
            let config = Config::load()?;
            config.resolve_dir(r)?
        }
        (None, None) => anyhow::bail!(
            "Specify a repo to migrate: `muxr migrate-layout <repo>` or `--dir <path>`."
        ),
    };

    let plan = migrate::plan(&repo_dir)?;
    let opts = migrate::Opts {
        dry_run,
        keep_archives,
    };
    migrate::print_plan(&repo_dir, &plan, &opts);
    migrate::execute(&repo_dir, &plan, &opts)?;
    Ok(())
}

/// Split a tmux session name into `(repo, campaign)`.
///
/// Session names are exactly two levels: `<repo>/<campaign>`. Campaigns are
/// validated kebab-case, so they never contain a slash; a name with more or
/// fewer than two non-empty components is not a campaign session (e.g. the
/// `muxr` control plane, or remote proxy names).
pub(crate) fn parse_session(name: &str) -> Option<(String, String)> {
    let (repo, campaign) = name.split_once('/')?;
    if repo.is_empty() || campaign.is_empty() || campaign.contains('/') {
        return None;
    }
    Some((repo.to_string(), campaign.to_string()))
}

/// Materialise the full launch command for a session: the single source of
/// truth shared by `open`, `restore`, and `upgrade`.
///
/// Reconstructs the repo's launch settings, the composed HARNESS + campaign +
/// log system prompt (written to a temp file), and the campaign's `--add-dir`
/// paths, then asks the tool to assemble the command. The binary name is
/// resolved fresh from config each call, so a relaunch picks up a newly
/// installed harness version.
///
/// `resume_id` resumes a specific conversation; when it is `None` and
/// `continue_fallback` is set, the tool's `--continue` args are appended so a
/// restored session without a discovered id still re-attaches to its most
/// recent conversation. Returns the command and the session directory.
///
/// The campaign and log files are loaded best-effort; callers that might not
/// have them (restore/upgrade of an archived session) get a degraded
/// composition rather than a failure.
pub(crate) fn compose_launch_command(
    config: &Config,
    session_name: &str,
    resume_id: Option<&str>,
    model: Option<&str>,
    continue_fallback: bool,
) -> Result<(String, std::path::PathBuf)> {
    let (repo_name, campaign) = parse_session(session_name)
        .with_context(|| format!("cannot derive repo/campaign from '{session_name}'"))?;

    let repo_dir = config.resolve_dir(&repo_name)?;
    let campaign_md = primitives::campaign_md_path(&repo_dir, &campaign);
    let log_path = primitives::log_md_path(&repo_dir, &campaign);

    let tool = config.resolve_tool(&repo_name, None);
    let tool_config = config.tool_for(&tool);
    let repo = config.repos.get(&repo_name);

    // Start from the repo's existing launch settings; layer campaign
    // paths and the composed prompt on top.
    let mut settings = repo.map(|v| v.launch.clone()).unwrap_or_default();

    // Campaign and log files are loaded best-effort. A relaunch must keep
    // the repo-level prompt and campaign --add-dir paths even when the log
    // body file is missing -- e.g. an archived-but-still-running session.
    // Without this an in-place upgrade of such a session would silently drop
    // its HARNESS rules and working dirs. Missing or unparseable files
    // degrade the composition (empty body / no campaign paths) instead of
    // failing the whole relaunch. Genuinely fatal errors (unknown repo,
    // unparseable slug) are still surfaced above, so callers keep their
    // name+resume fallback for the truly-unrecoverable case.
    let (campaign_data, campaign_body) =
        primitives::load_campaign(&campaign_md).unwrap_or_default();
    // Only the entrypoint (the movable pointer) goes inline; the full log body
    // stays on disk and is pointed at, not snapshotted into the prompt.
    let entrypoint = primitives::load_log(&log_path)
        .map(|(log, _)| log.entrypoint)
        .unwrap_or_default();

    let composed =
        primitives::compose_prompt(&campaign, &campaign_body, &entrypoint, &campaign_md, &log_path);

    // Claude Code rejects --append-system-prompt and --append-system-prompt-file
    // together, and multi-line content via shell send-keys breaks parsing.
    // Resolve EVERY configured HARNESS.md-style prompt file, combine them with
    // the composed campaign+log prompt, write to a single temp file, and pass
    // only that one --append-system-prompt-file.
    //
    // Both the plural `append_system_prompt_files` (base + overlay, e.g.
    // HARNESS-base.md + a per-repo HARNESS.md) and the singular
    // `append_system_prompt_file` must be folded in here. The plural takes
    // precedence, mirroring launch_command_with_settings. Crucially we then
    // CLEAR the plural array below: otherwise launch_command_with_settings
    // would prefer the (un-composed) plural array and silently drop this
    // composed temp file -- which dropped the campaign + log entrypoint from
    // the system prompt for every repo configured with the array.
    let harness_files: Vec<String> = settings
        .append_system_prompt_files
        .clone()
        .or_else(|| {
            settings
                .append_system_prompt_file
                .clone()
                .map(|f| vec![f])
        })
        .unwrap_or_default();

    let mut harness_md_content = String::new();
    for file in &harness_files {
        let expanded = shellexpand::tilde(file).to_string();
        let path = if expanded.starts_with('/') {
            std::path::PathBuf::from(expanded)
        } else {
            repo_dir.join(&expanded)
        };
        if let Ok(text) = std::fs::read_to_string(&path) {
            if !harness_md_content.is_empty() {
                harness_md_content.push_str("\n\n");
            }
            harness_md_content.push_str(text.trim_end());
        }
    }

    let full_prompt = if harness_md_content.trim().is_empty() {
        composed
    } else {
        format!("{}\n\n---\n\n{}", harness_md_content.trim_end(), composed)
    };

    let tmp_path = std::env::temp_dir().join(format!("muxr-prompt-{repo_name}-{campaign}.md"));
    std::fs::write(&tmp_path, &full_prompt)?;

    // Replace both prompt-file fields with our single composed temp file. The
    // plural array MUST be cleared so launch uses the composed file, not the
    // raw HARNESS files (which omit the campaign + log).
    settings.append_system_prompt = None;
    settings.append_system_prompt_files = None;
    settings.append_system_prompt_file = Some(tmp_path.to_string_lossy().to_string());

    for path in &campaign_data.paths {
        let expanded = primitives::expand_home(path);
        if !settings.add_dirs.iter().any(|d| d == &expanded) {
            settings.add_dirs.push(expanded);
        }
    }

    let tool_cmd = match &tool_config {
        Some(h) => {
            let mut cmd =
                h.launch_command_with_settings(Some(session_name), resume_id, model, &settings)?;
            // No discovered id but a continue fallback was requested: re-attach
            // to the most recent conversation rather than starting cold.
            if resume_id.is_none() && continue_fallback {
                for arg in &h.continue_args {
                    cmd.push(' ');
                    cmd.push_str(arg);
                }
            }
            cmd
        }
        None => tool.clone(),
    };

    Ok((tool_cmd, repo_dir))
}

/// Open or attach to a remote proxy session: muxr lab bootc
fn cmd_open_remote(tmux: &Tmux, config: &Config, remote_name: &str, args: &[String]) -> Result<()> {
    let remote = config
        .remote(remote_name)
        .context("Remote not found in config")?;

    let context = if args.len() >= 2 {
        args[1..].join("/")
    } else {
        "default".to_string()
    };

    // "muxr lab ls" lists remote instances and their sessions
    if context == "ls" {
        return cmd_remote_ls(remote, remote_name);
    }

    let session = format!("{remote_name}/{context}");

    let instance = remote.instance_name(&context);

    if tmux.session_exists(&session) {
        eprintln!("Attaching to {session} (remote)");
        tmux.attach(&session)?;
    } else {
        // Bootstrap Claude config on first connect
        if let Err(e) = remote::bootstrap_claude_config(remote, &instance) {
            eprintln!("  Bootstrap warning: {e}");
        }

        let connect_cmd = remote::connect_command(remote, &instance, &context)?;
        eprintln!("Creating {session} -> {instance} via {}", remote.connect);
        let home = dirs::home_dir().context("No home directory")?;
        tmux.create_session(&session, &home, &connect_cmd)?;
        tmux.attach(&session)?;
    }

    Ok(())
}

/// List running instances and their remote tmux sessions.
fn cmd_remote_ls(remote: &config::Remote, remote_name: &str) -> Result<()> {
    let instances = remote::list_instances(remote)?;
    if instances.is_empty() {
        println!("No running instances for {remote_name}");
        return Ok(());
    }

    for instance in &instances {
        println!("  {instance}:");
        match remote::list_remote_sessions(remote, instance) {
            Ok(sessions) if !sessions.is_empty() => {
                for sname in sessions {
                    println!("    {remote_name}/{sname}");
                }
            }
            _ => println!("    (no tmux sessions)"),
        }
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
    let old_path = primitives::campaign_dir(&dir, &old_campaign);
    let new_path = primitives::campaign_dir(&dir, &new_campaign);
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
    // longer exist in tmux, so `save` naturally excludes them — no manual
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
/// `kill -0` polls is suppressed — when the pid is gone the helper prints
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
/// campaign, or create a new one.
fn cmd_switch(tmux: &Tmux) -> Result<()> {
    match switcher::run(tmux)? {
        switcher::Action::Switch(session) => tmux.attach(&session),
        switcher::Action::Open(repo, campaign) => {
            let config = Config::load()?;
            cmd_open(tmux, &config, &repo, &campaign)
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
            let dry_run = args[2..].iter().any(|a| a == "--dry-run");
            tool::upgrade(
                tmux,
                config,
                harness_name,
                &harness,
                model.as_deref(),
                None,
                dry_run,
            )
        }
        "model" => {
            let model = args.get(2).map(|s| s.as_str());
            tool::model_switch(tmux, config, harness_name, &harness, model)
        }
        "compact" => {
            let threshold =
                find_flag_value(&args[2..], "--threshold").and_then(|v| v.parse::<u32>().ok());
            tool::compact(tmux, config, harness_name, &harness, threshold)
        }
        "status" => tool::status(tmux, config, harness_name, &harness),
        other => {
            anyhow::bail!(
                "Unknown {harness_name} subcommand: {other}\nAvailable: model, compact, upgrade, status"
            )
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
            assert!(composed.contains(marker), "composed prompt missing {marker}:\n{composed}");
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
