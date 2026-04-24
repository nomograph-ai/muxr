//! Harness operations: upgrade, status.
//!
//! Generic over HarnessConfig -- the same code handles claude, opencode, cursor.
//! All process management is local only (remote sessions do not participate).

use anyhow::{Context, Result};
use std::process::{Command, Stdio};

use crate::claude_status;
use crate::config::{Config, HarnessConfig};
use crate::state;
use crate::tmux::Tmux;

/// Upgrade all sessions running a harness to a new model.
///
/// For each session:
/// 1. Discover the session ID
/// 2. Send /exit to the harness (graceful exit)
/// 3. Wait for the shell prompt
/// 4. Send the new launch command with resume + model
pub fn upgrade(
    tmux: &Tmux,
    config: &Config,
    harness_name: &str,
    harness: &HarnessConfig,
    model: Option<&str>,
) -> Result<()> {
    let sessions = tmux.list_sessions()?;
    let mut upgraded = 0;
    let mut skipped = 0;

    for (name, _path) in &sessions {
        // Skip the muxr control plane
        if name == "muxr" {
            continue;
        }

        // Check if this session runs the right tool
        let vertical = name.split('/').next().unwrap_or(name);
        let tool = config.resolve_tool(vertical, None);
        if tool != harness_name {
            continue;
        }

        // Check if the harness process is actually running
        if !state::has_harness_process(tmux, name, &harness.bin) {
            eprintln!("  {name}: no {harness_name} process, skipping");
            skipped += 1;
            continue;
        }

        // Discover session ID before killing
        let session_id = state::discover_session_id(tmux, name, Some(harness));
        if session_id.is_none() {
            eprintln!("  {name}: could not discover session ID, skipping");
            skipped += 1;
            continue;
        }
        let session_id = session_id.unwrap();

        eprintln!("  {name}: upgrading (session {session_id})");

        // Find the harness PID for SIGTERM
        let shell_pid = tmux.pane_pid(name).ok().flatten();
        let harness_pid = shell_pid.and_then(|sp| {
            state::descendant_pids(sp)
                .into_iter()
                .find(|pid| is_harness_process(*pid, &harness.bin))
        });

        // Send /exit for graceful shutdown
        let target = Tmux::target(name);
        let _ = Command::new("tmux")
            .args(["send-keys", "-t", &target, "/exit", "Enter"])
            .status();

        // Wait for exit (up to 10s, then SIGKILL)
        if let Some(pid) = harness_pid {
            wait_for_exit(pid, 10);
        }

        // Wait for shell prompt (up to 5s)
        wait_for_prompt(tmux, name, 5);

        // Send new launch command
        let cmd = harness.launch_command(Some(name), Some(&session_id), model);
        let _ = Command::new("tmux")
            .args(["send-keys", "-t", &target, &cmd, "Enter"])
            .status();

        upgraded += 1;
    }

    eprintln!(
        "\nUpgraded {upgraded} session(s), skipped {skipped}."
    );
    if let Some(m) = model {
        eprintln!("Model: {m}");
    }

    Ok(())
}

/// Show harness status across all sessions.
pub fn status(
    tmux: &Tmux,
    config: &Config,
    harness_name: &str,
    _harness: &HarnessConfig,
) -> Result<()> {
    let sessions = tmux.list_sessions()?;

    eprintln!("{harness_name} sessions:\n");
    eprintln!(
        "  {:30} {:6} {:10}",
        "SESSION", "CTX %", "COST"
    );
    eprintln!("  {}", "-".repeat(50));

    let mut count = 0;
    for (name, _path) in &sessions {
        if name == "muxr" {
            continue;
        }
        let vertical = name.split('/').next().unwrap_or(name);
        let tool = config.resolve_tool(vertical, None);
        if tool != harness_name {
            continue;
        }

        let health = claude_status::read_health(name);
        let (ctx, cost) = match health {
            Some(h) => (
                format!("{}%", h.context_pct),
                format!("${:.2}", h.cost_usd),
            ),
            None => ("--".to_string(), "--".to_string()),
        };

        eprintln!("  {:30} {:>6} {:>10}", name, ctx, cost);
        count += 1;
    }

    if count == 0 {
        eprintln!("  (no active {harness_name} sessions)");
    }

    Ok(())
}

