# ADR 0001: A small stable core, iterated through extensions

- Status: Accepted
- Date: 2026-07-01 (recorded; the contract was earned and locked at 3.0.0)
- Implemented in: the launch chokepoint (`src/session.rs`) + the subprocess invoke (`src/extension.rs`); adapters embedded via `include_str!` (3.1.0)

## Context

muxr's irreducible job is small: a runtime-agnostic resume engine over tmux --
snapshot `(name, dir, runtime, session-id)`, replay the runtime's `--resume`,
recycle (flush -> exit -> reopen fresh from a pointer), and save/restore across
reboots. No dominant host absorbs that shape, which is why muxr exists at all.

Everything *around* the resume engine -- how a given coding CLI launches, where
its session id lives, how a launch maps to a repo or worktree, what "flush before
recycle" means, when a session is safe to relaunch -- is fiddly, varies by
runtime and by operator, and changes far more often than the core. Two failure
modes follow:

- **Core churn.** Folding each new per-runtime or per-operator behavior into core
  means the binary revs constantly, every rev risks the resume engine, and every
  operator is pinned to muxr's release cadence for behavior that is really their
  own.
- **Premature abstraction.** Locking an extension contract before it is
  understood produces the wrong seams -- the synthesist v2 -> v3 rebuild trap.

## Decision

Keep muxr **core small and stable, and rev it as rarely as possible.** Behavior
that varies by runtime or operator lives in **extensions**, not core.

There is **one** extension mechanism: at an opinionated chokepoint muxr
optionally invokes a configured command with structured JSON on stdin and reads
JSON (or, for a probe, an exit code) back; with nothing configured it runs a
built-in default and behaves exactly as the prior release (verified byte-for-byte
against the 2.1 tag). Transport is a **subprocess** (`sh -c`,
`MUXR_EXTENSION_POINT` env, fail-closed), deliberately **not WASM**. The contract
was earned from real extractions (resolver + make-durable, alongside the existing
hook and statusline seams) and **locked at 3.0.0**.

Extensions take two shapes:

- **Declarative adapters** (`[tools.<name>]` TOML) -- how to drive one CLI:
  launch / resume / continue / fork / model / exit, `session_discovery`,
  `readiness`. Adding a runtime is config, no code. `claude` + `pi` are the
  embedded defaults (`include_str!`); `opencode` is a config-only third-party
  port (a worked example, not a default).
- **Imperative subprocess points** -- `resolver` (the single launch chokepoint:
  intent in, layout facts out), `session_env`, `chooser`, `pre_create` hooks, and
  adapter-declared `Command` probes (`session_discovery`). Any language, any
  logic; they live in the operator's estate repo, referenced from the muxr
  config. (`make_durable` and the `readiness` probe were removed in 4.0.0 -- see
  [ADR 0008](0008-remove-readiness-inference-recycle-sentinel.md): recycle's
  flush moved into the estate `/recycle` skill, and readiness inference was
  deleted rather than re-patched.)

## Consequences

- **Most change ships without a muxr release** -- as an edit to a script or a
  TOML adapter in the operator's estate. `session_discovery` (adapter TOML) and
  the `resolver` / `session_env` / `chooser` subprocess points are all
  operator-owned and change without a binary rev. (The readiness probe of
  [ADR 0002](0002-readiness-gated-upgrade.md) / [ADR 0003](0003-reclaim-interrupted-sessions.md)
  was one such example until 4.0.0 removed readiness inference entirely --
  [ADR 0008](0008-remove-readiness-inference-recycle-sentinel.md); its removal was
  about an unobservable signal, not a failure of the extension posture.)
- Bare muxr (no `[extensions]` / `[session_env]` / `[chooser]`) stays a fully
  usable launcher -- the bare-launcher test.
- The cost is one indirection per chokepoint (a subprocess) and the discipline
  below. Accepted deliberately over a richer embedded runtime.
- **Not in core:** the statusline (a runtime's own chrome, shed in 3.0.0), WASM /
  wasmtime, per-runtime code branches, and any second bespoke mechanism.

## Discipline (governs change post-lock)

- **Prefer an extension over a core change.** Treat a core or config-schema
  change as the last resort, taken only when the contract genuinely cannot
  express the need. When it can, the fix is a script or adapter edit.
- **Earn a new extension point from >= 2 real extractions** before adding it;
  keep the format dead-simple (subprocess + JSON, no wasmtime); ONE contract for
  all bits, not N mechanisms.
- **Defaults stay behavior-compatible.** A behavior change to a default, or a new
  contract shape, is a major bump -- the core/extension re-architecture is what
  earned the 3.0 major.

## Alternatives considered

- **WASM / component-model extensions (wasmtime in core).** Sandboxed and typed,
  but heavy, a second transport shape, and over-engineered for "JSON in, JSON
  out." Transport-per-shape beats one embedded runtime; core stays
  dependency-light. Rejected.
- **Fold behavior into core (no extension contract).** Simplest to write, but it
  is the core-churn failure above -- every operator pinned to muxr's cadence.
  Rejected.
- **Adopt a dominant host (a container / worktree-per-agent fleet, e.g. scion).**
  No host has the right shape, and the nearest ones are unstable; the resume
  engine has no home but muxr, so core stays first-party and minimal. Rejected.
