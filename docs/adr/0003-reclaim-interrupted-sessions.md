# ADR 0003: Reclaim interrupted-but-idle sessions via a Command probe

- Status: Accepted
- Date: 2026-07-01
- Relates to: [ADR 0001](0001-extension-architecture.md), [ADR 0002](0002-readiness-gated-upgrade.md), nomograph/muxr#6
- Realized in: `extensions/examples/readiness.sh` (reference implementation)

## Context

Under [ADR 0002](0002-readiness-gated-upgrade.md) the `File` probe trusts a
`busy` state file until it is older than `STALE_BUSY_SECS` (1h). The flag is
written by turn-boundary hooks (`UserPromptSubmit` / `PreToolUse` -> `busy`,
`Stop` -> `idle`). But an **interrupted** Claude turn fires no `Stop` hook, so
`busy` is never cleared. For up to an hour an idle-and-waiting session reads
`Busy("turn in flight")` and every readiness-gated `upgrade` skips it --
silently, because the skip reason looks legitimate. The classifier's stale-busy
guard is time-only (`STALE_BUSY_SECS`), which is not enough: it cannot tell an
interrupted session from one that is genuinely working.

## Decision

Do not trust `busy` blindly -- corroborate it against real tmux pane activity,
and do so **in the extension layer with no muxr core change** (per the
extensions-first posture of [ADR 0001](0001-extension-architecture.md): keep core
stable, release rarely). Wire a `Command`
readiness probe in place of the `File` probe:

- `busy` + pane active within `min_idle` -> genuinely working -> stay Busy.
- `busy` + pane quiet for `>= min_idle` -> stale / interrupted -> reclaim (Safe).

The shipped `File` probe stays the dependency-free default; the Command probe is
an opt-in an operator wires in their own config.

## Consequences

- **No `src/` change.** muxr already interpolates `{pid}` (the tmux `pane_pid`)
  into a `Command` probe's argv, so the script recovers the pane's
  `session_activity` from tmux itself and makes the call. The lever is entirely
  the extension.
- The corroboration script owns its own quiet period -- a `Command` probe does
  not receive muxr's `min_idle`, so the script defaults it (45s), overridable.
- No semantic process-tree interpretation; this stays within ADR 0001's
  coarse-floor rule.
- A `Command` probe's `argv[0]` is executed directly (not via a shell), so it is
  not tilde-expanded -- the wired path must be absolute (or a name on `PATH`).
- Reference implementation: [`../../extensions/examples/readiness.sh`](../../extensions/examples/readiness.sh),
  wired via `[tools.<name>.readiness] type = "command"`.

## Alternatives considered

- **Teach the core `File`-probe `busy` arm to corroborate against the `activity`
  timestamp `session_readiness` already receives.** Small, and it would give
  every `File`-probe runtime the reclaim for free. But it is a core/classifier
  change and therefore a release. Deferred unless the probe approach proves
  insufficient -- extensions-first prefers the probe.
- **Shorten `STALE_BUSY_SECS`.** Still time-only: it just trades a longer-stranded
  session for a sooner-but-still-blind reclaim, and risks reclaiming a genuinely
  long-running turn. Rejected -- it does not distinguish interrupted from working.
