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
    /// Skip the interactive confirmation and upgrade every matched session.
    pub force: bool,
}

/// Upgrade running harness sessions onto the freshly resolved binary, resuming
/// each conversation in place.
///
/// For each matching session: discover the session id, compose the full
/// relaunch (prompt + add-dirs + resume), send `/exit`, wait for it to quit,
/// then send the relaunch at the shell prompt.
///
/// muxr does NOT infer whether a session is "safe" to relaunch -- readiness
/// inference was removed in 4.0.0 (ADR 0008) because the signal is unobservable
/// from outside the runtime. Instead muxr LISTS the sessions it will relaunch
/// with a display-only quiet-age column (seconds since the pane last emitted
/// output) and the human confirms. `--force` skips the confirmation
/// (scripting/CI); `--dry-run` prints the listing + composed relaunch and
/// touches nothing. `name_filter` limits the run to a single session.
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
    } = opts;
    let sessions = tmux.list_sessions()?;

    // Pane-output activity for the display-only quiet-age column: window_activity
    // (pane output), NOT session_activity (client interaction, which is frozen
    // for an unattended working session). One tmux round-trip, reused per row.
    let now = now_epoch();
    let activity_map: std::collections::HashMap<String, u64> = tmux
        .list_sessions_detailed()
        .unwrap_or_default()
        .into_iter()
        .map(|s| (s.name, s.window_activity))
        .collect();

    // One planned upgrade: the session, its discovered id, the composed relaunch
    // command, and the display-only quiet-age.
    struct Planned {
        name: String,
        session_id: String,
        cmd: String,
        quiet_age: Option<u64>,
    }
    let mut planned: Vec<Planned> = Vec::new();
    let mut skipped = 0;

    for (name, _path) in &sessions {
        if name == "muxr" {
            continue;
        }
        if let Some(filter) = name_filter
            && name != filter
        {
            continue;
        }
        // Only sessions running the requested tool.
        let harness = name.split('/').next().unwrap_or(name);
        if config.resolve_tool(harness, None) != harness_name {
            continue;
        }
        if !state::has_harness_process(tmux, name, &tool_def.bin) {
            eprintln!("  {name}: no {harness_name} process, skipping");
            skipped += 1;
            continue;
        }
        let Some(session_id) = state::discover_session_id(tmux, name, Some(tool_def)) else {
            eprintln!("  {name}: could not discover session ID, skipping");
            skipped += 1;
            continue;
        };
        // Self-upgrade guard: never relaunch the session running `upgrade`.
        if tmux.current_session().as_deref() == Some(name.as_str()) {
            eprintln!("  {name}: skipping -- this is the session running `upgrade`");
            skipped += 1;
            continue;
        }
        // Compose the FULL relaunch up front, BEFORE any exit. A missing
        // campaign/log degrades cleanly inside compose (an archived-but-running
        // session keeps its repo prompt + resume); a present-but-unparseable one
        // returns Err and we SKIP loud rather than exit + relaunch it stripped of
        // its rules (#11). Composing before the exit also means a bad compose can
        // never destroy a session.
        let cmd = match crate::session::compose_launch_command(
            config,
            name,
            Some(&session_id),
            model,
            false,
        ) {
            Ok((cmd, _)) => cmd,
            Err(e) => {
                eprintln!("  {name}: skipping -- compose failed: {e:#}");
                skipped += 1;
                continue;
            }
        };
        let quiet_age = activity_map.get(name).map(|a| now.saturating_sub(*a));
        planned.push(Planned {
            name: name.clone(),
            session_id,
            cmd,
            quiet_age,
        });
    }

    if planned.is_empty() {
        eprintln!("No sessions to upgrade ({skipped} skipped).");
        return Ok(());
    }

    // Listing with the display-only quiet-age column -- the cue the human uses to
    // decide. muxr does not judge readiness for them.
    eprintln!("Sessions to upgrade:");
    for p in &planned {
        let age = p
            .quiet_age
            .map(crate::fmt_age)
            .unwrap_or_else(|| "?".to_string());
        eprintln!("  {}  (session {})  quiet {age}", p.name, p.session_id);
        if dry_run {
            eprintln!("    -> {}", p.cmd);
        }
    }

    if dry_run {
        eprintln!(
            "\n{} session(s) would be upgraded, {skipped} skipped (dry run).",
            planned.len()
        );
        if let Some(m) = model {
            eprintln!("Model: {m}");
        }
        return Ok(());
    }

    // Human checkpoint (unless --force). Ambiguous / non-tty input aborts.
    if !force && !confirm(&format!("Upgrade {} session(s)?", planned.len()))? {
        eprintln!("Aborted; no sessions touched.");
        return Ok(());
    }

    let mut upgraded = 0;
    for p in &planned {
        let name = &p.name;
        eprintln!("  {name}: upgrading (session {})", p.session_id);
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
        // Graceful-exit command (runtime-specific: Pi = /quit).
        let exit_cmd = tool_def.exit_command.as_deref().unwrap_or("/exit");
        let target = Tmux::target(name);
        tmux.send_keys(&target, exit_cmd);
        // Wait for exit (up to 10s, then SIGKILL), then the shell prompt.
        if let Some(pid) = harness_pid {
            wait_for_exit(pid, 10);
        }
        wait_for_prompt(tmux, name, 5);
        tmux.send_keys(&target, &p.cmd);
        upgraded += 1;
    }

    eprintln!("\nUpgraded {upgraded} session(s), skipped {skipped}.");
    if let Some(m) = model {
        eprintln!("Model: {m}");
    }
    Ok(())
}