/// Switch model on all sessions by sending /model command (no restart).
pub fn model_switch(
    tmux: &Tmux,
    config: &Config,
    harness_name: &str,
    harness: &HarnessConfig,
    model: Option<&str>,
) -> Result<()> {
    let model = model.context("Usage: muxr {harness_name} model <model-name>")?;
    let cmd_template = harness
        .model_switch_command
        .as_ref()
        .context("Harness does not support live model switch")?;

    let cmd = crate::config::interpolate_raw(cmd_template, "model", model);
    let sessions = tmux.list_sessions()?;
    let mut switched = 0;

    for (name, _) in &sessions {
        if name == "muxr" {
            continue;
        }
        let vertical = name.split('/').next().unwrap_or(name);
        let tool = config.resolve_tool(vertical, None);
        if tool != harness_name {
            continue;
        }

        let target = Tmux::target(name);
        let _ = Command::new("tmux")
            .args(["send-keys", "-t", &target, &cmd, "Enter"])
            .status();
        eprintln!("  {name}: sent {cmd}");
        switched += 1;
    }

    eprintln!("\nSwitched {switched} session(s) to {model}.");
    Ok(())
}

/// Compact context on sessions over a threshold.
pub fn compact(
    tmux: &Tmux,
    config: &Config,
    harness_name: &str,
    harness: &HarnessConfig,
    threshold: Option<u32>,
) -> Result<()> {
    let threshold = threshold.unwrap_or(80);
    let cmd = harness
        .compact_command
        .as_ref()
        .context("Harness does not support compact")?;

    let sessions = tmux.list_sessions()?;
    let mut compacted = 0;

    for (name, _) in &sessions {
        if name == "muxr" {
            continue;
        }
        let vertical = name.split('/').next().unwrap_or(name);
        let tool = config.resolve_tool(vertical, None);
        if tool != harness_name {
            continue;
        }

        let health = crate::claude_status::read_health(name);
        let pct = health.as_ref().map(|h| h.context_pct).unwrap_or(0);

        if pct >= threshold {
            let target = Tmux::target(name);
            let _ = Command::new("tmux")
                .args(["send-keys", "-t", &target, cmd, "Enter"])
                .status();
            eprintln!("  {name}: {pct}% -> compacting");
            compacted += 1;
        } else {
            eprintln!("  {name}: {pct}% (under threshold)");
        }
    }

    eprintln!("\nCompacted {compacted} session(s) (threshold: {threshold}%).");
    Ok(())
}

/// Check if a PID is running a specific binary.
///
/// Matches against full argv -- node-based harnesses (claude-code) run
/// as `node /path/to/claude ...` where comm is `node`.
fn is_harness_process(pid: u32, bin: &str) -> bool {
    let suffix = format!("/{bin}");
    Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
        .map(|o| {
            let args_str = String::from_utf8_lossy(&o.stdout);
            args_str
                .split_whitespace()
                .any(|tok| tok == bin || tok.ends_with(&suffix))
        })
        .unwrap_or(false)
}

/// Wait for a process to exit, escalating to SIGKILL after timeout.
fn wait_for_exit(pid: u32, timeout_secs: u32) {
    for _ in 0..timeout_secs * 10 {
        // Check if still alive. Suppress stderr -- when pid is gone,
        // `kill -0` prints "No such process" which is not an error condition
        // for this polling check.
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if !alive {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // Still alive after timeout -- SIGKILL
    eprintln!("    process {pid} did not exit, sending SIGKILL");
    let _ = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
}

/// Wait for a shell prompt to appear in the pane.
fn wait_for_prompt(_tmux: &Tmux, session: &str, timeout_secs: u32) {
    let target = Tmux::target(session);
    for _ in 0..timeout_secs * 10 {
        if let Ok(output) = std::process::Command::new("tmux")
            .args(["capture-pane", "-p", "-t", &target])
            .output()
        {
            let content = String::from_utf8_lossy(&output.stdout);
            let last_line = content.lines().rev().find(|l| !l.is_empty()).unwrap_or("");
            // Common shell prompt endings
            if last_line.ends_with('$')
                || last_line.ends_with('%')
                || last_line.ends_with('#')
                || last_line.ends_with('>')
            {
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
