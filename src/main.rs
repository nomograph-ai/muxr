#![deny(warnings, clippy::all)]

mod claude_status;
mod completions;
mod config;
mod harness;
mod init;
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
    Ls,
    /// Snapshot sessions before reboot
    Save,
    /// Restore sessions after reboot
    Restore,
    /// Generate tmux status-left config from verticals
    #[command(name = "tmux-status")]
    TmuxStatus,
    /// Claude Code statusline (reads JSON from stdin, outputs ANSI)
    #[command(name = "claude-status")]
    ClaudeStatus,
    /// Create a session in the background (don't attach)
    New {
        /// Override the default tool (e.g., --tool opencode)
        #[arg(long)]
        tool: Option<String>,

        /// Vertical and context (e.g., work api auth)
        #[arg(num_args = 1..)]
        args: Vec<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_tool_uses_override() {
        let config: Config = toml::from_str("[verticals]").unwrap();
        assert_eq!(config.resolve_tool("work", Some("opencode")), "opencode");
    }

    #[test]
    fn resolve_tool_falls_back_to_config() {
        let config: Config = toml::from_str("[verticals]").unwrap();
        assert_eq!(config.resolve_tool("work", None), "claude");
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let tmux = Tmux::new(cli.server);

    match cli.command {
        Some(Commands::Init) => init::init(),
        Some(Commands::Ls) => cmd_ls(&tmux),
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
        Some(Commands::New { tool, args }) => cmd_new(&tmux, &args, tool.as_deref()),
        Some(Commands::Rename { name }) => cmd_rename(&tmux, &name, cli.tool.as_deref()),
        Some(Commands::Kill { name }) => cmd_kill(&tmux, &name),
        Some(Commands::Completions { shell }) => completions::generate(&shell),
        Some(Commands::External(args)) => {
            let config = Config::load()?;
            cmd_harness_dispatch(&tmux, &config, &args)
        }
        None => {
            if cli.args.is_empty() {
                cmd_control_plane(&tmux)
            } else {
                // Check if first arg is a harness name before treating as vertical
                let first = &cli.args[0];
                let config = Config::load().ok();
                let is_harness = config
                    .as_ref()
                    .map(|c| c.harness_names().contains(&first.to_string()))
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

    // Route to remote handler if this is a remote vertical
    if config.is_remote(name) {
        return cmd_open_remote(tmux, &config, name, args);
    }

    let tool = config.resolve_tool(name, tool_override);
    let harness = config.harness_for(&tool);

    if !config.verticals.contains_key(name) {
        let names = config.all_names().join(", ");
        anyhow::bail!("Unknown vertical or remote: {name}\nKnown: {names}");
    }

    let session = if args.len() >= 2 {
        let context = args[1..].join("/");
        format!("{name}/{context}")
    } else {
        format!("{name}/default")
    };

    let dir = config.resolve_dir(name)?;

    if tmux.session_exists(&session) {
        eprintln!("Attaching to {session}");
        tmux.attach(&session)?;
    } else {
        let vertical = config.verticals.get(name);
        let use_worktree = vertical.map(|v| v.worktree).unwrap_or(false)
            && harness.is_some()
            && tmux::is_git_repo(&dir);

        let session_dir = if use_worktree {
            let context = if args.len() >= 2 {
                args[1..].join("/")
            } else {
                "default".to_string()
            };
            let wt = tmux::create_worktree(&dir, &context)?;
            eprintln!("Creating {session} in {} (worktree, {})", wt.display(), tool);
            wt
        } else {
            eprintln!("Creating {session} in {} ({})", dir.display(), tool);
            dir.clone()
        };

        config.run_pre_create_hooks(&session_dir);
        let tool_cmd = match (&harness, vertical) {
            (Some(h), Some(v)) => h.launch_command_with_settings(Some(&session), None, None, &v.harness),
            (Some(h), None) => h.launch_command(Some(&session), None, None),
            _ => tool.clone(),
        };
        tmux.create_session(&session, &session_dir, &tool_cmd)?;
        tmux.attach(&session)?;
    }

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

/// Create a session in the background without attaching.
fn cmd_new(tmux: &Tmux, args: &[String], tool_override: Option<&str>) -> Result<()> {
    let config = Config::load()?;
    let name = &args[0];

    let context = if args.len() >= 2 {
        args[1..].join("/")
    } else {
        "default".to_string()
    };

    let session = format!("{name}/{context}");

    if tmux.session_exists(&session) {
        eprintln!("{session} already exists");
        return Ok(());
    }

    if config.is_remote(name) {
        let remote = config.remote(name).context("Remote not found")?;
        let instance = remote.instance_name(&context);
        if let Err(e) = remote::bootstrap_claude_config(remote, &instance) {
            eprintln!("  Bootstrap warning: {e}");
        }
        let connect_cmd = remote::connect_command(remote, &instance, &context)?;
        let home = dirs::home_dir().context("No home directory")?;
        tmux.create_session(&session, &home, &connect_cmd)?;
        eprintln!("Created {session} -> {instance} (remote)");
    } else if config.verticals.contains_key(name) {
        let tool = config.resolve_tool(name, tool_override);
        let harness = config.harness_for(&tool);
        let vertical = config.verticals.get(name);
        let dir = config.resolve_dir(name)?;

        let use_worktree = vertical.map(|v| v.worktree).unwrap_or(false)
            && harness.is_some()
            && tmux::is_git_repo(&dir);

        let session_dir = if use_worktree {
            tmux::create_worktree(&dir, &context)?
        } else {
            dir.clone()
        };

        config.run_pre_create_hooks(&session_dir);
        let tool_cmd = match (&harness, vertical) {
            (Some(h), Some(v)) => {
                h.launch_command_with_settings(Some(&session), None, None, &v.harness)
            }
            (Some(h), None) => h.launch_command(Some(&session), None, None),
            _ => tool.clone(),
        };
        tmux.create_session(&session, &session_dir, &tool_cmd)?;
        let wt_label = if use_worktree { " (worktree)" } else { "" };
        eprintln!("Created {session} ({tool}){wt_label}");
    } else {
        let names = config.all_names().join(", ");
        anyhow::bail!("Unknown vertical or remote: {name}\nKnown: {names}");
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

    let vertical = old.split('/').next().unwrap_or("default");

    tmux.rename_session(Some(old), new)?;
    eprintln!("Renamed {old} -> {new}");

    // Flow rename through to the harness if configured
    if let Ok(config) = Config::load() {
        let tool = config.resolve_tool(vertical, tool_override);
        if let Some(harness) = config.harness_for(&tool)
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

/// Kill a session or all sessions. Cleans up worktrees if configured.
fn cmd_kill(tmux: &Tmux, name: &str) -> Result<()> {
    let config = Config::load().ok();

    let kill_one = |sname: &str| {
        tmux.kill_session(sname).ok();
        eprintln!("Killed {sname}");

        // Clean up worktree if this vertical uses worktrees
        if let Some(ref config) = config {
            let vertical = sname.split('/').next().unwrap_or(sname);
            let context = sname.split('/').skip(1).collect::<Vec<_>>().join("/");
            if let Ok(dir) = config.resolve_dir(vertical)
                && config
                    .verticals
                    .get(vertical)
                    .map(|v| v.worktree)
                    .unwrap_or(false)
            {
                let ctx = if context.is_empty() {
                    "default"
                } else {
                    &context
                };
                if let Err(e) = tmux::remove_worktree(&dir, ctx) {
                    eprintln!("  worktree cleanup: {e}");
                }
            }
        }
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

fn cmd_ls(tmux: &Tmux) -> Result<()> {
    let config = Config::load().ok();
    let sessions = tmux.list_sessions()?;
    if sessions.is_empty() {
        eprintln!("No active tmux sessions.");
    } else {
        for (name, path) in &sessions {
            let vertical = name.split('/').next().unwrap_or(name);
            let is_remote = config
                .as_ref()
                .map(|c| c.is_remote(vertical))
                .unwrap_or(false);

            if is_remote {
                println!("  {name}  (remote)");
            } else {
                println!("  {name}  ({path})");
            }
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
        .harness_for(harness_name)
        .with_context(|| format!("Unknown harness: {harness_name}"))?;

    let sub = args.get(1).map(|s| s.as_str()).unwrap_or("status");

    match sub {
        "upgrade" => {
            let model = find_flag_value(&args[2..], "--model");
            harness::upgrade(tmux, config, harness_name, &harness, model.as_deref())
        }
        "model" => {
            let model = args.get(2).map(|s| s.as_str());
            harness::model_switch(tmux, config, harness_name, &harness, model)
        }
        "compact" => {
            let threshold = find_flag_value(&args[2..], "--threshold")
                .and_then(|v| v.parse::<u32>().ok());
            harness::compact(tmux, config, harness_name, &harness, threshold)
        }
        "fork" => harness::fork(tmux, config, harness_name, &harness),
        "status" => harness::status(tmux, config, harness_name, &harness),
        other => {
            anyhow::bail!(
                "Unknown {harness_name} subcommand: {other}\nAvailable: model, compact, fork, upgrade, status"
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

/// Generate tmux status-left format string from config verticals.
/// Used by tmux.conf: set -g status-left "#(muxr tmux-status)"
fn cmd_tmux_status(tmux: &Tmux) -> Result<()> {
    let session_name = tmux.display_message("#{session_name}")?;

    let vertical = session_name.split('/').next().unwrap_or(&session_name);

    let config = Config::load().ok();
    let color = config
        .as_ref()
        .map(|c| c.color_for(vertical).to_string())
        .unwrap_or_else(|| "#8a7f83".to_string());

    // Output tmux format string
    print!("#[fg={color}]● #[fg=#E8DDD0]{session_name} #[fg=#3B3639]│ ");

    Ok(())
}
