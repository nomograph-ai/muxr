# ADR 0010: Recycle flush is muxr-owned (generic flush prompt + agent sentinel)

- Status: Accepted
- Date: 2026-07-19
- Deciders: operator + implementation
- Refines: [ADR 0008](0008-remove-readiness-inference-recycle-sentinel.md) (the sentinel-handshake mechanics)
- Relates to: [ADR 0001](0001-extension-architecture.md), [ADR 0004](0004-companion-pane.md)
- Implemented in: 4.0.0

## Context

ADR 0008 removed readiness inference and made recycle a positive sentinel
handshake. In the Phase-1 interim, the FLUSH half lived in an external Claude
Code skill (the estate's `dunn.dev/runes/recycle`): the agent ran it to flush
via `durable`, then handed off to muxr for the exit + reopen. That kept the
flush agent-owned (correct) but left recycle NOT self-contained -- it required a
per-runtime, estate-maintained skill, and the guidance drifted from muxr's own.

Two facts made that split unnecessary:

1. **muxr already emits its own skill.** `muxr skill` prints a compiled-in
   `resources/skill.md` -- "so it never drifts from the installed version" -- and
   it already documents the flush procedure, noting "muxr owns the log.md format,
   so the procedure lives here (no separate skill to drift out of sync)."
2. **muxr defines the layout it flushes to** (`campaigns/`, `log.md`,
   `entrypoint:`), so a GENERIC flush is muxr-expressible. Only the estate's push
   discipline (safe-land, blast-radius gate, via `durable`) is estate-specific --
   exactly what an override seam is for.

## Decision

**muxr owns recycle end-to-end. `muxr recycle` sends a flush prompt into the
pane and waits for the agent's positively-written sentinel; no external skill.**

1. **Flush prompt.** `muxr recycle` `send_text`s a flush prompt into the pane --
   plain text, so it is RUNTIME-AGNOSTIC (claude/pi/opencode/...), not a
   Claude-Code skill. The default (`DEFAULT_FLUSH_PROMPT`) is muxr-owned and
   generic: it references muxr's own log.md flush procedure and ends by telling
   the agent to write a done-signal file. A harness overrides it via
   `[recycle].flush_prompt` (tokens `{session} {repo} {campaign} {log}
   {sentinel}`) -- e.g. to compose the estate `durable` skill.

2. **Positive sentinel gate.** The agent flushes in-context and, when done,
   writes `~/.local/state/muxr/recycle-<slug>.flag` (the path the prompt hands
   it). muxr WAITS for that file -- the agent's positive "flush complete" -- then
   drives `/exit`, waits for the pane to return to its shell
   (`#{pane_current_command}`), composes BEFORE the kill, kills, recreates the
   session FRESH, and clears the sentinel. No idle inference anywhere.

3. **Fail-safe.** If the sentinel never appears within the timeout, muxr ABORTS
   and leaves the session untouched -- no signal, no `/exit`, no kill. A busy or
   unresponsive session is preserved, never destroyed by a recycle.

4. **The external `/recycle` skill is retired.** This reverses ADR 0008's
   decision [g] ("keep both the skill and the CLI verb"): the skill is superseded
   by muxr owning the mechanism + `muxr skill` documenting it. Triggers: the
   operator runs `muxr recycle <session>` from the control shell or `muxr switch`
   -> `c`; to recycle the session you are inside, run `setsid muxr recycle
   <session> &` (documented in `muxr skill`) so it survives your own `/exit`.
   No runtime-specific skill file required for any path.

## Consequences

- **Recycle is self-contained and runtime-agnostic.** It works out of the box
  with zero estate skill; the estate customizes exactly one seam
  (`[recycle].flush_prompt` -> `durable`).
- **The sentinel is AGENT-written** (its original ADR 0008 role: the positive
  flush-done signal), not muxr-written. `recycle_sentinel_path` /
  `clear_recycle_sentinel` / the `cmd_open` stale-clear (a leftover flag from an
  interrupted recycle, logged + cleared, never auto-reopened) are unchanged.
- **This does NOT reopen the inference wound.** The 3.6.x churn was muxr
  INFERRING flush-done from idle bytes; here the agent SIGNALS it positively and
  muxr waits for the signal. That distinction is the whole ADR 0008 lesson, so
  muxr-owned-flush realizes 0008's "the actor that KNOWS emits it positively"
  more fully -- by making muxr own the prompt rather than depending on an
  external skill. It supersedes ADR 0008's Design-detail mechanics (the
  pane-exited hook, already amended to a detached watcher on 2026-07-19), not its
  removal decisions.
- **The old flush-puppet failure modes are closed:** unsubmitted flush (fixed by
  3.5.2 `send_text`: literal body + separate Enter) and idle inference (replaced
  by the positive sentinel gate).

## Design detail

```
operator: muxr recycle <session>   (control shell / switch->c / detached from in-session)
  muxr : send_text(flush_prompt)   (DEFAULT_FLUSH_PROMPT or [recycle].flush_prompt)
  agent: flush to log.md pointer (+ durable, if the override composes it)
       : echo done > ~/.local/state/muxr/recycle-<slug>.flag   (positive done-signal)
  muxr : wait_for_sentinel(flag, 1200s)  -- appears -> proceed; timeout -> ABORT (session untouched)
  muxr : send_text("/exit") -> wait_for_return_to_shell -> compose_launch_command
       : kill_session -> create_session (fresh) -> clear sentinel
```

Validated by a three-leg live-sim (2026-07-19, isolated tmux server + stub tool
that writes the sentinel on receiving the flush prompt, real sessions
untouched): normal recycle (flush prompt -> sentinel -> fresh reopen -> cleared);
flush-timeout (no sentinel -> abort, session preserved); interrupted watcher
(stale sentinel logged + cleared at the next open).

## Alternatives considered

- **Keep the flush in an external estate skill (the P1 interim).** Not
  self-contained; per-runtime; drifts from muxr's own emitted skill. Rejected --
  muxr already owns the layout + emits its skill.
- **Bundle the skill file inside muxr (`muxr skill install`).** Lighter, but
  still a per-runtime skill mechanism and keeps the flush in a skill rather than
  in muxr. Rejected in favour of muxr sending the flush prompt directly (works on
  any runtime via `send_text`).
- **Have muxr compose the whole flush itself (no agent).** Impossible in spirit
  and against ADR 0008: the flush is inherently in-context work only the agent
  can do; muxr's job is to prompt it and wait for the positive signal.
