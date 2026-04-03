use anyhow::{Context, Result};
use std::process::Command;

use crate::config::Remote;

/// Resolve the external IP of a GCE instance via gcloud.
/// Caches the result in /tmp/muxr-ip-<instance> for 5 minutes.
pub fn resolve_ip(remote: &Remote, instance: &str) -> Result<String> {
    let cache_path = format!("/tmp/muxr-ip-{instance}");

    // Check cache (5 min TTL)
    if let Ok(metadata) = std::fs::metadata(&cache_path)
        && let Ok(modified) = metadata.modified()
        && let Ok(age) = modified.elapsed()
        && age.as_secs() < 300
        && let Ok(ip) = std::fs::read_to_string(&cache_path)
    {
        let ip = ip.trim().to_string();
        if !ip.is_empty() {
            return Ok(ip);
        }
    }

    let output = Command::new("gcloud")
        .args([
            "compute",
            "instances",
            "describe",
            instance,
            "--project",
            &remote.project,
            "--zone",
            &remote.zone,
            "--format",
            "get(networkInterfaces[0].accessConfigs[0].natIP)",
        ])
        .output()
        .context("Failed to run gcloud")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("gcloud describe failed for {instance}: {stderr}");
    }

    let ip = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if ip.is_empty() {
        anyhow::bail!("No external IP found for {instance}");
    }

    let _ = std::fs::write(&cache_path, &ip);

    Ok(ip)
}

/// Build the connection command string for a remote session.
/// Wraps the SSH/mosh command in a reconnect loop that:
/// - Exits cleanly on exit code 0 (user typed `exit` or clean detach)
/// - Reconnects on non-zero exit (connection failure) with backoff
/// - Invalidates IP cache before retrying (handles IP changes)
/// - Gives up after 20 consecutive failures
pub fn connect_command(remote: &Remote, instance: &str, context: &str) -> Result<String> {
    let ip = resolve_ip(remote, instance)?;
    let cache_path = format!("/tmp/muxr-ip-{instance}");

    let inner_cmd = match remote.connect.as_str() {
        "mosh" => {
            format!(
                "mosh --ssh='ssh -o StrictHostKeyChecking=no' {}@{} -- tmux new-session -A -s {}",
                remote.user, ip, context
            )
        }
        _ => {
            format!(
                "gcloud compute ssh {}@{} --project={} --zone={} -- tmux new-session -A -s {}",
                remote.user, instance, remote.project, remote.zone, context
            )
        }
    };

    // Wrap in reconnect loop
    let cmd = format!(
        r#"fails=0; delay=3; while true; do {inner_cmd}; rc=$?; if [ $rc -eq 0 ]; then break; fi; fails=$((fails+1)); if [ $fails -ge 20 ]; then echo "muxr: 20 consecutive failures, giving up."; break; fi; rm -f {cache_path}; echo "muxr: connection lost (rc=$rc). Reconnecting in ${{delay}}s... (attempt $fails/20)"; sleep $delay; delay=$((delay<30 ? delay*2 : 30)); done"#
    );

    Ok(cmd)
}

/// Bootstrap Claude Code config on a remote instance.
/// Checks if ~/.claude/settings.json exists; if not, pushes a baseline config.
/// This ensures AI coding tools have consistent settings on fresh lab VMs.
pub fn bootstrap_claude_config(remote: &Remote, instance: &str) -> Result<()> {
    let check = Command::new("gcloud")
        .args([
            "compute",
            "ssh",
            &format!("{}@{}", remote.user, instance),
            "--project",
            &remote.project,
            "--zone",
            &remote.zone,
            "--command",
            "test -f ~/.claude/settings.json && echo exists || echo missing",
        ])
        .output()
        .context("Failed to check remote Claude config")?;

    let stdout = String::from_utf8_lossy(&check.stdout);
    if stdout.trim() == "exists" {
        return Ok(());
    }

    eprintln!("  Bootstrapping Claude Code config...");

    // Baseline config for remote sessions
    let config_json = r#"{
  "model": "opus[1m]",
  "effortLevel": "high",
  "env": {
    "CLAUDE_CODE_NO_FLICKER": "1",
    "CLAUDE_ENABLE_STREAM_WATCHDOG": "1",
    "CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY": "20",
    "API_TIMEOUT_MS": "900000",
    "CLAUDE_CODE_DISABLE_TERMINAL_TITLE": "1",
    "CLAUDE_BASH_MAINTAIN_PROJECT_WORKING_DIR": "1"
  }
}"#;

    let setup_cmd = format!(
        "mkdir -p ~/.claude && cat > ~/.claude/settings.json << 'MUXR_EOF'\n{config_json}\nMUXR_EOF"
    );

    let status = Command::new("gcloud")
        .args([
            "compute",
            "ssh",
            &format!("{}@{}", remote.user, instance),
            "--project",
            &remote.project,
            "--zone",
            &remote.zone,
            "--command",
            &setup_cmd,
        ])
        .status()
        .context("Failed to bootstrap Claude config")?;

    if status.success() {
        eprintln!("  Claude config ready");
    } else {
        eprintln!("  Warning: Claude config bootstrap failed (non-fatal)");
    }

    Ok(())
}

/// List remote tmux sessions by SSHing to the instance.
pub fn list_remote_sessions(remote: &Remote, instance: &str) -> Result<Vec<String>> {
    let output = Command::new("gcloud")
        .args([
            "compute",
            "ssh",
            &format!("{}@{}", remote.user, instance),
            "--project",
            &remote.project,
            "--zone",
            &remote.zone,
            "--command",
            "tmux list-sessions -F '#{session_name}' 2>/dev/null || true",
        ])
        .output()
        .context("Failed to SSH for session listing")?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let sessions = stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    Ok(sessions)
}

/// List running GCE instances matching a remote's prefix.
pub fn list_instances(remote: &Remote) -> Result<Vec<String>> {
    let prefix = remote.instance_prefix.as_deref().unwrap_or("");
    let filter = if prefix.is_empty() {
        "status=RUNNING".to_string()
    } else {
        format!("name~^{prefix} AND status=RUNNING")
    };

    let output = Command::new("gcloud")
        .args([
            "compute",
            "instances",
            "list",
            "--project",
            &remote.project,
            "--filter",
            &filter,
            "--format",
            "value(name)",
        ])
        .output()
        .context("Failed to list GCE instances")?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}
