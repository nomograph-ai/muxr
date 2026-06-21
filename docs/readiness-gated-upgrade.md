# Readiness-gated upgrade

Status: design (proposed)
Scope: `muxr upgrade` / `muxr migrate`, plus a read-only `muxr status` view.

## Problem

`muxr upgrade` relaunches running sessions in place (graceful `/exit` →
relaunch with `--resume`). The conversation log is always safe — that is what
`--resume` reads — but the relaunch *interrupts in-flight work*: a turn that is
actively generating, a running tool/sub-agent, a build, or a pending permission
prompt. Operators are (rightly) nervous about migrating a fleet without knowing
which sessions are at a safe boundary.

We want muxr to answer "is this session safe to relaunch right now?" and gate
the upgrade on it — **without** baking any single runtime's internals into core.

## Constraints

1. **Runtime-agnostic.** muxr drives multiple runtimes (`claude`, `opencode`,
   `pi`, and whatever comes next). Core must never branch on the tool name or
   know a runtime's session-log format. Anything runtime-specific lives in the
   adapter TOML (data) or in that runtime's own hooks (outside muxr).
2. **Extension-based.** The capability plugs in the same way every other
   per-tool behavior does: a declarative field on the `Tool` descriptor,
   shipped in `extensions/adapters/<tool>.toml`, overridable via
   `[tools.<name>]`, merged by the existing type-default heuristic. This mirrors
   `session_discovery` exactly.

## The interface: a normalized state file

The contract between *any* runtime and muxr is one small JSON file, written by
that runtime's own turn-boundary hooks:

```json
{ "state": "idle", "since": 1750000000 }
```

- `state`: `"idle"` (at a safe boundary) or `"busy"` (turn/tool in flight).
- `since`: epoch seconds of the last transition, used to enforce a quiet period.

muxr reads this generically — tilde-expand, interpolate the path, parse JSON,
pull the declared keys — exactly like `read_session_id_from_file`. muxr has no
idea how the file was produced. That is the whole point: the runtime owns
*producing* the signal; muxr owns *interpreting* it uniformly.

## Extension point: `ReadinessProbe` on `Tool`

Mirrors `SessionDiscovery`. New field on `Tool` plus a tagged enum:

```rust
#[serde(default = "default_readiness_none")]
pub readiness: ReadinessProbe,

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ReadinessProbe {
    /// Read a normalized state file the runtime's hooks maintain.
    File {
        pattern: String,           // supports {session_id} (preferred) and {pid}
        state_key: String,         // e.g. "state"
        idle_value: String,        // value meaning safe, e.g. "idle"
        #[serde(default)]
        since_key: Option<String>, // epoch key for the quiet period
    },
    /// Escape hatch: a runtime that exposes state via a CLI. exit 0 = idle.
    Command { argv: Vec<String> }, // {session_id}/{pid} interpolation
    /// No runtime probe — core uses the universal floor only.
    None,
}
```

Merge rule (in `merge_tool_with_builtin`): user `None` → builtin wins; otherwise
user wins. Identical to the `session_discovery` arm.

## Core classifier (runtime-agnostic)

```rust
pub enum Readiness { Safe, Busy(String), Unknown(String) }

pub fn session_readiness(
    tmux: &Tmux, name: &str, tool: &Tool, session_id: &str, min_idle: u64,
) -> Readiness
```

Evaluation order — none of it tool-specific:

1. **Probe** (if declared, `File` or `Command`):
   - `busy` → `Busy("turn in flight")`
   - `idle` and `now - since >= min_idle` → `Safe`
   - `idle` but too recent → `Busy("settling")`
   - file missing / parse error / command failure → `Unknown`
2. **Universal floor** (probe is `None` or `Unknown`): the tmux
   `session_activity` timestamp (already exposed via `list_sessions_detailed`).
   Quiet for `>= min_idle` → `Safe`; otherwise `Busy("recent pane activity")`.
   Process-tree descendants are deliberately **not** interpreted semantically:
   telling an MCP server apart from a real worker needs runtime knowledge, which
   would break agnosticism. The floor stays coarse-but-correct.
3. **Conservative default:** `Unknown` is treated as not-safe unless `--force`.
   Never relaunch on doubt.

## `upgrade()` integration

Insert the gate right after `discover_session_id` succeeds and before the
`/exit` send. New flags:

- default: migrate only `Safe`; print `Busy`/`Unknown` sessions with their
  reason and skip them.
- `--force`: ignore readiness (today's unconditional behavior).
- `--wait [secs]`: poll readiness until `Safe` or timeout, then migrate.
- `--dry-run`: gains a readiness column per session.

Plus a read-only **`muxr status`** subcommand that runs the same classifier and
prints `SAFE` / `BUSY(reason)` / `UNKNOWN(reason)` per session — the "tell me
when it is safe" surface, usable any time without touching a session.

## Per-runtime plug-ins (one example)

The Claude adapter is just one implementation of the contract:

`extensions/adapters/claude.toml`:

```toml
[readiness]
type = "file"
pattern = "~/.config/muxr/readiness/{session_id}.json"
state_key = "state"
idle_value = "idle"
since_key = "since"
```

The hooks that *produce* that file live in the operator's harness, **not** in
muxr: a Claude Code `Stop` hook writes `{state:"idle", since:<now>}`;
`UserPromptSubmit` / `PreToolUse` write `{state:"busy"}`. `opencode` and `pi`
adapters declare their own `[readiness]` (their event mechanism) or omit it and
fall back to the floor. Adding a runtime stays pure config + hooks.

## Layering (how it earns "agnostic + extension")

| Layer | Owns | Runtime-specific? |
|---|---|---|
| muxr core | probe reading, classifier, gate, `--force`/`--wait`, `muxr status` | No |
| adapter TOML | *declares* where/how to read state | data only |
| harness hooks | *produce* the `{state,since}` file | yes — isolated |

## Non-goals

- No semantic process-tree interpretation in core (agnosticism).
- No dependency on the legacy 2.x statusline health files.
- muxr does not produce the state file for any runtime; runtimes do.
