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

    // Fetch tmux activity ONCE for the whole sweep (the gate's floor uses it);
    // the --wait poll re-reads a single session live. Avoids an O(n^2) of
    // list-sessions calls across the loop.
    let activity_map: std::collections::HashMap<String, u64> = tmux
        .list_sessions_detailed()
        .unwrap_or_default()
        .into_iter()
        .map(|s| (s.name, s.activity))
        .collect();

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

        // Self-upgrade guard: never send the exit command to the session we are
        // invoked from -- it would kill the in-flight `upgrade` run.
        if tmux.current_session().as_deref() == Some(name.as_str()) {
            eprintln!("  {name}: skipping — this is the session running `upgrade`");
            skipped += 1;
            continue;
        }

        // Compute readiness ONCE (--force skips the probe entirely). The floor
        // uses the pre-fetched activity (no per-session list-sessions call).
        let readiness = if force {
            None
        } else {
            Some(state::session_readiness(
                tmux,
                name,
                tool_def,
                &session_id,
                min_idle,
                activity_map.get(name).copied(),
            ))
        };
        let verdict = match &readiness {
            None => "FORCED".to_string(),
            Some(state::Readiness::Safe) => "SAFE".to_string(),
            Some(state::Readiness::Busy(r)) => format!("BUSY({r})"),
            Some(state::Readiness::Unknown(r)) => format!("UNKNOWN({r})"),
        };
        let is_safe = matches!(readiness, None | Some(state::Readiness::Safe));

        // Compose the FULL relaunch (system prompt + campaign add-dirs + resume),
        // falling back to a bare name+resume if the campaign/session files can't
        // be composed. Deferred behind a closure: in a live run we compose AFTER
        // sending the exit (off the readiness->exit TOCTOU path), and we skip
        // composing entirely for a dry-run that would skip.
        let compose = || match crate::session::compose_launch_command(
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
            // Never poll/sleep in a dry run; report the live verdict + decision.
            if is_safe {
                eprintln!("  {name}: would upgrade (session {session_id}) [{verdict}]");
                eprintln!("    -> {}", compose());
                upgraded += 1;
            } else {
                eprintln!("  {name}: would skip [{verdict}]");
                skipped += 1;
            }
            continue;
        }

        // Live readiness gate: wait or skip unless already safe (or forced).
        // The --wait poll re-reads this session's activity live each tick.
        if !is_safe {
            if let Some(wait_secs) = wait {
                let deadline =
                    std::time::Instant::now() + std::time::Duration::from_secs(wait_secs);
                let mut current = state::session_readiness(
                    tmux,
                    name,
                    tool_def,
                    &session_id,
                    min_idle,
                    tmux.session_activity(name),
                );
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
                        tmux.session_activity(name),
                    );
                }
                if !matches!(current, state::Readiness::Safe) {
                    eprintln!("  {name}: skipping — timed out waiting for readiness [{verdict}]");
                    skipped += 1;
                    continue;
                }
            } else {
                eprintln!("  {name}: skipping — {verdict}");
                skipped += 1;
                continue;
            }
        }

        // CONFIRMED SAFE. Find the harness pid, then send the exit IMMEDIATELY --
        // before the slower compose -- to shrink the window between the readiness
        // check and the relaunch trigger (TOCTOU).
        eprintln!("  {name}: upgrading (session {session_id})");
        let shell_pid = tmux.pane_pid(name).ok().flatten();
        let harness_pid = shell_pid.and_then(|sp| {
            state::descendant_pids(sp)
                .into_iter()
                .find(|pid| state::pid_runs_bin(*pid, &tool_def.bin))
        });
        if shell_pid.is_some() && harness_pid.is_none() {
            eprintln!(
                "  {name}: no live {} process found; may have already exited",
                tool_def.bin
            );
        }

        // Graceful-exit command (runtime-specific: Pi = /quit), sent at once.
        let exit_cmd = tool_def.exit_command.as_deref().unwrap_or("/exit");
        let target = Tmux::target(name);
        tmux.send_keys(&target, exit_cmd);

        // Compose the relaunch WHILE the session is exiting (off the TOCTOU path).
        let cmd = compose();

        // Wait for exit (up to 10s, then SIGKILL), then the shell prompt.
        if let Some(pid) = harness_pid {
            wait_for_exit(pid, 10);
        }
        wait_for_prompt(tmux, name, 5);

        tmux.send_keys(&target, &cmd);

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
        tmux.send_keys(&target, &cmd);
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
fn wait_for_prompt(tmux: &Tmux, session: &str, timeout_secs: u32) {
    let target = Tmux::target(session);
    for _ in 0..timeout_secs.saturating_mul(10) {
        if let Some(content) = tmux.capture_pane(&target) {
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
