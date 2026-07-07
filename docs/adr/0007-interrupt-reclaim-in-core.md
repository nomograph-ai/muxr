# ADR 0007: Interrupt-reclaim in the core File probe (corroborate busy against pane activity)

- Status: Accepted
- Date: 2026-07-07
- Relates to: [ADR 0001](0001-extension-architecture.md), [ADR 0002](0002-readiness-gated-upgrade.md), [ADR 0003](0003-reclaim-interrupted-sessions.md), nomograph/muxr#12
- Implemented in: 3.6.2

## Context

Under [ADR 0002](0002-readiness-gated-upgrade.md) the default `File` probe
trusts a `busy` state file until it is older than `stale_busy_secs` (default
1 h). The flag is written by turn-boundary hooks (`UserPromptSubmit` /
`PreToolUse` -> `busy`, `Stop` -> `idle`). An INTERRUPTED turn fires no `Stop`,
so `busy` is never cleared: `classify_state_file` returns `Busy` and
`session_readiness` returns it directly, never consulting the tmux-activity
floor. For up to an hour `upgrade` skips the session and `recycle` waits out its
timeout (#12).

[ADR 0003](0003-reclaim-interrupted-sessions.md) solved this as an OPT-IN
`Command` probe (`extensions/examples/readiness.sh`) that corroborates the
`busy` claim against pane activity. But the default (File probe, 1 h) still
shipped the stranding behavior, and #3.6.0's configurable `stale_busy_secs` was
a knob, not a fix: the out-of-the-box hour-long block remained. The operator's
directive was to fix it fully, in core, by default.

## Decision

Promote ADR 0003's corroboration into the DEFAULT File-probe path. When
`classify_state_file` returns still-busy (`BUSY_IN_FLIGHT`), `session_readiness`
no longer returns it blindly: `corroborate_busy` checks the tmux-activity floor,
and if the pane has been quiet at least `max(min_idle, INTERRUPT_RECLAIM_QUIET_SECS)`
the turn is treated as interrupted and reclaimed (`Safe`). A genuinely in-flight
turn keeps the pane refreshing (elapsed timer / streamed tokens), so a pane
quiet that long is not a live turn.

## Consequences

- `upgrade` reclaims an interrupted-but-quiet session after the quiet window
  instead of skipping it for up to `stale_busy_secs`; `recycle` stops waiting
  out its full timeout against one.
- Conservative by construction: `INTERRUPT_RECLAIM_QUIET_SECS` (120 s) floors
  the small `min_idle` that recycle passes (5 s), so a briefly-paused live flush
  is never cut off; `upgrade`'s larger `min_idle` (default 180 s) is used as-is.
  When tmux activity is unavailable the verdict stays `Busy`, so
  `stale_busy_secs` remains the backstop -- reclaim is never permissive on
  missing data.
- Post-idle cooldown (`"settling"`) and `Safe` are untouched; only the
  interrupted-turn case is reclaimed.
- The ADR 0003 `Command` probe example remains valid for runtimes whose live
  turns do NOT refresh the pane (where pane-quiet is a weaker signal); the core
  default now covers the common Claude Code case without an opt-in.

## Alternatives considered

- **Keep it opt-in (ADR 0003 only).** Rejected per the directive: the default
  must not strand interrupted sessions for an hour.
- **Lower the default `stale_busy_secs`.** A blunt instrument: it also shortens
  the crashed-session backstop and still ignores pane activity, so it either
  reclaims too eagerly (blind to a genuinely long turn) or not eagerly enough.
- **Reuse `min_idle` directly as the corroboration window.** Rejected: recycle
  passes `min_idle = 5 s`, which would let a briefly-paused live flush be
  reclaimed mid-flush. The dedicated `INTERRUPT_RECLAIM_QUIET_SECS` floor
  decouples the interrupt window from the idle cooldown.
