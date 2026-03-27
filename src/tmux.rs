use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Format a session name as a tmux target.
/// Session names with `/` (e.g., "work/api") conflict with tmux's
/// session/window target syntax. The trailing `:` tells tmux to treat
/// the entire string as a session name targeting the current window.
fn target(name: &str) -> String {
    format!("={name}:")
}

/// Check if a tmux session exists.
pub fn session_exists(name: &str) -> bool {
    Command::new("tmux")
        .args(["has-session", "-t", &target(name)])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Create a new tmux session (detached).
pub fn create_session(name: &str, dir: &Path, tool: &str) -> Result<()> {
    let dir_str = dir.to_str().context("Invalid directory path")?;

    let status = Command::new("tmux")
        .args(["new-session", "-d", "-s", name, "-c", dir_str])
        .status()
        .context("Failed to create tmux session")?;

    if !status.success() {
        anyhow::bail!("tmux new-session failed for {name}");
    }

    // Send the tool command
    let status = Command::new("tmux")
        .args(["send-keys", "-t", &target(name), tool, "Enter"])
        .status()
        .context("Failed to send keys to tmux session")?;

    if !status.success() {
        anyhow::bail!("tmux send-keys failed for {name}");
    }

    Ok(())
}

/// Attach to an existing tmux session.
pub fn attach(name: &str) -> Result<()> {
    // If we're inside tmux, switch client. Otherwise, attach.
    if std::env::var("TMUX").is_ok() {
        let status = Command::new("tmux")
            .args(["switch-client", "-t", &target(name)])
            .status()
            .context("Failed to switch tmux client")?;
        if !status.success() {
            anyhow::bail!("tmux switch-client failed for {name}");
        }
    } else {
        let t = target(name);
        let err = exec::execvp("tmux", &["tmux", "attach", "-t", &t]);
        anyhow::bail!("Failed to exec tmux attach: {err}");
    }
    Ok(())
}

/// List all tmux sessions as (name, path) pairs.
pub fn list_sessions() -> Result<Vec<(String, String)>> {
    let output = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}|#{session_path}"])
        .output()
        .context("Failed to list tmux sessions")?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let sessions = stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(2, '|').collect();
            if parts.len() == 2 {
                Some((parts[0].to_string(), parts[1].to_string()))
            } else {
                None
            }
        })
        .collect();

    Ok(sessions)
}

/// Kill a tmux session.
pub fn kill_session(name: &str) -> Result<()> {
    let status = Command::new("tmux")
        .args(["kill-session", "-t", &target(name)])
        .status()
        .context("Failed to kill tmux session")?;
    if !status.success() {
        anyhow::bail!("tmux kill-session failed for {name}");
    }
    Ok(())
}

/// Check if tmux server is running.
pub fn _server_running() -> bool {
    Command::new("tmux")
        .args(["list-sessions"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
