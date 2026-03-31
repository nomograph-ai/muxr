mod completions;
mod config;
mod init;
mod state;
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
    let tool = resolve_tool(tool_override, &config);

    let vertical = &args[0];

    if !config.verticals.contains_key(vertical) {
        let names = config.vertical_names().join(", ");
        anyhow::bail!("Unknown vertical: {vertical}\nKnown verticals: {names}");
    }

    let session = if args.len() >= 2 {
        let context = args[1..].join("/");
        format!("{vertical}/{context}")
    } else {
        format!("{vertical}/default")
    };

    let dir = config.resolve_dir(vertical)?;

    if tmux::session_exists(&session) {
        eprintln!("Attaching to {session}");
        tmux::attach(&session)?;
    } else {
        let tool_cmd = tmux::tool_command(&tool, &session, None);
        eprintln!("Creating {session} in {} ({})", dir.display(), tool);
        tmux::create_session(&session, &dir, &tool_cmd)?;
        tmux::attach(&session)?;
    }

    Ok(())
}

/// Create a session in the background without attaching.
fn cmd_new(args: &[String], tool_override: Option<&str>) -> Result<()> {
    let config = Config::load()?;
    let tool = resolve_tool(tool_override, &config);
    let vertical = &args[0];

    if !config.verticals.contains_key(vertical) {
        let names = config.vertical_names().join(", ");
        anyhow::bail!("Unknown vertical: {vertical}\nKnown verticals: {names}");
    }

    let session = if args.len() >= 2 {
        let context = args[1..].join("/");
        format!("{vertical}/{context}")
    } else {
        format!("{vertical}/default")
    };

    let dir = config.resolve_dir(vertical)?;

    if tmux::session_exists(&session) {
        eprintln!("{session} already exists");
    } else {
        let tool_cmd = tmux::tool_command(&tool, &session, None);
        tmux::create_session(&session, &dir, &tool_cmd)?;
        eprintln!("Created {session} ({})", tool);
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
    let sessions = tmux::list_sessions()?;
    if sessions.is_empty() {
        eprintln!("No active tmux sessions.");
    } else {
        for (name, path) in &sessions {
            println!("  {name}  ({path})");
        }
    }
    Ok(())
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
