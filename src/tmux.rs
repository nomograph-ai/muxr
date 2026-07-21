use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Settle delay between sending a prompt body and the Enter that submits it.
/// An agent TUI buffers a fast input burst as a paste and folds a same-burst
/// trailing Enter into the draft as a newline instead of submitting; a
/// distinct, later Enter submits cleanly. See `send_text`.
const SEND_TEXT_SUBMIT_SETTLE_MS: u64 = 250;

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
    /// `viewer`, when set, splits an extra pane running its command (focus
    /// stays on the tool pane). Recreated identically on restore. See ADR 0004.
    pub fn create_session(
        &self,
        name: &str,
        dir: &Path,
        tool_cmd: &str,
        env: &[(String, String)],
        viewer: Option<&crate::config::ResolvedViewer>,
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

        // Viewer pane: split an auxiliary pane running the configured command
        // (`-d` keeps focus on the tool pane). Runs on launch AND restore, so a
        // restored session comes back byte-identical. See ADR 0004.
        if let Some(c) = viewer {
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
                .context("Failed to split viewer pane")?;
            if !status.success() {
                anyhow::bail!("tmux split-window (viewer) failed for {name}");
            }
        }

        Ok(())
    }

    /// Send text to a session's pane and submit it with a separate Enter.
    ///
    /// Used to inject a prompt (reorient nudge, recycle flush, `/exit`) into a
    /// live agent pane. Two deliberate details, both learned from a stranded
    /// recycle where a long multi-line flush sat unsubmitted in the composer:
    ///   1. The body is sent with `-l` (literal) and NO trailing Enter, so text
    ///      is never parsed as tmux key names and a multi-line prompt lands as a
    ///      draft rather than partially submitting on an embedded newline.
    ///   2. Enter is a SEPARATE send-keys after a short settle. An agent TUI
    ///      (e.g. Claude Code) buffers a fast input burst as a paste and folds a
    ///      same-burst trailing Enter into the draft as a newline; a distinct,
    ///      later Enter submits cleanly.
    pub fn send_text(&self, name: &str, text: &str) -> Result<()> {
        self.send_text_target(&Self::target(name), text)
    }

    /// Like [`send_text`](Self::send_text) but to a pre-resolved tmux target
    /// (typically a specific `%pane_id`). The `=name:` form resolves to the
    /// session's ACTIVE pane, which need not be the tool pane; recycle pins its
    /// flush and `/exit` sends to the tool pane's id so a focused non-tool pane
    /// can never receive them (where a stray redirect could even forge the flush
    /// sentinel and drive a kill of a session the operator thought intact).
    pub(crate) fn send_text_target(&self, target: &str, text: &str) -> Result<()> {
        let body = self
            .command()
            .args(["send-keys", "-t", target, "-l", "--", text])
            .status()
            .context("Failed to send text to tmux target")?;
        if !body.success() {
            anyhow::bail!("tmux send-keys (text) failed for {target}");
        }
        // Let the TUI ingest the (possibly paste-buffered) input and close the
        // paste before we submit, so Enter is treated as submit, not newline.
        std::thread::sleep(std::time::Duration::from_millis(SEND_TEXT_SUBMIT_SETTLE_MS));
        let submit = self
            .command()
            .args(["send-keys", "-t", target, "Enter"])
            .status()
            .context("Failed to submit text to tmux target")?;
        if !submit.success() {
            anyhow::bail!("tmux send-keys (submit) failed for {target}");
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

    /// True if a tmux server is running (it responds to `list-sessions`).
    /// `list_sessions` maps BOTH "no server" and "server with zero sessions" to
    /// an empty list, so `muxr save` guards on this to avoid clobbering the saved
    /// state with an empty one after a reboot (before `restore`).
    pub fn server_running(&self) -> bool {
        self.command()
            .args(["list-sessions"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Session info with activity timestamp.
    pub fn list_sessions_detailed(&self) -> Result<Vec<SessionInfo>> {
        let output = self
            .command()
            .args([
                "list-sessions",
                "-F",
                "#{session_name}|#{session_path}|#{session_activity}|#{window_activity}",
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
                let parts: Vec<&str> = line.splitn(4, '|').collect();
                if parts.len() == 4 {
                    Some(SessionInfo {
                        name: parts[0].to_string(),
                        path: parts[1].to_string(),
                        activity: parts[2].parse().unwrap_or(0),
                        window_activity: parts[3].parse().unwrap_or(0),
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

    /// The foreground command of a tmux target, e.g. `claude`, `node`, `nono`, or
    /// a shell name (`zsh`/`bash`) once the tool has exited. The target is a
    /// `%pane_id` (returns that pane) or a session target (returns its first pane,
    /// index 0 -- the tool pane).
    ///
    /// muxr launches the tool via `send-keys` INTO a persistent shell, so the tool
    /// exiting returns the pane to its shell rather than firing a pane-exited event
    /// -- there is no pane-death signal to wait on. Polling this for a
    /// return-to-shell is how recycle detects the tool is gone (ADR 0008), robust
    /// to the pi `nono` wrapper (we watch for the shell coming back, not a specific
    /// tool name going away). Recycle passes a `%pane_id` so the poll reads the
    /// SAME pane it sends `/exit` to (see [`send_text_target`](Self::send_text_target)).
    /// `None` on tmux error / no pane.
    pub(crate) fn pane_command_at(&self, target: &str) -> Option<String> {
        let output = self
            .command()
            .args(["list-panes", "-t", target, "-F", "#{pane_current_command}"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .map(|l| l.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// The stable tmux pane id (`%N`) of a session's tool pane (index 0 -- muxr
    /// always launches the tool there; the viewer is a later `-d` split, so focus
    /// stays on pane 0). Recycle pins every pane op (shell check, flush send,
    /// `/exit`, return-to-shell poll) to this id so a focused non-tool pane can
    /// never diverge the read guards (which read pane 0) from the writes (which
    /// otherwise target the ACTIVE pane via `=name:`). `None` on tmux error / no
    /// pane.
    pub fn tool_pane_id(&self, session: &str) -> Option<String> {
        let output = self
            .command()
            .args(["list-panes", "-t", &Self::target(session), "-F", "#{pane_id}"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .map(|l| l.trim().to_string())
            .filter(|s| !s.is_empty())
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
    /// `#{session_activity}`: time of last CLIENT interaction with the session
    /// (keystroke / attach / switch). Used by the switcher for recency sort.
    /// NOT a signal of agent work -- it stays frozen while an unattended agent
    /// streams output.
    pub activity: u64,
    /// `#{window_activity}`: time of last PANE OUTPUT in the session's active
    /// window. This DOES advance while an agent is producing output (even
    /// detached/headless). DISPLAY-ONLY since 4.0.0 (ADR 0008 removed readiness
    /// inference): `upgrade`'s quiet-age column reports `now - window_activity`;
    /// it is never a gate.
    pub window_activity: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_wraps_with_equals_colon() {
        assert_eq!(Tmux::target("work/api"), "=work/api:");
        assert_eq!(Tmux::target("muxr"), "=muxr:");
    }

    // Behavioral: drives the real create_session against an ISOLATED tmux server
    // and asserts the viewer adds exactly one pane. Restore calls the SAME
    // create_session with viewer_for, so a faithful restore follows from this.
    // `#[ignore]`d because it spawns a tmux server; run: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn create_session_adds_viewer_pane() {
        use crate::config::ResolvedViewer;
        use std::process::Command as Pc;

        let srv = "muxr-viewer-selftest";
        let kill = || {
            let _ = Pc::new("tmux").args(["-L", srv, "kill-server"]).status();
        };
        kill(); // clean any leftover from a prior run

        let tmux = Tmux::new(Some(srv.to_string()));
        let dir = std::env::temp_dir();
        let viewer = ResolvedViewer {
            cmd: "sleep 600".to_string(),
            side: "h".to_string(),
            size: 40,
        };

        tmux.create_session("cp/with", &dir, "", &[], Some(&viewer))
            .expect("create session with viewer");
        tmux.create_session("cp/without", &dir, "", &[], None)
            .expect("create session without viewer");

        let panes = |name: &str| -> usize {
            let out = Pc::new("tmux")
                .args([
                    "-L",
                    srv,
                    "list-panes",
                    "-t",
                    &Tmux::target(name),
                    "-F",
                    "#{pane_id}",
                ])
                .output()
                .expect("tmux list-panes");
            String::from_utf8_lossy(&out.stdout).lines().count()
        };

        let with = panes("cp/with");
        let without = panes("cp/without");
        kill();

        assert_eq!(with, 2, "viewer session must have a second pane");
        assert_eq!(without, 1, "no-viewer session stays single-pane");
    }
}
