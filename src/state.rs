use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};
use sysinfo::{Pid, ProcessesToUpdate, System};

use crate::config::{Config, ReadinessProbe, SessionDiscovery, Tool};
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
        SessionDiscovery::File(d) => (d.pattern.as_str(), d.id_key.as_str()),
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

/// Readiness verdict for a session before an upgrade gate.
#[derive(Debug, PartialEq)]
pub enum Readiness {
    /// Safe to upgrade right now.
    Safe,
    /// Not safe; human-readable reason is the payload.
    Busy(String),
    /// Could not determine; treat as not-safe unless --force.
    Unknown(String),
}

/// Default quiet period (seconds) a session must be idle before it is
/// considered safe to relaunch. Shared by `muxr upgrade` and `muxr status`.
pub const DEFAULT_MIN_IDLE_SECS: u64 = 180;

/// Default threshold: a `busy` state file older than this is treated as stale
/// -- a likely crashed session that fired its busy hook but never wrote `idle`
/// (no `Stop`). Such a file would otherwise block the session's upgrade forever;
/// instead we fall through to the floor so tmux activity can resolve it. This is
/// the DEFAULT only; the effective threshold is configurable via
/// `[readiness].stale_busy_secs` and threaded in as `stale_busy_secs`.
pub const STALE_BUSY_SECS: u64 = 3600;

/// Max wall-clock a `Command` readiness probe may run before it is killed and
/// the result treated as Unknown (-> floor), so a hung probe can't block muxr.
pub const PROBE_TIMEOUT_SECS: u64 = 10;

/// Pure helper that classifies a state file without any tmux or process deps.
/// Path is tilde-expanded; `{session_id}` in `path` must already be
/// interpolated by the caller. Returns `Unknown` on any file/parse error.
///
/// `stale_busy_secs` is the stale-busy threshold (configurable via
/// `[readiness].stale_busy_secs`; defaults to [`STALE_BUSY_SECS`]): a `busy`
/// file older than this reads `Unknown` so the caller falls through to the floor.
pub fn classify_state_file(
    path: &str,
    state_key: &str,
    idle_value: &str,
    since_key: Option<&str>,
    now: u64,
    min_idle: u64,
    stale_busy_secs: u64,
) -> Readiness {
    let expanded = shellexpand::tilde(path).to_string();
    let content = match std::fs::read_to_string(&expanded) {
        Ok(c) => c,
        Err(e) => return Readiness::Unknown(format!("cannot read {expanded}: {e}")),
    };
    let v: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => return Readiness::Unknown(format!("parse error in {expanded}: {e}")),
    };
    let state = match v.get(state_key).and_then(|s| s.as_str()) {
        Some(s) => s.to_string(),
        None => {
            return Readiness::Unknown(format!(
                "key {state_key:?} missing or not a string in {expanded}"
            ));
        }
    };
    let since = since_key.and_then(|sk| v.get(sk).and_then(|s| s.as_u64()));
    if state != idle_value {
        // Stale-busy guard: a `busy` file older than `stale_busy_secs` is almost
        // certainly a crashed session that never wrote `idle`. Return Unknown so
        // the caller falls through to the floor instead of blocking forever.
        if let Some(s) = since
            && now.saturating_sub(s) > stale_busy_secs
        {
            return Readiness::Unknown("stale busy (possible crashed session)".to_string());
        }
        return Readiness::Busy("turn in flight".to_string());
    }
    // state == idle_value: enforce the cooldown.
    match (since_key, since) {
        // Declared a since_key but the file has no usable timestamp: be
        // conservative (Unknown -> floor) rather than declaring Safe with no
        // cooldown -- this is what made a just-idled session read Safe instantly.
        (Some(_), None) => Readiness::Unknown("idle but missing since timestamp".to_string()),
        (Some(_), Some(s)) if now.saturating_sub(s) < min_idle => {
            Readiness::Busy("settling".to_string())
        }
        _ => Readiness::Safe,
    }
}

