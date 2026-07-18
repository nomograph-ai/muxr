# ADR 0002: Readiness-gated upgrade

- Status: Superseded by [ADR 0008](0008-remove-readiness-inference-recycle-sentinel.md)
- Date: 2026-07-01 (recorded; the design predates this and was captured as prose)
- Implemented in: `src/state.rs` (probe classifier + gate), `muxr upgrade` / `muxr migrate`, `muxr status`
- Relates to: [ADR 0001](0001-extension-architecture.md) -- the extensions-first posture this instantiates

## Context

`muxr upgrade` relaunches running sessions in place (graceful `/exit` ->
relaunch with `--resume`). The conversation log is always safe -- that is what
`--resume` reads -- but the relaunch *interrupts in-flight work*: a turn that is
actively generating, a running tool/sub-agent, a build, or a pending permission
prompt. Migrating a fleet blind is risky; muxr needs to answer "is this session
safe to relaunch right now?" and gate on it, under two constraints:

1. **Runtime-agnostic.** muxr drives multiple runtimes (`claude`, `opencode`,
   `pi`, and whatever comes next). Core must never branch on the tool name or
   know a runtime's session-log format. Anything runtime-specific lives in the
   adapter TOML (data) or in that runtime's own hooks (outside muxr).
2. **Extension-based.** The capability plugs in the same way every other
   per-tool behavior does: a declarative field on the `Tool` descriptor, shipped
   in `extensions/adapters/<tool>.toml`, overridable via `[tools.<name>]`, merged
   by the existing type-default heuristic. This mirrors `session_discovery`.

## Decision

Gate `upgrade` / `migrate` on a runtime-agnostic **readiness classifier** fed by
an adapter-declared **probe** over a normalized `{state, since}` state file, with
a universal tmux-activity **floor** whenever no probe (or an inconclusive one)
applies. muxr interprets a uniform signal; each runtime *produces* it. `Unknown`
is never safe unless `--force`.

## Design detail

### The interface: a normalized state file

The contract between *any* runtime and muxr is one small JSON file, written by
that runtime's own turn-boundary hooks:

```json
{ "state": "idle", "since": 1750000000 }
```

- `state`: `"idle"` (at a safe boundary) or `"busy"` (turn/tool in flight).
- `since`: epoch seconds of the last transition, used to enforce a quiet period.

muxr reads this generically -- tilde-expand, interpolate the path, parse JSON,
pull the declared keys -- exactly like `read_session_id_from_file`. muxr has no
idea how the file was produced: the runtime owns *producing* the signal; muxr
owns *interpreting* it uniformly.

### Extension point: `ReadinessProbe` on `Tool`

Mirrors `SessionDiscovery`. A field on `Tool` plus a tagged enum:

```rust
#[serde(default = "default_readiness_none")]
pub readiness: ReadinessProbe,

#[serde(tag = "type", rename_all = "lowercase")]
pub enum ReadinessProbe {
    /// Read a normalized state file the runtime's hooks maintain.
    File {
        pattern: String,           // supports {session_id} (preferred) and {pid}
        state_key: String,
        idle_value: String,
        since_key: Option<String>, // epoch key for the quiet period
    },
    /// Escape hatch: a runtime that exposes state via a CLI. exit 0 = idle.
    Command { argv: Vec<String> }, // {session_id}/{pid} interpolation
    None,
}
```

Merge rule (`merge_tool_with_builtin`): user `None` -> builtin wins; otherwise
user wins. Identical to the `session_discovery` arm.

### Core classifier (runtime-agnostic)

```rust
pub enum Readiness { Safe, Busy(String), Unknown(String) }

pub fn session_readiness(
    tmux: &Tmux, name: &str, tool: &Tool, session_id: &str, min_idle: u64,
) -> Readiness
```

Evaluation order -- none of it tool-specific:

1. **Probe** (if declared, `File` or `Command`):
   - `busy` -> `Busy("turn in flight")`
   - `idle` and `now - since >= min_idle` -> `Safe`
   - `idle` but too recent -> `Busy("settling")`
   - file missing / parse error / command failure -> `Unknown`
2. **Universal floor** (probe is `None` or `Unknown`): the tmux `session_activity`
   timestamp (exposed via `list_sessions_detailed`). Quiet for `>= min_idle` ->
   `Safe`; otherwise `Busy("recent pane activity")`. Process-tree descendants are
   deliberately **not** interpreted semantically -- telling an MCP server apart
   from a real worker needs runtime knowledge, which would break agnosticism. The
   floor stays coarse-but-correct.
3. **Conservative default:** `Unknown` is not-safe unless `--force`. Never
   relaunch on doubt.

### `upgrade()` integration

The gate sits right after `discover_session_id` succeeds and before the `/exit`
send:

- default: migrate only `Safe`; print `Busy`/`Unknown` sessions with their reason
  and skip them.
- `--force`: ignore readiness (the pre-gate unconditional behavior).
- `--wait [secs]`: poll readiness until `Safe` or timeout, then migrate.
- `--dry-run`: gains a readiness column per session.

Plus a read-only **`muxr status`** subcommand running the same classifier,
printing `SAFE` / `BUSY(reason)` / `UNKNOWN(reason)` per session -- the "tell me
when it is safe" surface, usable any time without touching a session.

### Per-runtime plug-in (one example)

The Claude adapter is one implementation of the contract. `extensions/adapters/claude.toml`:

```toml
[readiness]
type = "file"
pattern = "~/.config/muxr/readiness/{session_id}.json"
state_key = "state"
idle_value = "idle"
since_key = "since"
```

The hooks that *produce* that file live in the operator's harness, **not** in
muxr: a `Stop` hook writes `{state:"idle", since:<now>}`; `UserPromptSubmit` /
`PreToolUse` write `{state:"busy"}`. `opencode` and `pi` adapters declare their
own `[readiness]` or omit it and fall back to the floor. Adding a runtime stays
pure config + hooks.

## Consequences

Layering is how this earns "agnostic + extension":

| Layer | Owns | Runtime-specific? |
|---|---|---|
| muxr core | probe reading, classifier, gate, `--force`/`--wait`, `muxr status` | No |
| adapter TOML | *declares* where/how to read state | data only |
| harness hooks | *produce* the `{state,since}` file | yes -- isolated |

Non-goals:

- No semantic process-tree interpretation in core (agnosticism).
- No dependency on the legacy 2.x statusline health files.
- muxr does not produce the state file for any runtime; runtimes do.

Follow-up: a `busy` flag can go **stale** when a runtime fires no turn-end hook
(e.g. an interrupted Claude turn leaves `busy` set with no `Stop`). Closing that
without breaking agnosticism is its own decision -- see
[ADR 0003](0003-reclaim-interrupted-sessions.md).
