use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    pub opencode_session: Option<String>,
}

impl SavedState {
    /// Snapshot all current tmux sessions to the state file.
    pub fn save(config: &Config) -> Result<()> {
        let sessions = tmux::list_sessions()?;
        let mut saved = Vec::new();

        for (name, path) in sessions {
            // Determine tool -- for now default to config default
            let tool = config.default_tool.clone();

            saved.push(SavedSession {
                name,
                dir: path,
                tool,
                opencode_session: None, // TODO: extract from opencode session list
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

            // Build the tool command, optionally with --session
            let tool_cmd = match &s.opencode_session {
                Some(id) if !id.is_empty() => format!("{} --session {}", s.tool, id),
                _ => s.tool.clone(),
            };

            tmux::create_session(&s.name, &dir, &tool_cmd)?;
            eprintln!("  {} -> {}", s.name, s.dir);
            count += 1;
        }

        eprintln!("Restored {count} sessions.");
        Ok(())
    }
}
