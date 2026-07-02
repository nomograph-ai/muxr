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

    /// Send keys (followed by Enter) to a session target. Honors `-L` so it
    /// works on an isolated tmux server; a bare `tmux` would hit the wrong one.
    pub fn send_keys(&self, target: &str, keys: &str) {
        let _ = self
            .command()
            .args(["send-keys", "-t", target, keys, "Enter"])
            .status();
    }

    /// Capture the visible pane content for a session target (honors `-L`).
    pub fn capture_pane(&self, target: &str) -> Option<String> {
        let output = self
            .command()
            .args(["capture-pane", "-p", "-t", target])
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Last-activity epoch for a single session (None if not found / tmux error).
    /// For a fresh per-poll read; for a sweep, fetch `list_sessions_detailed`
    /// once and index it instead.
    pub fn session_activity(&self, name: &str) -> Option<u64> {
        self.list_sessions_detailed()
            .ok()?
            .into_iter()
            .find(|s| s.name == name)
            .map(|s| s.activity)
    }

    /// Format a session name as a tmux target.
    /// Session names with `/` conflict with tmux's session/window target
    /// syntax. The trailing `:` tells tmux to treat the entire string as
    /// a session name targeting the current window.
    pub fn target(name: &str) -> String {
        format!("={name}:")
    }

    /// Get the name of the current tmux session (if running inside tmux).
    pub fn current_session(&self) -> Option<String> {
        let output = self
            .command()
            .args(["display-message", "-p", "#{session_name}"])
            .output()
            .ok()?;
        if output.status.success() {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if name.is_empty() { None } else { Some(name) }
        } else {
            None
        }
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
    /// `env` sets session-scoped variables via `new-session -e KEY=VALUE`
    /// (tmux 3.2+); pass `&[]` for none.
    /// `companion`, when set, splits an extra pane running its command (focus
    /// stays on the tool pane). Recreated identically on restore. See ADR 0004.
    pub fn create_session(
        &self,
        name: &str,
        dir: &Path,
        tool_cmd: &str,
        env: &[(String, String)],
        companion: Option<&crate::config::ResolvedCompanion>,
    ) -> Result<()> {
        let dir_str = dir.to_str().context("Invalid directory path")?;

        let mut new_session = self.command();
        new_session.args(["new-session", "-d", "-s", name, "-c", dir_str]);
        for (k, v) in env {
            new_session.arg("-e").arg(format!("{k}={v}"));
        }
        let status = new_session
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

        // Companion pane: split an auxiliary pane running the configured command
        // (`-d` keeps focus on the tool pane). Runs on launch AND restore, so a
        // restored session comes back byte-identical. See ADR 0004.
        if let Some(c) = companion {
            let flag = if c.side == "v" { "-v" } else { "-h" };
            let size = format!("{}%", c.size);
            let status = self
                .command()
                .args([
                    "split-window",
                    flag,
                    "-d",
                    "-l",
                    &size,
                    "-t",
                    &Self::target(name),
                    "-c",
                    dir_str,
                    &c.cmd,
                ])
                .status()
                .context("Failed to split companion pane")?;
            if !status.success() {
                anyhow::bail!("tmux split-window (companion) failed for {name}");
            }
        }

        Ok(())
    }

    /// Send a single line of text to a session's pane, then Enter. Used to
    /// inject a prompt (e.g. a reorient nudge) into a live harness session.
    pub fn send_text(&self, name: &str, text: &str) -> Result<()> {
        let status = self
            .command()
            .args(["send-keys", "-t", &Self::target(name), text, "Enter"])
            .status()
            .context("Failed to send keys to tmux session")?;
        if !status.success() {
            anyhow::bail!("tmux send-keys failed for {name}");
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
                let err = exec::execvp("tmux", &["tmux", "-L", server, "attach", "-t", &t]);
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

    /// Rename a tmux session. If `old` is None, renames the current session.
    pub fn rename_session(&self, old: Option<&str>, new: &str) -> Result<()> {
        let mut cmd = self.command();
        cmd.arg("rename-session");
        let target;
        if let Some(o) = old {
            target = Self::target(o);
            cmd.args(["-t", &target]);
        }
        cmd.arg(new);
        let status = cmd.status().context("Failed to rename session")?;
        if !status.success() {
            anyhow::bail!("tmux rename-session failed");
        }
        Ok(())
    }
}

/// Session info with activity timestamp.
pub struct SessionInfo {
    pub name: String,
    #[allow(dead_code)]
    pub path: String,
    pub activity: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_wraps_with_equals_colon() {
        assert_eq!(Tmux::target("work/api"), "=work/api:");
        assert_eq!(Tmux::target("muxr"), "=muxr:");
    }
}
