mod claude_status;
mod completions;
mod config;
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
struct Cli {
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
}

fn resolve_tool(tool_override: Option<&str>, config: &Config) -> String {
    match tool_override {
        Some(t) => t.to_string(),
        None => config.default_tool.clone(),
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
        Some(Commands::Restore) => state::SavedState::restore(&tmux),
        Some(Commands::TmuxStatus) => cmd_tmux_status(&tmux),
        Some(Commands::ClaudeStatus) => claude_status::run(&tmux),
        Some(Commands::Switch) => cmd_switch(&tmux),
        Some(Commands::New { tool, args }) => cmd_new(&tmux, &args, tool.as_deref()),
        Some(Commands::Rename { name }) => cmd_rename(&tmux, &name),
        Some(Commands::Kill { name }) => cmd_kill(&tmux, &name),
        Some(Commands::Completions { shell }) => completions::generate(&shell),
        None => {
            if cli.args.is_empty() {
                cmd_control_plane(&tmux)
            } else {
                cmd_open(&tmux, &cli.args, cli.tool.as_deref())
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

    let tool = resolve_tool(tool_override, &config);

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
        let tool_cmd = tmux::tool_command(&tool, None, Some(&session));
        eprintln!("Creating {session} in {} ({})", dir.display(), tool);
        tmux.create_session(&session, &dir, &tool_cmd)?;
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
        let tool = resolve_tool(tool_override, &config);
        let dir = config.resolve_dir(name)?;
        let tool_cmd = tmux::tool_command(&tool, None, Some(&session));
        tmux.create_session(&session, &dir, &tool_cmd)?;
        eprintln!("Created {session} ({})", tool);
    } else {
        let names = config.all_names().join(", ");
        anyhow::bail!("Unknown vertical or remote: {name}\nKnown: {names}");
    }

    Ok(())
}

/// Rename the current tmux session.
fn cmd_rename(tmux: &Tmux, name: &str) -> Result<()> {
    tmux.rename_session(name)?;
    eprintln!("Renamed to {name}");
    Ok(())
}

/// Kill a session or all sessions.
fn cmd_kill(tmux: &Tmux, name: &str) -> Result<()> {
    if name == "all" {
        let sessions = tmux.list_sessions()?;
        for (sname, _) in &sessions {
            tmux.kill_session(sname)?;
            eprintln!("Killed {sname}");
        }
    } else if tmux.session_exists(name) {
        tmux.kill_session(name)?;
        eprintln!("Killed {name}");
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
        switcher::Action::None => Ok(()),
    }
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
