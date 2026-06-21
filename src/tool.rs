//! Harness operations: upgrade, model-switch.
//!
//! Generic over Tool -- the same code handles claude, opencode, cursor.
//! All process management is local only (remote sessions do not participate).

use anyhow::{Context, Result};
use std::process::{Command, Stdio};

use crate::config::{Config, Tool};
use crate::state;
use crate::tmux::Tmux;

/// Upgrade flags grouped to stay under the 7-arg clippy limit.
pub struct UpgradeOpts<'a> {
    pub model: Option<&'a str>,
    pub name_filter: Option<&'a str>,
    pub dry_run: bool,
    /// Bypass readiness gate and upgrade unconditionally.
    pub force: bool,
    /// Poll readiness for up to this many seconds before skipping.
    pub wait: Option<u64>,
    /// Minimum seconds of tmux inactivity to consider a session safe.
    pub min_idle: u64,
}

/// Upgrade running harness sessions onto the freshly resolved binary,
/// resuming each conversation in place.
///
/// For each matching session:
/// 1. Discover the session ID
/// 2. Compose the full relaunch command (prompt + add-dirs + resume)
/// 3. Send /exit to the harness (graceful exit), wait for it to quit
/// 4. Wait for the shell prompt, then send the relaunch command
///
/// `opts.name_filter` limits the run to a single session name; `None` upgrades
/// every session running `harness_name`. `opts.dry_run` composes and prints the
/// relaunch command for each target without touching the session.
pub fn upgrade(
    tmux: &Tmux,
    config: &Config,
    harness_name: &str,
    tool_def: &Tool,
    opts: UpgradeOpts<'_>,
) -> Result<()> {
    let UpgradeOpts {
        model,
        name_filter,
        dry_run,
        force,
        wait,
        min_idle,
    } = opts;
    let sessions = tmux.list_sessions()?;
    let mut upgraded = 0;
    let mut skipped = 0;

    for (name, _path) in &sessions {
        // Skip the muxr control plane
        if name == "muxr" {
            continue;
        }

        // Limit to a single named session when requested.
        if let Some(filter) = name_filter
            && name != filter
        {
            continue;
        }

        // Check if this session runs the right tool
        let harness = name.split('/').next().unwrap_or(name);
        let tool = config.resolve_tool(harness, None);
        if tool != harness_name {
            continue;
        }

        // Check if the harness process is actually running
        if !state::has_harness_process(tmux, name, &tool_def.bin) {
            eprintln!("  {name}: no {harness_name} process, skipping");
            skipped += 1;
            continue;
        }

        // Discover session ID before killing
        let session_id = state::discover_session_id(tmux, name, Some(tool_def));
        if session_id.is_none() {
            eprintln!("  {name}: could not discover session ID, skipping");
            skipped += 1;
            continue;
        }
        let session_id = session_id.unwrap();

        // Readiness gate: skip or wait unless --force.
        if !force {
            let readiness = state::session_readiness(tmux, name, tool_def, &session_id, min_idle);
            match readiness {
                state::Readiness::Safe => {}
                state::Readiness::Busy(ref reason) | state::Readiness::Unknown(ref reason) => {
                    if let Some(wait_secs) = wait {
                        // Poll until Safe or timeout.
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(wait_secs);
                        let mut current = readiness;
                        while !matches!(current, state::Readiness::Safe)
                            && std::time::Instant::now() < deadline
                        {
                            std::thread::sleep(std::time::Duration::from_secs(1));
                            current = state::session_readiness(
                                tmux,
                                name,
                                tool_def,
                                &session_id,
                                min_idle,
                            );
                        }
                        if !matches!(current, state::Readiness::Safe) {
                            eprintln!("  {name}: skipping — timed out waiting for readiness");
                            skipped += 1;
                            continue;
                        }
                    } else {
                        eprintln!("  {name}: skipping — {reason}");
                        skipped += 1;
                        continue;
                    }
                }
            }
        }

        // Compose the FULL relaunch up front (system prompt + campaign
        // add-dirs + resume) so the resumed session keeps its harness rules
        // and working directories on the freshly resolved binary, and so a
        // dry run surfaces compose errors before any session is touched. Fall
        // back to a bare name+resume relaunch if the campaign/session files
        // can't be composed (e.g. an archived session).
        let cmd = match crate::session::compose_launch_command(
            config,
            name,
            Some(&session_id),
            model,
            false,
        ) {
            Ok((cmd, _)) => cmd,
            Err(e) => {
                eprintln!("    full compose failed ({e}); relaunching name+resume only");
                tool_def.launch_command(Some(name), Some(&session_id), model)
            }
        };

        if dry_run {
            let readiness = state::session_readiness(tmux, name, tool_def, &session_id, min_idle);
            let verdict = match &readiness {
                state::Readiness::Safe => "SAFE".to_string(),
                state::Readiness::Busy(r) => format!("BUSY({r})"),
                state::Readiness::Unknown(r) => format!("UNKNOWN({r})"),
            };
            eprintln!("  {name}: would upgrade (session {session_id}) [{verdict}]");
            eprintln!("    -> {cmd}");
            upgraded += 1;
            continue;
        }

        eprintln!("  {name}: upgrading (session {session_id})");

        // Find the harness PID so we can wait for a clean exit.
        let shell_pid = tmux.pane_pid(name).ok().flatten();
        let harness_pid = shell_pid.and_then(|sp| {
            state::descendant_pids(sp)
                .into_iter()
                .find(|pid| state::pid_runs_bin(*pid, &tool_def.bin))
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

        let _ = Command::new("tmux")
            .args(["send-keys", "-t", &target, &cmd, "Enter"])
            .status();

        upgraded += 1;
    }

    if dry_run {
        eprintln!("\n{upgraded} session(s) would be upgraded, {skipped} skipped (dry run).");
    } else {
        eprintln!("\nUpgraded {upgraded} session(s), skipped {skipped}.");
    }
    if let Some(m) = model {
        eprintln!("Model: {m}");
    }

    Ok(())
}

/// Show harness status across all sessions.
/// Switch model on all sessions by sending /model command (no restart).
pub fn model_switch(
    tmux: &Tmux,
    config: &Config,
    harness_name: &str,
    tool_def: &Tool,
    model: Option<&str>,
) -> Result<()> {
    let model = model.context("Usage: muxr {harness_name} model <model-name>")?;
    let cmd_template = tool_def
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
        let harness = name.split('/').next().unwrap_or(name);
        let tool = config.resolve_tool(harness, None);
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

/// Wait for a process to exit, escalating to SIGKILL after timeout.
pub(crate) fn wait_for_exit(pid: u32, timeout_secs: u32) {
    for _ in 0..timeout_secs.saturating_mul(10) {
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
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
}

/// Wait for a shell prompt to appear in the pane.
fn wait_for_prompt(_tmux: &Tmux, session: &str, timeout_secs: u32) {
    let target = Tmux::target(session);
    for _ in 0..timeout_secs.saturating_mul(10) {
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
