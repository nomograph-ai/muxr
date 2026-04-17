use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
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

// -- Git worktree management --

/// Check if a directory is a git repository.
pub fn is_git_repo(dir: &Path) -> bool {
    Command::new("git")
        .args(["-C", dir.to_str().unwrap_or("."), "rev-parse", "--git-dir"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Derive the worktree directory path for a session context.
/// Places worktrees in a sibling directory: `<repo>-worktrees/<context>`
pub fn worktree_path(repo_dir: &Path, context: &str) -> PathBuf {
    let repo_name = repo_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");
    let slug = context.replace('/', "-");
    repo_dir
        .parent()
        .unwrap_or(repo_dir)
        .join(format!("{repo_name}-worktrees"))
        .join(slug)
}

/// Create a git worktree for a session. Returns the worktree path.
/// Creates branch `muxr/<context>` from HEAD.
pub fn create_worktree(repo_dir: &Path, context: &str) -> Result<PathBuf> {
    let wt_path = worktree_path(repo_dir, context);
    let branch = format!("muxr/{}", context.replace('/', "-"));

    // If worktree already exists, just return the path
    if wt_path.exists() {
        return Ok(wt_path);
    }

    // Ensure parent directory exists
    if let Some(parent) = wt_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let status = Command::new("git")
        .args([
            "-C",
            repo_dir.to_str().context("Invalid repo path")?,
            "worktree",
            "add",
            wt_path.to_str().context("Invalid worktree path")?,
            "-B",
            &branch,
        ])
        .status()
        .context("Failed to run git worktree add")?;

    if !status.success() {
        anyhow::bail!("git worktree add failed for {}", wt_path.display());
    }

    Ok(wt_path)
}

/// Remove a git worktree and optionally delete its branch.
pub fn remove_worktree(repo_dir: &Path, context: &str) -> Result<()> {
    let wt_path = worktree_path(repo_dir, context);

    if !wt_path.exists() {
        return Ok(());
    }

    let _ = Command::new("git")
        .args([
            "-C",
            repo_dir.to_str().unwrap_or("."),
            "worktree",
            "remove",
            wt_path.to_str().unwrap_or(""),
            "--force",
        ])
        .status();

    // Clean up the branch
    let branch = format!("muxr/{}", context.replace('/', "-"));
    let _ = Command::new("git")
        .args([
            "-C",
            repo_dir.to_str().unwrap_or("."),
            "branch",
            "-D",
            &branch,
        ])
        .status();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_wraps_with_equals_colon() {
        assert_eq!(Tmux::target("work/api"), "=work/api:");
        assert_eq!(Tmux::target("muxr"), "=muxr:");
    }

    #[test]
    fn worktree_path_simple() {
        let repo = std::path::Path::new("/home/user/projects/den");
        let wt = worktree_path(repo, "opus");
        assert_eq!(wt, std::path::PathBuf::from("/home/user/projects/den-worktrees/opus"));
    }

    #[test]
    fn worktree_path_nested_context() {
        let repo = std::path::Path::new("/home/user/projects/den");
        let wt = worktree_path(repo, "api/auth");
        assert_eq!(wt, std::path::PathBuf::from("/home/user/projects/den-worktrees/api-auth"));
    }
}
