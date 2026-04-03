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

    match cli.command {
        Some(Commands::Init) => init::init(),
        Some(Commands::Ls) => cmd_ls(),
        Some(Commands::Save) => {
            let config = Config::load()?;
            state::SavedState::save(&config)
        }
        Some(Commands::Restore) => state::SavedState::restore(),
        Some(Commands::TmuxStatus) => cmd_tmux_status(),
        Some(Commands::Switch) => cmd_switch(),
        Some(Commands::New { tool, args }) => cmd_new(&args, tool.as_deref()),
        Some(Commands::Rename { name }) => cmd_rename(&name),
        Some(Commands::Kill { name }) => cmd_kill(&name),
        Some(Commands::Completions { shell }) => completions::generate(&shell),
        None => {
            if cli.args.is_empty() {
                cmd_control_plane()
            } else {
                cmd_open(&cli.args, cli.tool.as_deref())
            }
        }
    }
}

/// Start or attach to the muxr control plane shell.
fn cmd_control_plane() -> Result<()> {
    let session = "muxr";
    let home = dirs::home_dir().context("Could not determine home directory")?;

    if tmux::session_exists(session) {
        tmux::attach(session)?;
    } else {
        // Create with a bare shell (no tool), just the home directory
        let status = std::process::Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-s",
                session,
                "-c",
                home.to_str().unwrap_or("~"),
            ])
            .status()
            .context("Failed to create muxr control plane")?;
        if !status.success() {
            anyhow::bail!("Failed to create muxr session");
        }
        tmux::attach(session)?;
    }

    Ok(())
}

/// Open or attach to a session: muxr work api auth
fn cmd_open(args: &[String], tool_override: Option<&str>) -> Result<()> {
    let config = Config::load()?;
    let name = &args[0];

    // Route to remote handler if this is a remote vertical
    if config.is_remote(name) {
        return cmd_open_remote(&config, name, args);
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

    if tmux::session_exists(&session) {
        eprintln!("Attaching to {session}");
        tmux::attach(&session)?;
    } else {
        let tool_cmd = tmux::tool_command(&tool, None);
        eprintln!("Creating {session} in {} ({})", dir.display(), tool);
        tmux::create_session(&session, &dir, &tool_cmd)?;
        tmux::attach(&session)?;
    }

    Ok(())
}

/// Open or attach to a remote proxy session: muxr lab bootc
fn cmd_open_remote(config: &Config, remote_name: &str, args: &[String]) -> Result<()> {
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

    if tmux::session_exists(&session) {
        eprintln!("Attaching to {session} (remote)");
        tmux::attach(&session)?;
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
        tmux::create_session(&session, &home, &connect_cmd)?;
        tmux::attach(&session)?;
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
fn cmd_new(args: &[String], tool_override: Option<&str>) -> Result<()> {
    let config = Config::load()?;
    let name = &args[0];

    let context = if args.len() >= 2 {
        args[1..].join("/")
    } else {
        "default".to_string()
    };

    let session = format!("{name}/{context}");

    if tmux::session_exists(&session) {
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
        tmux::create_session(&session, &home, &connect_cmd)?;
        eprintln!("Created {session} -> {instance} (remote)");
    } else if config.verticals.contains_key(name) {
        let tool = resolve_tool(tool_override, &config);
        let dir = config.resolve_dir(name)?;
        let tool_cmd = tmux::tool_command(&tool, None);
        tmux::create_session(&session, &dir, &tool_cmd)?;
        eprintln!("Created {session} ({})", tool);
    } else {
        let names = config.all_names().join(", ");
        anyhow::bail!("Unknown vertical or remote: {name}\nKnown: {names}");
    }

    Ok(())
}

/// Rename the current tmux session.
fn cmd_rename(name: &str) -> Result<()> {
    let status = std::process::Command::new("tmux")
        .args(["rename-session", name])
        .status()
        .context("Failed to rename session")?;
    if !status.success() {
        anyhow::bail!("tmux rename-session failed");
    }
    eprintln!("Renamed to {name}");
    Ok(())
}

/// Kill a session or all sessions.
fn cmd_kill(name: &str) -> Result<()> {
    if name == "all" {
        let sessions = tmux::list_sessions()?;
        for (sname, _) in &sessions {
            tmux::kill_session(sname)?;
            eprintln!("Killed {sname}");
        }
    } else if tmux::session_exists(name) {
        tmux::kill_session(name)?;
        eprintln!("Killed {name}");
    } else {
        eprintln!("Session not found: {name}");
    }
    Ok(())
}

fn cmd_ls() -> Result<()> {
    let config = Config::load().ok();
    let sessions = tmux::list_sessions()?;
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
fn cmd_switch() -> Result<()> {
    match switcher::run()? {
        switcher::Action::Switch(session) => tmux::attach(&session),
        switcher::Action::Kill(session) => {
            tmux::kill_session(&session)?;
            eprintln!("Killed {session}");
            // Re-enter the switcher after kill
            cmd_switch()
        }
        switcher::Action::None => Ok(()),
    }
}

/// Generate tmux status-left format string from config verticals.
/// Used by tmux.conf: set -g status-left "#(muxr tmux-status)"
fn cmd_tmux_status() -> Result<()> {
    // This is called by tmux to get the status-left string.
    // We read the current tmux session name and color it by vertical.
    let output = std::process::Command::new("tmux")
        .args(["display-message", "-p", "#{session_name}"])
        .output()?;

    let session_name = String::from_utf8_lossy(&output.stdout).trim().to_string();

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
