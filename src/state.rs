use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

use crate::config::Config;
use crate::tmux;

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedState {
    pub sessions: Vec<SavedSession>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedSession {
    pub name: String,
    pub dir: String,
    pub tool: String,
    #[serde(default, alias = "opencode_session")]
    pub session_id: Option<String>,
}

/// List child PIDs of a given parent process.
fn child_pids(parent: u32) -> Vec<u32> {
    let output = Command::new("pgrep")
        .args(["-P", &parent.to_string()])
        .output()
        .ok();

    match output {
        Some(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect(),
        _ => vec![],
    }
}

/// Read a Claude session file and extract the sessionId.
fn read_claude_session_id(pid: u32) -> Option<String> {
    let home = dirs::home_dir()?;
    let path = home
        .join(".claude")
        .join("sessions")
        .join(format!("{pid}.json"));

    let content = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    v.get("sessionId")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
}

/// Discover the active Claude session ID for a tmux session.
///
/// Chain: tmux pane PID (shell) -> child PIDs -> find one with a Claude session file -> sessionId
fn discover_session_id(tmux_session: &str) -> Option<String> {
    let shell_pid = tmux::pane_pid(tmux_session).ok()??;
    for pid in child_pids(shell_pid) {
        if let Some(id) = read_claude_session_id(pid) {
            return Some(id);
        }
    }
    None
}

impl SavedState {
    /// Snapshot all current tmux sessions to the state file.
    pub fn save(config: &Config) -> Result<()> {
        let sessions = tmux::list_sessions()?;
        let mut saved = Vec::new();

        for (name, path) in sessions {
            let tool = config.default_tool.clone();
            let session_id = discover_session_id(&name);

            if let Some(ref id) = session_id {
                eprintln!("  {name}: claude session {id}");
            }

            saved.push(SavedSession {
                name,
                dir: path,
                tool,
                session_id,
            });
        }

        let state = SavedState { sessions: saved };
        let path = Config::state_path()?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(&state)?;
        std::fs::write(&path, &json)
            .with_context(|| format!("Failed to write state to {}", path.display()))?;

        eprintln!(
            "Saved {} sessions to {}",
            state.sessions.len(),
            path.display()
        );
        for s in &state.sessions {
            eprintln!("  {}  ->  {}", s.name, s.dir);
        }

        Ok(())
    }

    /// Restore tmux sessions from the state file.
    pub fn restore() -> Result<()> {
        let path = Config::state_path()?;
        if !path.exists() {
            anyhow::bail!(
                "No state file found at {}\nRun `muxr save` before rebooting.",
                path.display()
            );
        }

        let content = std::fs::read_to_string(&path)?;
        let state: SavedState = serde_json::from_str(&content)?;

        let mut count = 0;
        for s in &state.sessions {
            if tmux::session_exists(&s.name) {
                eprintln!("  {} -- already exists, skipping", s.name);
                continue;
            }

            let dir = PathBuf::from(&s.dir);
            if !dir.exists() {
                eprintln!("  {} -- directory {} not found, skipping", s.name, s.dir);
                continue;
            }

            let tool_cmd = tmux::tool_command(&s.tool, s.session_id.as_deref());

            tmux::create_session(&s.name, &dir, &tool_cmd)?;
            eprintln!("  {} -> {}", s.name, s.dir);
            count += 1;
        }

        eprintln!("Restored {count} sessions.");
        Ok(())
    }
}
