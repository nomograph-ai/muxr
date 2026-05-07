use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use sysinfo::{Pid, ProcessesToUpdate, System};

use crate::config::{Config, SessionDiscovery, Tool};
use crate::remote;
use crate::tmux::Tmux;

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
    /// If set, this is a remote proxy session (value is the remote name from config).
    #[serde(default)]
    pub remote: Option<String>,
}

/// List child PIDs of a given parent process.
///
/// Uses sysinfo (libproc on macOS, /proc on Linux) instead of shelling
/// out to `ps`. The previous `ps -A -o pid,ppid` approach had two
/// problems:
///   1. Sandboxes (e.g. nono's `dangerous_commands_macos` group) can
///      block /bin/ps, leaving every save with sessionId=null.
///   2. macOS pgrep's `-P` flag silently failed without a pattern arg.
///
/// Native API avoids both. sysinfo refreshes process metadata in-process,
/// so callers running inside a sandbox still see the full process tree
/// they own.
pub fn child_pids(parent: u32) -> Vec<u32> {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let parent_pid = Pid::from_u32(parent);
    sys.processes()
        .iter()
        .filter_map(|(pid, p)| {
            if p.parent() == Some(parent_pid) {
                Some(pid.as_u32())
            } else {
                None
            }
        })
        .collect()
}

/// Recursively collect all descendant PIDs of a process.
pub fn descendant_pids(parent: u32) -> Vec<u32> {
    let mut all = Vec::new();
    let children = child_pids(parent);
    for child in &children {
        all.push(*child);
        all.extend(descendant_pids(*child));
    }
    all
}

/// Read a session file for a PID using the harness discovery config.
fn read_session_id_from_file(pid: u32, pattern: &str, id_key: &str) -> Option<String> {
    let expanded = shellexpand::tilde(pattern).to_string();
    let path = expanded.replace("{pid}", &pid.to_string());

    let content = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    v.get(id_key)
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
}

/// Discover the harness session ID for a tmux session.
///
/// Walks the process tree recursively from the pane shell PID,
/// trying the harness's session discovery method for each descendant.
pub fn discover_session_id(
    tmux: &Tmux,
    tmux_session: &str,
    harness: Option<&Tool>,
) -> Option<String> {
    let harness = harness?;
    let (pattern, id_key) = match &harness.session_discovery {
        SessionDiscovery::File { pattern, id_key } => (pattern.as_str(), id_key.as_str()),
        SessionDiscovery::None => return None,
    };

    let shell_pid = tmux.pane_pid(tmux_session).ok()??;
    for pid in descendant_pids(shell_pid) {
        if let Some(id) = read_session_id_from_file(pid, pattern, id_key) {
            return Some(id);
        }
    }
    None
}

/// Check if a harness process is running in a tmux session.
///
/// Matches against the full argv (not just comm) because node-based
/// harnesses like claude-code run as `node /path/to/claude ...` -- the
/// executable's comm is `node`, but one of the args ends with `/claude`.
///
/// Uses sysinfo for process metadata so this stays sandbox-safe (no
/// /bin/ps shell-out).
#[allow(dead_code)] // used by harness.rs and switcher.rs
pub fn has_harness_process(tmux: &Tmux, tmux_session: &str, bin: &str) -> bool {
    let Some(Ok(Some(shell_pid))) = Some(tmux.pane_pid(tmux_session)) else {
        return false;
    };
    let suffix = format!("/{bin}");

    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);

    for pid in descendant_pids(shell_pid) {
        let Some(proc) = sys.process(Pid::from_u32(pid)) else {
            continue;
        };
        let cmd_matches = proc.cmd().iter().any(|arg| {
            let s = arg.to_string_lossy();
            s == bin || s.ends_with(&suffix)
        });
        if cmd_matches {
            return true;
        }
    }
    false
}

impl SavedState {
    /// Snapshot all current tmux sessions to the state file.
    pub fn save(config: &Config, tmux: &Tmux) -> Result<()> {
        let sessions = tmux.list_sessions()?;
        let mut saved = Vec::new();

        for (name, path) in sessions {
            // Skip the muxr control plane -- it's a bare shell for typing
            // muxr commands, not a work session. Recording it would cause
            // restore to relaunch claude inside it (tool defaults to the
            // config's default_tool), which is not what this pane is for.
            if name == "muxr" {
                continue;
            }

            let vertical = name.split('/').next().unwrap_or(&name);

            // Detect if this is a remote proxy session
            let remote = if config.is_remote(vertical) {
                Some(vertical.to_string())
            } else {
                None
            };

            let tool = config.resolve_tool(vertical, None);
            let harness = config.tool_for(&tool);
            let session_id = discover_session_id(tmux, &name, harness.as_ref());

            if let Some(ref id) = session_id {
                eprintln!("  {name}: {tool} session {id}");
            }
            if remote.is_some() {
                eprintln!("  {name}: remote proxy");
            }

            saved.push(SavedSession {
                name,
                dir: path,
                tool,
                session_id,
                remote,
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
    pub fn restore(tmux: &Tmux, config: &Config) -> Result<()> {
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
            // Defense: never restore the control plane as a tool session.
            if s.name == "muxr" {
                continue;
            }
            if tmux.session_exists(&s.name) {
                eprintln!("  {} -- already exists, skipping", s.name);
                continue;
            }

            if let Some(ref remote_name) = s.remote {
                // Remote proxy session -- reconnect via mosh/ssh
                let Some(remote) = config.remote(remote_name) else {
                    eprintln!(
                        "  {} -- remote '{}' not in config, skipping",
                        s.name, remote_name
                    );
                    continue;
                };

                let context = s.name.split('/').skip(1).collect::<Vec<_>>().join("/");
                let context = if context.is_empty() {
                    "default"
                } else {
                    &context
                };
                let instance = remote.instance_name(context);

                match remote::connect_command(remote, &instance, context) {
                    Ok(connect_cmd) => {
                        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
                        tmux.create_session(&s.name, &home, &connect_cmd)?;
                        eprintln!("  {} -> {} (remote)", s.name, instance);
                        count += 1;
                    }
                    Err(e) => {
                        eprintln!("  {} -- remote connect failed: {e}", s.name);
                    }
                }
            } else {
                // Local session
                let dir = PathBuf::from(&s.dir);
                if !dir.exists() {
                    eprintln!("  {} -- directory {} not found, skipping", s.name, s.dir);
                    continue;
                }

                let harness = config.tool_for(&s.tool);
                let tool_cmd = match &harness {
                    Some(h) => h.restore_command(Some(&s.name), s.session_id.as_deref()),
                    None => s.tool.clone(),
                };
                tmux.create_session(&s.name, &dir, &tool_cmd)?;
                eprintln!("  {} -> {}", s.name, s.dir);
                count += 1;
            }
        }

        eprintln!("Restored {count} sessions.");
        Ok(())
    }
}