/// Epoch seconds now (0 on the impossible platform error).
fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Prompt on stderr and read yes/no from stdin. Returns true only on an explicit
/// `y`/`yes`. A non-tty / EOF / read error returns false, so a non-interactive
/// `upgrade` without `--force` aborts safely instead of relaunching everything.
fn confirm(question: &str) -> Result<bool> {
    use std::io::Write;
    eprint!("{question} [y/N] ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => Ok(false),
        Ok(_) => {
            let a = line.trim().to_ascii_lowercase();
            Ok(a == "y" || a == "yes")
        }
    }
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

/// Interactive shells a pane returns to once its foreground tool exits.
const SHELLS: &[&str] = &["zsh", "bash", "sh", "fish", "dash", "ksh"];

/// Poll the tool pane's foreground command until it is a known shell (the tool
/// exited and control returned to the launch shell) or `timeout_secs` elapses.
/// Returns true if the pane returned to a shell.
///
/// muxr launches the tool via `send-keys` INTO a persistent shell, so the tool
/// exiting returns the pane to that shell rather than closing it -- there is no
/// pane-death event to wait on (ADR 0008). This is more robust than matching the
/// tool PID by process-tree pattern (the pre-4.0 approach, fragile across
/// platforms + the pi `nono` wrapper): we watch for the shell coming BACK, not
/// for a specific tool name going away.
pub(crate) fn wait_for_return_to_shell(tmux: &Tmux, session: &str, timeout_secs: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        if let Some(cmd) = tmux.pane_current_command(session)
            && SHELLS.contains(&cmd.as_str())
        {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// Poll for `path` to exist -- the agent's POSITIVE flush-done signal on recycle
/// -- until it appears (true) or `timeout_secs` elapses (false).
///
/// This is the ADR 0008/0010 core: muxr never INFERS that a flush finished from
/// idle bytes (the pre-4.0 churn); it sends a flush prompt asking the agent to
/// write this file when done and waits for the file. A timeout means the flush
/// did not complete -- the caller aborts WITHOUT exiting the session (fail-safe:
/// no signal, no destructive action).
pub(crate) fn wait_for_sentinel(path: &std::path::Path, timeout_secs: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
    loop {
        if path.exists() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
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
