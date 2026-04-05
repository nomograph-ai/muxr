use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Tmux client with optional server isolation.
///
/// When `server` is set, all commands use `tmux -L <server>` to operate
/// on an isolated socket. This prevents demo recordings and tests from
/// colliding with production sessions.
pub struct Tmux {
    server: Option<String>,
}

impl Tmux {
    pub fn new(server: Option<String>) -> Self {
        Self { server }
    }

    /// Build a tmux Command with the server flag if set.
    fn command(&self) -> Command {
        let mut cmd = Command::new("tmux");
        if let Some(ref server) = self.server {
            cmd.args(["-L", server]);
        }
        cmd
    }

    /// Format a session name as a tmux target.
    /// Session names with `/` conflict with tmux's session/window target
    /// syntax. The trailing `:` tells tmux to treat the entire string as
    /// a session name targeting the current window.
    fn target(name: &str) -> String {
        format!("={name}:")
    }

    /// Check if a tmux session exists.
    pub fn session_exists(&self, name: &str) -> bool {
        self.command()
            .args(["has-session", "-t", &Self::target(name)])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Create a new tmux session (detached).
    /// If `tool_cmd` is empty, creates a bare shell session.
    pub fn create_session(&self, name: &str, dir: &Path, tool_cmd: &str) -> Result<()> {
        let dir_str = dir.to_str().context("Invalid directory path")?;

        let status = self
            .command()
            .args(["new-session", "-d", "-s", name, "-c", dir_str])
            .status()
            .context("Failed to create tmux session")?;

        if !status.success() {
            anyhow::bail!("tmux new-session failed for {name}");
        }

        if !tool_cmd.is_empty() {
            let status = self
                .command()
                .args(["send-keys", "-t", &Self::target(name), tool_cmd, "Enter"])
                .status()
                .context("Failed to send keys to tmux session")?;

            if !status.success() {
                anyhow::bail!("tmux send-keys failed for {name}");
            }
        }

        Ok(())
    }

    /// Attach to an existing tmux session.
    pub fn attach(&self, name: &str) -> Result<()> {
        if std::env::var("TMUX").is_ok() {
            let status = self
                .command()
                .args(["switch-client", "-t", &Self::target(name)])
                .status()
                .context("Failed to switch tmux client")?;
            if !status.success() {
                anyhow::bail!("tmux switch-client failed for {name}");
            }
        } else {
            let t = Self::target(name);
            if let Some(ref server) = self.server {
                let err =
                    exec::execvp("tmux", &["tmux", "-L", server, "attach", "-t", &t]);
                anyhow::bail!("Failed to exec tmux attach: {err}");
            } else {
                let err = exec::execvp("tmux", &["tmux", "attach", "-t", &t]);
                anyhow::bail!("Failed to exec tmux attach: {err}");
            }
        }
        Ok(())
    }

    /// List all tmux sessions as (name, path) pairs.
    pub fn list_sessions(&self) -> Result<Vec<(String, String)>> {
        let output = self
            .command()
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

    /// Session info with activity timestamp.
    pub fn list_sessions_detailed(&self) -> Result<Vec<SessionInfo>> {
        let output = self
            .command()
            .args([
                "list-sessions",
                "-F",
                "#{session_name}|#{session_path}|#{session_activity}",
            ])
            .output()
            .context("Failed to list tmux sessions")?;

        if !output.status.success() {
            return Ok(vec![]);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let sessions = stdout
            .lines()
            .filter_map(|line| {
                let parts: Vec<&str> = line.splitn(3, '|').collect();
                if parts.len() == 3 {
                    Some(SessionInfo {
                        name: parts[0].to_string(),
                        path: parts[1].to_string(),
                        activity: parts[2].parse().unwrap_or(0),
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(sessions)
    }

    /// Kill a tmux session.
    pub fn kill_session(&self, name: &str) -> Result<()> {
        let status = self
            .command()
            .args(["kill-session", "-t", &Self::target(name)])
            .status()
            .context("Failed to kill tmux session")?;
        if !status.success() {
            anyhow::bail!("tmux kill-session failed for {name}");
        }
        Ok(())
    }

    /// Get the PID of the first pane's process in a session.
    pub fn pane_pid(&self, session: &str) -> Result<Option<u32>> {
        let output = self
            .command()
            .args([
                "list-panes",
                "-t",
                &Self::target(session),
                "-F",
                "#{pane_pid}",
            ])
            .output()
            .context("Failed to get pane PID")?;

        if !output.status.success() {
            return Ok(None);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let pid = stdout.lines().next().and_then(|l| l.trim().parse().ok());
        Ok(pid)
    }

    /// Get a tmux variable via display-message.
    pub fn display_message(&self, fmt: &str) -> Result<String> {
        let output = self
            .command()
            .args(["display-message", "-p", fmt])
            .output()
            .context("Failed to display tmux message")?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Rename the current tmux session.
    pub fn rename_session(&self, name: &str) -> Result<()> {
        let status = self
            .command()
            .args(["rename-session", name])
            .status()
            .context("Failed to rename session")?;
        if !status.success() {
            anyhow::bail!("tmux rename-session failed");
        }
        Ok(())
    }
}

/// Session info with activity timestamp.
pub struct SessionInfo {
    pub name: String,
    pub path: String,
    pub activity: u64,
}

/// Build the tool launch command.
/// When session_name is set, passes --name for Claude session identification.
/// When resuming, passes the session ID shell-quoted to prevent word splitting.
pub fn tool_command(tool: &str, resume_id: Option<&str>, session_name: Option<&str>) -> String {
    if tool == "claude" {
        let mut cmd = "claude".to_string();
        if let Some(name) = session_name {
            cmd.push_str(&format!(" --name '{name}'"));
        }
        if let Some(id) = resume_id {
            cmd.push_str(&format!(" --resume '{id}'"));
        }
        cmd
    } else {
        tool.to_string()
    }
}