/// Epoch seconds from SystemTime::now(). Returns 0 on platform error (shouldn't happen).
fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Determine whether a session is safe to upgrade right now.
///
/// Evaluation order:
/// 1. If `tool.readiness` is `File` or `Command`, evaluate the probe.
///    - `File`: interpolate `{session_id}` (and `{pid}` if resolvable), call
///      `classify_state_file`. If `Unknown`, fall through to floor.
///    - `Command`: interpolate, run; exit 0 → `Safe`, non-zero → `Busy`,
///      spawn error → fall through to floor.
/// 2. **Floor** (probe is `None` or returned `Unknown`): compare tmux
///    `session_activity` against `now - min_idle`. Quiet → `Safe`, else
///    `Busy("recent pane activity")`.
///
/// `stale_busy_secs` (configurable via `[readiness].stale_busy_secs`, default
/// [`STALE_BUSY_SECS`]) is threaded into `classify_state_file` as the stale-busy
/// threshold.
pub fn session_readiness(
    tmux: &Tmux,
    name: &str,
    tool: &Tool,
    session_id: &str,
    min_idle: u64,
    stale_busy_secs: u64,
    activity: Option<u64>,
) -> Readiness {
    let now = now_epoch();

    // Resolve pane PID once (best-effort; used for {pid} interpolation).
    let pane_pid: Option<u32> = tmux.pane_pid(name).ok().flatten();

    match &tool.readiness {
        ReadinessProbe::File(p) => {
            // Interpolate {session_id} and {pid} into the path pattern.
            let mut path = p.pattern.replace("{session_id}", session_id);
            if let Some(pid) = pane_pid {
                path = path.replace("{pid}", &pid.to_string());
            }
            if path.contains("{pid}") {
                eprintln!(
                    "muxr: readiness probe for {name}: pattern still has {{pid}} (pane pid unknown)"
                );
            }
            let result = classify_state_file(
                &path,
                &p.state_key,
                &p.idle_value,
                p.since_key.as_deref(),
                now,
                min_idle,
                stale_busy_secs,
            );
            if matches!(result, Readiness::Unknown(_)) {
                // Fall through to the floor
                activity_floor(activity, now, min_idle)
            } else {
                result
            }
        }
        ReadinessProbe::Command(c) => {
            if c.argv.is_empty() {
                return activity_floor(activity, now, min_idle);
            }
            // Interpolate {session_id} and {pid}.
            let interpolated: Vec<String> = c
                .argv
                .iter()
                .map(|a| {
                    let mut s = a.replace("{session_id}", session_id);
                    if let Some(pid) = pane_pid {
                        s = s.replace("{pid}", &pid.to_string());
                    }
                    s
                })
                .collect();
            // Bounded by PROBE_TIMEOUT_SECS so a hung probe can't block muxr.
            match std::process::Command::new(&interpolated[0])
                .args(&interpolated[1..])
                .spawn()
            {
                Ok(mut child) => {
                    let deadline = Instant::now() + Duration::from_secs(PROBE_TIMEOUT_SECS);
                    loop {
                        match child.try_wait() {
                            Ok(Some(status)) if status.success() => return Readiness::Safe,
                            Ok(Some(_)) => {
                                return Readiness::Busy("probe reports busy".to_string());
                            }
                            Ok(None) if Instant::now() >= deadline => {
                                let _ = child.kill();
                                let _ = child.wait();
                                return activity_floor(activity, now, min_idle);
                            }
                            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                            Err(_) => return activity_floor(activity, now, min_idle),
                        }
                    }
                }
                Err(_) => activity_floor(activity, now, min_idle),
            }
        }
        ReadinessProbe::None | ReadinessProbe::Disabled => {
            activity_floor(activity, now, min_idle)
        }
    }
}

/// Universal floor: compare tmux session_activity against min_idle.
/// `activity` is the session's last tmux-activity epoch, fetched ONCE by the
/// caller (so a multi-session sweep is O(n), not O(n^2)). `None` = lookup
/// failed → Unknown (conservative on missing data, never permissive Safe).
fn activity_floor(activity: Option<u64>, now: u64, min_idle: u64) -> Readiness {
    match activity {
        None => Readiness::Unknown("tmux activity unavailable".to_string()),
        Some(a) if now.saturating_sub(a) >= min_idle => Readiness::Safe,
        Some(_) => Readiness::Busy("recent pane activity".to_string()),
    }
}

