#![deny(warnings, clippy::all)]

mod claude_status;
mod completions;
mod config;
mod init;
mod primitives;
mod tool;
mod remote;
mod state;
mod switcher;
mod tmux;

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

    /// Vertical name (e.g., work, personal, oss)
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
        /// Session name (e.g. tanuki/2026-04-24) or "all" to retire every
        /// tmux session.
        name: String,
    },
    /// Interactive session switcher (TUI)
    Switch,
    /// Generate shell completions (zsh, bash, fish)
    Completions {
        /// Shell to generate completions for
        shell: String,
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
        Some(Commands::Switch) => cmd_switch(&tmux),
        Some(Commands::Rename { name }) => cmd_rename(&tmux, &name, cli.tool.as_deref()),
        Some(Commands::Kill { name }) => cmd_kill(&tmux, &name),
        Some(Commands::Retire { name }) => cmd_retire(&tmux, &name),
        Some(Commands::Completions { shell }) => completions::generate(&shell),
        Some(Commands::External(args)) => {
            let config = Config::load()?;
            cmd_harness_dispatch(&tmux, &config, &args)
        }
        None => {
            if cli.args.is_empty() {
                cmd_control_plane(&tmux)
            } else {
                // Check if first arg is a harness name before treating as harness
                let first = &cli.args[0];
                let config = Config::load().ok();
                let is_harness = config
                    .as_ref()
                    .map(|c| c.tool_names().contains(&first.to_string()))
                    .unwrap_or(false);

                if is_harness {
                    let config = config.unwrap();
                    cmd_harness_dispatch(&tmux, &config, &cli.args)
                } else {
                    cmd_open(&tmux, &cli.args, cli.tool.as_deref())
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

/// Open or attach to a session: muxr work api auth
fn cmd_open(tmux: &Tmux, args: &[String], tool_override: Option<&str>) -> Result<()> {
    let config = Config::load()?;
    let name = &args[0];

    // Route to remote handler if this is a remote harness
    if config.is_remote(name) {
        return cmd_open_remote(tmux, &config, name, args);
    }

    if !config.harnesses.contains_key(name) {
        let names = config.all_names().join(", ");
        anyhow::bail!("Unknown harness or remote: {name}\nKnown: {names}");
    }

    let dir = config.resolve_dir(name)?;

    // No campaign arg -> route to the harness switchboard (auto-scaffold
    // on first launch).
    let switchboard_slug: String;
    let campaign: &str = if let Some(arg) = args.get(1) {
        arg.as_str()
    } else {
        primitives::scaffold_switchboard(&dir)?;
        switchboard_slug = primitives::SWITCHBOARD.to_string();
        &switchboard_slug
    };

    let date = args
        .get(2)
        .cloned()
        .unwrap_or_else(primitives::today);
    let _ = tool_override;
    cmd_open_campaign(tmux, &config, name, campaign, &date)
}

/// Open or attach to a campaign session: muxr <harness> <campaign> [<date>]
///
/// Resolves `campaigns/<campaign>/sessions/<date>[-<suffix>].md`, scaffolding
/// from `campaigns/TEMPLATE/sessions/TEMPLATE.md` if no same-day file exists.
/// Composes system prompt from the campaign body + session body; passes each
/// campaign `paths:` entry as `--add-dir`.
fn cmd_open_campaign(
    tmux: &Tmux,
    config: &Config,
    harness_name: &str,
    campaign: &str,
    date: &str,
) -> Result<()> {
    let harness_dir = config.resolve_dir(harness_name)?;
    // If the campaign doesn't exist yet, prompt interactively so the
    // human can scaffold it in-flow. Keeps the launch single-command
    // from the muxr control plane.
    let campaign_md_path = harness_dir
        .join("campaigns")
        .join(campaign)
        .join("campaign.md");
    if !campaign_md_path.is_file() {
        primitives::scaffold_campaign_stub(&harness_dir, campaign)?;
    }
    let campaign_md = primitives::campaign_file(&harness_dir, campaign)?;
    let session_path =
        primitives::resolve_or_scaffold_session(&harness_dir, campaign, date)?;

    let session_basename = session_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(date);
    let session_name = format!("{harness_name}/{campaign}/{session_basename}");

    if tmux.session_exists(&session_name) {
        eprintln!("Attaching to {session_name}");
        tmux.attach(&session_name)?;
        return Ok(());
    }

    let tool = config.resolve_tool(harness_name, None);
    let tool_config = config.tool_for(&tool);
    let harness = config.harnesses.get(harness_name);

    // Start from the harness's existing launch settings; layer campaign
    // paths and the composed prompt on top.
    let mut settings = harness
        .map(|v| v.launch.clone())
        .unwrap_or_default();

    let (campaign_data, campaign_body) = primitives::load_campaign(&campaign_md)?;
    let (session_data, session_body) = primitives::load_session(&session_path)?;

    // Schema validation: session's campaign must match requested campaign.
    if session_data.campaign != campaign {
        anyhow::bail!(
            "Session file {} declares campaign '{}' but was opened as '{}'.",
            session_path.display(),
            session_data.campaign,
            campaign
        );
    }
    if !session_data.entrypoint.is_empty() {
        eprintln!("  entrypoint: {}", session_data.entrypoint);
    }

    let composed = primitives::compose_prompt(campaign, &campaign_body, &session_body);

    // Claude Code rejects --append-system-prompt and --append-system-prompt-file
    // together. Also, multi-line content via shell send-keys breaks shell
    // parsing. Solution: resolve any configured HARNESS.md-style file,
    // combine with the composed campaign+session prompt, write to a single
    // temp file, pass only --append-system-prompt-file.
    let harness_md_content = if let Some(ref file) = settings.append_system_prompt_file {
        let expanded = shellexpand::tilde(file).to_string();
        let path = if expanded.starts_with('/') {
            std::path::PathBuf::from(expanded)
        } else {
            harness_dir.join(&expanded)
        };
        std::fs::read_to_string(&path).unwrap_or_default()
    } else {
        String::new()
    };

    let full_prompt = if harness_md_content.trim().is_empty() {
        composed
    } else {
        format!("{}\n\n---\n\n{}", harness_md_content.trim_end(), composed)
    };

    let tmp_path = std::env::temp_dir().join(format!(
        "muxr-prompt-{}-{}-{}.md",
        harness_name,
        campaign,
        session_basename
    ));
    std::fs::write(&tmp_path, &full_prompt)?;

    // Clear the inline and replace the file with our composed temp file.
    settings.append_system_prompt = None;
    settings.append_system_prompt_file = Some(tmp_path.to_string_lossy().to_string());

    for path in &campaign_data.paths {
        let expanded = primitives::expand_home(path);
        if !settings.add_dirs.iter().any(|d| d == &expanded) {
            settings.add_dirs.push(expanded);
        }
    }

    let session_dir = harness_dir.clone();
    config.run_pre_create_hooks(&session_dir);

    let tool_cmd = match &tool_config {
        Some(h) => {
            h.launch_command_with_settings(Some(&session_name), None, None, &settings)
        }
        None => tool.clone(),
    };

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

/// Open or attach to a remote proxy session: muxr lab bootc
fn cmd_open_remote(
    tmux: &Tmux,
    config: &Config,
    remote_name: &str,
    args: &[String],
) -> Result<()> {
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
        eprintln!(
            "Creating {session} -> {instance} via {}",
            remote.connect
        );
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

/// Move the session file on disk to match a tmux rename.
///
/// Convention (set by the `serialize` skill): each session file lives at
/// `<harness_dir>/campaigns/<campaign>/sessions/<segment>.md`, where
/// `<segment>` is the trailing component of the tmux session name
/// (e.g. `2026-04-24-foo` from `tanuki/harness/2026-04-24-foo`).
///
/// Both old and new tmux session names must follow `<harness>/<campaign>/<segment>`
/// for the move to fire. Anything else (bare names, mismatched harnesses,
/// missing source file) is silently skipped — this is a hint, not a
/// correctness requirement.
fn try_move_session_file(config: &Config, old: &str, new: &str) {
    let Some((old_h, old_campaign, old_seg)) = parse_three_part(old) else {
        return;
    };
    let Some((new_h, new_campaign, new_seg)) = parse_three_part(new) else {
        return;
    };
    if old_h != new_h || old_campaign != new_campaign {
        // We don't move files across harnesses or campaigns from a rename.
        return;
    }
    let dir = match config.resolve_dir(old_h) {
        Ok(p) => p,
        Err(_) => return,
    };
    let base = dir
        .join("campaigns")
        .join(old_campaign)
        .join("sessions");
    let old_path = base.join(format!("{old_seg}.md"));
    let new_path = base.join(format!("{new_seg}.md"));
    if !old_path.exists() {
        return;
    }
    if new_path.exists() {
        eprintln!(
            "Session file at {} already exists; not overwriting",
            new_path.display()
        );
        return;
    }
    match std::fs::rename(&old_path, &new_path) {
        Ok(()) => eprintln!(
            "Moved session file: {} -> {}",
            old_path.display(),
            new_path.display()
        ),
        Err(e) => eprintln!(
            "Could not move session file {} -> {}: {e}",
            old_path.display(),
            new_path.display()
        ),
    }
}

fn parse_three_part(name: &str) -> Option<(&str, &str, &str)> {
    let mut parts = name.splitn(3, '/');
    let h = parts.next()?;
    let c = parts.next()?;
    let s = parts.next()?;
    if h.is_empty() || c.is_empty() || s.is_empty() {
        return None;
    }
    Some((h, c, s))
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
                        .find(|pid| harness_proc_match(*pid, &harness.bin))
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

/// Check if a PID is running the named harness binary. Matches against full
/// argv, not just `comm`, because node-based harnesses (claude-code) run as
/// `node /path/to/claude …` where comm is `node`.
fn harness_proc_match(pid: u32, bin: &str) -> bool {
    use std::process::Stdio;
    let suffix = format!("/{bin}");
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .stderr(Stdio::null())
        .output()
        .map(|o| {
            let args = String::from_utf8_lossy(&o.stdout);
            args.split_whitespace()
                .any(|tok| tok == bin || tok.ends_with(&suffix))
        })
        .unwrap_or(false)
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

/// Interactive TUI session switcher.
fn cmd_switch(tmux: &Tmux) -> Result<()> {
    match switcher::run(tmux)? {
        switcher::Action::Switch(session) => tmux.attach(&session),
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
            tool::upgrade(tmux, config, harness_name, &harness, model.as_deref())
        }
        "model" => {
            let model = args.get(2).map(|s| s.as_str());
            tool::model_switch(tmux, config, harness_name, &harness, model)
        }
        "compact" => {
            let threshold = find_flag_value(&args[2..], "--threshold")
                .and_then(|v| v.parse::<u32>().ok());
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
        let config: Config = toml::from_str("[harnesses]").unwrap();
        assert_eq!(config.resolve_tool("work", Some("opencode")), "opencode");
    }

    #[test]
    fn resolve_tool_falls_back_to_config() {
        let config: Config = toml::from_str("[harnesses]").unwrap();
        assert_eq!(config.resolve_tool("work", None), "claude");
    }

    #[test]
    fn parse_three_part_splits_on_two_slashes() {
        assert_eq!(
            parse_three_part("tanuki/harness/2026-04-24-foo"),
            Some(("tanuki", "harness", "2026-04-24-foo"))
        );
    }

    #[test]
    fn parse_three_part_keeps_extra_slashes_in_segment() {
        assert_eq!(
            parse_three_part("tanuki/harness/2026/04/24"),
            Some(("tanuki", "harness", "2026/04/24"))
        );
    }

    #[test]
    fn parse_three_part_rejects_two_part_names() {
        assert_eq!(parse_three_part("tanuki/harness"), None);
        assert_eq!(parse_three_part("just-a-name"), None);
    }

    #[test]
    fn parse_three_part_rejects_empty_components() {
        assert_eq!(parse_three_part("/harness/foo"), None);
        assert_eq!(parse_three_part("tanuki//foo"), None);
        assert_eq!(parse_three_part("tanuki/harness/"), None);
    }

    #[test]
    fn try_move_session_file_moves_when_source_exists() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("campaigns/lab/sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let old_path = sessions.join("2026-04-24.md");
        std::fs::write(&old_path, "session body").unwrap();

        let toml = format!(
            "[harnesses.tanuki]\ndir = {dir:?}\ncolor = \"#fff\"\n",
            dir = dir.path()
        );
        let config: Config = toml::from_str(&toml).unwrap();

        try_move_session_file(&config, "tanuki/lab/2026-04-24", "tanuki/lab/2026-04-24-named");

        assert!(!old_path.exists(), "old should be gone");
        let new_path = sessions.join("2026-04-24-named.md");
        assert!(new_path.exists(), "new should exist");
        assert_eq!(std::fs::read_to_string(new_path).unwrap(), "session body");
    }

    #[test]
    fn try_move_session_file_silent_when_source_missing() {
        let dir = tempfile::tempdir().unwrap();
        let toml = format!(
            "[harnesses.tanuki]\ndir = {dir:?}\ncolor = \"#fff\"\n",
            dir = dir.path()
        );
        let config: Config = toml::from_str(&toml).unwrap();
        // No file at the expected location. Should not panic, should not create anything.
        try_move_session_file(&config, "tanuki/lab/2026-04-24", "tanuki/lab/2026-04-24-named");
        let sessions = dir.path().join("campaigns/lab/sessions");
        assert!(!sessions.exists() || std::fs::read_dir(&sessions).unwrap().next().is_none());
    }

    #[test]
    fn try_move_session_file_refuses_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("campaigns/lab/sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let old_path = sessions.join("2026-04-24.md");
        let new_path = sessions.join("2026-04-24-named.md");
        std::fs::write(&old_path, "old").unwrap();
        std::fs::write(&new_path, "existing").unwrap();

        let toml = format!(
            "[harnesses.tanuki]\ndir = {dir:?}\ncolor = \"#fff\"\n",
            dir = dir.path()
        );
        let config: Config = toml::from_str(&toml).unwrap();

        try_move_session_file(&config, "tanuki/lab/2026-04-24", "tanuki/lab/2026-04-24-named");

        assert!(old_path.exists(), "old must not have been clobbered");
        assert_eq!(std::fs::read_to_string(new_path).unwrap(), "existing");
    }

    #[test]
    fn try_move_session_file_skips_cross_campaign() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = dir.path().join("campaigns/lab/sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let old_path = sessions.join("2026-04-24.md");
        std::fs::write(&old_path, "x").unwrap();
        let toml = format!(
            "[harnesses.tanuki]\ndir = {dir:?}\ncolor = \"#fff\"\n",
            dir = dir.path()
        );
        let config: Config = toml::from_str(&toml).unwrap();

        try_move_session_file(&config, "tanuki/lab/2026-04-24", "tanuki/different/2026-04-24");

        // Cross-campaign move is not supported; old file stays.
        assert!(old_path.exists());
    }
}
