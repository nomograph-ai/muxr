//! The muxr extension contract (3.0).
//!
//! ONE mechanism for every fiddly bit that keeps changing: at an opinionated
//! chokepoint muxr OPTIONALLY invokes a configured command with structured
//! JSON on stdin and reads structured JSON from stdout. When no command is
//! configured for a point, muxr runs its built-in DEFAULT and behaves exactly
//! as 2.1 did -- so bare muxr (no `[extensions]`) stays a usable launcher.
//!
//! Transport is a SUBPROCESS, deliberately NOT WASM. It mirrors muxr's
//! existing `pre_create` hook runner (and the runtime's own statusline
//! command, which muxr no longer ships) and synthesist's
//! `discover_policy_extension`. One transport for every shape keeps the social
//! contract between tools thin: "JSON in, JSON out, default when absent" --
//! no shared library, no lockstep.
//!
//! The extension point name is exported as `MUXR_EXTENSION_POINT` so a single
//! dispatcher script can serve several points and branch on it.

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::Write;
use std::process::{Command, Stdio};

/// Invoke an extension command at `point`: run `sh -c <cmd>`, write the JSON
/// encoding of `input` to its stdin, and parse `O` from its stdout. The
/// command inherits stderr (so diagnostics surface) and is told which point
/// it is serving via `MUXR_EXTENSION_POINT`.
///
/// Errors are propagated, not swallowed: an extension is OPT-IN, and once you
/// configure one that decides where a session launches, a silent fallback
/// could attach to the wrong place. Fail closed and loud; the absent-config
/// path (the default) is what stays quietly behavior-compatible.
pub fn invoke<I, O>(cmd: &str, point: &str, input: &I) -> Result<O>
where
    I: Serialize,
    O: DeserializeOwned,
{
    let payload = serde_json::to_vec(input)
        .with_context(|| format!("serialize {point} extension input"))?;

    let mut child = Command::new("sh")
        .args(["-c", cmd])
        .env("MUXR_EXTENSION_POINT", point)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn {point} extension: {cmd}"))?;

    child
        .stdin
        .take()
        .context("extension stdin unavailable")?
        .write_all(&payload)
        .with_context(|| format!("write {point} extension input"))?;

    let out = child
        .wait_with_output()
        .with_context(|| format!("await {point} extension"))?;

    if !out.status.success() {
        anyhow::bail!("{point} extension `{cmd}` exited {}", out.status);
    }

    serde_json::from_slice(&out.stdout).with_context(|| {
        format!(
            "parse {point} extension output as JSON: {}",
            String::from_utf8_lossy(&out.stdout).trim()
        )
    })
}