/// Check if a harness process is running in a tmux session.
#[allow(dead_code)] // used by harness.rs and switcher.rs
pub fn has_harness_process(tmux: &Tmux, tmux_session: &str, bin: &str) -> bool {
    let Some(Ok(Some(shell_pid))) = Some(tmux.pane_pid(tmux_session)) else {
        return false;
    };
    descendant_pids(shell_pid)
        .into_iter()
        .any(|pid| pid_runs_bin(pid, bin))
}

/// True if process `pid` is running `bin`, matched across three sysinfo signals.
/// Reads metadata via sysinfo (syscalls), NOT by shelling `ps` -- a sandbox can
/// block `/bin/ps`, which previously left every `save` with sessionId=null.
///
/// The signals, and exactly what each covers:
///   1. `name()` (executable basename) `== bin`: carries any process whose
///      reported name IS the bin. claude 2.x ships as a native (Mach-O/Bun)
///      binary, so its `name()` is literally `claude` -- this is the signal
///      that works on macOS, where sysinfo's `cmd()` is empty (see below).
///   2. `exe()` file-stem `== bin`: the resolved executable PATH's basename.
///      Populated on macOS via libproc `proc_pidpath` even when `cmd()` is
///      empty, so it catches a binary exec'd directly by path whose `name()`
///      differs or was truncated (the kernel name field is length-capped).
///   3. `cmd()` argv token (`== bin` or ending in `/bin`): argv coverage on
///      Linux, where `cmd()` is populated.
///
/// KNOWN LIMITATION (documented, not a bug): on macOS sysinfo's `cmd()` (argv
/// via KERN_PROCARGS2) is restricted and comes back EMPTY. So a harness launched
/// as a SEPARATE INTERPRETER process whose only "claude" reference is an argv
/// token (e.g. `node /…/claude.js`, where `name()`/`exe()` are both `node`) is
/// NOT detectable on macOS -- neither name/exe nor the empty argv carry it.
/// This is acceptable because current harnesses (claude 2.x) ship as native
/// binaries caught by signal 1; it is the reason v3.0.0's argv-only match broke
/// macOS detection entirely (recycle flush-wait + `muxr upgrade`).
pub fn pid_runs_bin(pid: u32, bin: &str) -> bool {
    let suffix = format!("/{bin}");
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    let Some(p) = sys.process(Pid::from_u32(pid)) else {
        return false;
    };
    // 1. name() == bin -- the macOS-reliable signal for a native binary.
    if p.name().to_str() == Some(bin) {
        return true;
    }
    // 2. exe() file-stem == bin -- a path-exec'd binary; populated on macOS even
    //    when cmd() is empty.
    if p.exe().and_then(|e| e.file_name()).and_then(|n| n.to_str()) == Some(bin) {
        return true;
    }
    // 3. cmd() argv token -- Linux argv coverage (empty on macOS).
    p.cmd().iter().any(|tok| {
        tok.to_str()
            .map(|t| t == bin || t.ends_with(&suffix))
            .unwrap_or(false)
    })
}

impl SavedState {
    /// Load the saved state, or an empty state if no file exists yet.
    pub fn load() -> Result<SavedState> {
        let path = Config::state_path()?;
        if !path.exists() {
            return Ok(SavedState {
                sessions: Vec::new(),
            });
        }
        let content = std::fs::read_to_string(&path)?;
        let state: SavedState = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        Ok(state)
    }

    /// The last-known conversation id recorded for a session name, if any.
    /// This is what makes a dormant campaign resumable: `muxr save` records
    /// each running session's id here, and `open` consults it to relaunch
    /// with `--resume` instead of starting a cold conversation.
    pub fn session_id_for(name: &str) -> Option<String> {
        Self::load()
            .ok()?
            .sessions
            .into_iter()
            .find(|s| s.name == name)
            .and_then(|s| s.session_id)
    }

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

            let harness = name.split('/').next().unwrap_or(&name);

            // Detect if this is a remote proxy session
            let remote = if config.is_remote(harness) {
                Some(harness.to_string())
            } else {
                None
            };

            let tool = config.resolve_tool(harness, None);
            let tool_def = config.tool_for(&tool);
            let session_id = discover_session_id(tmux, &name, tool_def.as_ref());

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
                        tmux.create_session(&s.name, &home, &connect_cmd, &[], None)?;
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

                // Rebuild the full launch (prompt + add-dirs + resume) through
                // the shared composer so a restored session is identical to a
                // freshly opened one. If the campaign/session files are gone
                // (e.g. the session was archived), fall back to a name+resume
                // relaunch rather than dropping the session.
                let tool_cmd = match crate::session::compose_launch_command(
                    config,
                    &s.name,
                    s.session_id.as_deref(),
                    None,
                    true,
                ) {
                    Ok((cmd, _)) => cmd,
                    Err(e) => {
                        eprintln!(
                            "  {} -- full compose failed ({e}); restoring name+resume only",
                            s.name
                        );
                        match config.tool_for(&s.tool) {
                            Some(h) => h.restore_command(Some(&s.name), s.session_id.as_deref()),
                            None => s.tool.clone(),
                        }
                    }
                };
                tmux.create_session(
                    &s.name,
                    &dir,
                    &tool_cmd,
                    &config.session_env_for(&s.name),
                    config
                        .companion_for(&s.name, dir.to_str().unwrap_or(""))
                        .as_ref(),
                )?;
                eprintln!("  {} -> {}", s.name, s.dir);
                count += 1;
            }
        }

        eprintln!("Restored {count} sessions.");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Readiness tests --

    #[test]
    fn classify_state_file_idle_old_since_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        // since = 100 seconds ago, min_idle = 20 → Safe
        let now: u64 = 1_750_000_100;
        let since: u64 = 1_750_000_000;
        std::fs::write(&path, format!(r#"{{"state":"idle","since":{since}}}"#)).unwrap();
        let result = super::classify_state_file(
            path.to_str().unwrap(),
            "state",
            "idle",
            Some("since"),
            now,
            20,
            super::STALE_BUSY_SECS,
        );
        assert!(matches!(result, super::Readiness::Safe), "{result:?}");
    }

    #[test]
    fn classify_state_file_idle_recent_since_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        // since = 5 seconds ago, min_idle = 20 → Busy("settling")
        let now: u64 = 1_750_000_100;
        let since: u64 = 1_750_000_095;
        std::fs::write(&path, format!(r#"{{"state":"idle","since":{since}}}"#)).unwrap();
        let result = super::classify_state_file(
            path.to_str().unwrap(),
            "state",
            "idle",
            Some("since"),
            now,
            20,
            super::STALE_BUSY_SECS,
        );
        assert!(
            matches!(result, super::Readiness::Busy(ref r) if r == "settling"),
            "{result:?}"
        );
    }

    #[test]
    fn classify_state_file_busy_state_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, r#"{"state":"busy"}"#).unwrap();
        let result = super::classify_state_file(
            path.to_str().unwrap(),
            "state",
            "idle",
            None,
            0,
            20,
            super::STALE_BUSY_SECS,
        );
        assert!(matches!(result, super::Readiness::Busy(_)), "{result:?}");
    }

    #[test]
    fn classify_state_file_missing_file_is_unknown() {
        let result = super::classify_state_file(
            "/tmp/muxr-test-nonexistent-readiness-xyz.json",
            "state",
            "idle",
            None,
            0,
            20,
            super::STALE_BUSY_SECS,
        );
        assert!(matches!(result, super::Readiness::Unknown(_)), "{result:?}");
    }

    #[test]
    fn classify_state_file_stale_busy_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        // busy, since > STALE_BUSY_SECS ago -> Unknown (not Busy), so the caller
        // can fall through to the floor instead of blocking forever.
        let now: u64 = 1_750_000_000 + super::STALE_BUSY_SECS + 100;
        let since: u64 = 1_750_000_000;
        std::fs::write(&path, format!(r#"{{"state":"busy","since":{since}}}"#)).unwrap();
        let result = super::classify_state_file(
            path.to_str().unwrap(),
            "state",
            "idle",
            Some("since"),
            now,
            20,
            super::STALE_BUSY_SECS,
        );
        assert!(matches!(result, super::Readiness::Unknown(_)), "{result:?}");
    }

    #[test]
    fn classify_state_file_idle_missing_since_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        // idle but no `since` while since_key is declared -> conservative Unknown.
        std::fs::write(&path, r#"{"state":"idle"}"#).unwrap();
        let result = super::classify_state_file(
            path.to_str().unwrap(),
            "state",
            "idle",
            Some("since"),
            1_750_000_000,
            20,
            super::STALE_BUSY_SECS,
        );
        assert!(matches!(result, super::Readiness::Unknown(_)), "{result:?}");
    }

    #[test]
    fn classify_state_file_stale_busy_custom_threshold_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        // busy file aged 200s. With a lowered stale_busy_secs = 100, 200 > 100
        // -> stale -> Unknown, so the caller falls through to the floor and can
        // reclaim the interrupted-but-quiet session sooner.
        let since: u64 = 1_750_000_000;
        let now: u64 = since + 200;
        std::fs::write(&path, format!(r#"{{"state":"busy","since":{since}}}"#)).unwrap();
        let result = super::classify_state_file(
            path.to_str().unwrap(),
            "state",
            "idle",
            Some("since"),
            now,
            20,
            100,
        );
        assert!(matches!(result, super::Readiness::Unknown(_)), "{result:?}");
    }

    #[test]
    fn classify_state_file_busy_default_threshold_stays_busy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        // The SAME 200s-old busy file, but with the default stale_busy_secs =
        // 3600: 200 < 3600 -> still Busy("turn in flight"). Default behavior is
        // preserved -- the gate blocks upgrade exactly as pre-3.6.
        let since: u64 = 1_750_000_000;
        let now: u64 = since + 200;
        std::fs::write(&path, format!(r#"{{"state":"busy","since":{since}}}"#)).unwrap();
        let result = super::classify_state_file(
            path.to_str().unwrap(),
            "state",
            "idle",
            Some("since"),
            now,
            20,
            super::STALE_BUSY_SECS,
        );
        assert!(
            matches!(result, super::Readiness::Busy(ref r) if r == "turn in flight"),
            "{result:?}"
        );
    }

    #[test]
    fn readiness_config_default_is_3600() {
        // Guards the Default-is-not-zero trap: a 0 threshold would make every
        // busy file instantly stale. Default must equal the const (3600).
        assert_eq!(
            crate::config::ReadinessConfig::default().stale_busy_secs,
            3600
        );
        assert_eq!(
            crate::config::ReadinessConfig::default().stale_busy_secs,
            super::STALE_BUSY_SECS
        );
    }

    #[test]
    fn pid_runs_bin_detects_native_binary_by_name() {
        // Signal 1 (name()): the macOS-load-bearing path. sysinfo's cmd() is
        // EMPTY on macOS, so v3.0.0's argv-only match reported "no process" for
        // every live harness -> broke recycle/upgrade. `sleep` is a native
        // binary (like claude 2.x), so its name() is "sleep" and this exercises
        // exactly the signal that carries detection on macOS. NOTE: this does
        // NOT exercise the macOS interpreter+argv gap documented on
        // `pid_runs_bin` -- that case is undetectable by design, not tested.
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let found = pid_runs_bin(child.id(), "sleep");
        let _ = child.kill();
        let _ = child.wait();
        assert!(found, "pid_runs_bin must detect a running native `sleep`");
    }

    #[test]
    fn pid_runs_bin_no_false_match_on_unrelated_bin() {
        // The match must be specific: a live process must NOT report as running
        // some other bin. Guards the false-positive direction (a wrong match
        // feeds recycle's wait_for_exit, which SIGKILLs the matched pid).
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let matched = pid_runs_bin(child.id(), "muxr-no-such-harness");
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            !matched,
            "pid_runs_bin must not match an unrelated bin name"
        );
    }

    #[test]
    fn pid_runs_bin_false_for_dead_pid() {
        // A pid sysinfo can't see resolves to "not running", never a panic.
        assert!(!pid_runs_bin(u32::MAX, "claude"));
    }
}
