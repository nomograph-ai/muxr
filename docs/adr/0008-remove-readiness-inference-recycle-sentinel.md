# ADR 0008: Remove readiness inference; recycle via a positive sentinel handshake

- Status: Accepted
- Date: 2026-07-17
- Deciders: operator + design review (Fable pass, Opus second pass)
- Supersedes: [ADR 0002](0002-readiness-gated-upgrade.md), [ADR 0003](0003-reclaim-interrupted-sessions.md), [ADR 0007](0007-interrupt-reclaim-in-core.md)
- Relates to: [ADR 0001](0001-extension-architecture.md), [ADR 0004](0004-companion-pane.md), nomograph/muxr#12
- Implemented in: 4.0.0 (breaking)

> **Amended 2026-07-19 (implementation):** the reopen watcher is a DETACHED
> `muxr recycle` process, NOT a tmux `pane-exited` hook -- the hook cannot fire
> under muxr's shell-wrapped launch model. The sentinel becomes a crash
> breadcrumb rather than the reopen trigger. See "## Update" below; it revises
> Decision 2/3 and the Design detail. The removal decisions (1, 4, 5) are
> unchanged.

## Context

muxr's two live-session operations -- `upgrade` (relaunch in place with
`--resume`) and `recycle` (flush -> exit -> reopen fresh from a pointer) -- both
need to answer one question: *is this session safe to act on right now?* An
`upgrade` must not interrupt a turn mid-generation; a `recycle` must not exit
before its flush is written.

ADR 0002/0003/0007 answered that question by INFERRING idle-vs-busy from outside
the runtime: a `busy`/`idle` state file the runtime's hooks maintain,
corroborated against a tmux activity timestamp, with a universal activity floor.
The inference was forced by a structural gap: **Claude Code's hook surface has no
"interrupted" event.** An interrupted turn fires no `Stop` hook, so the `busy`
flag is never cleared, and muxr can only GUESS from tmux bytes whether a `busy`
session is working or stranded.

That guess was the single largest source of churn in muxr's history: `state.rs`
29 commits, `tmux.rs` 20, ten releases touching the family, and the 3.6.2 ->
3.6.3 same-day near-miss where the classifier read `session_activity` (last
CLIENT interaction, frozen while an unattended agent streams) and would have
reclaimed -- killed -- a live-but-unattended turn. 3.6.3 switched to
`window_activity` (pane output), which is better but still cannot distinguish a
genuinely long-quiet turn from an interrupted one, and a companion pane's output
muddies it further. Every refinement moved the failure; none closed it. **The
state is not observable from outside the runtime, so every inference of it is a
heuristic with a live-session-killing failure mode.**

Two things changed the calculus:

1. **The viewer (ADR 0004, recast).** A review surface beside every session is
   why recycle's blind flush-and-reopen mattered as a way to glance at work
   before resetting. The viewer makes the glance first-class, so recycle no
   longer has to infer a safe moment to reset -- the operator picks it.
2. **The operator reframe.** Recycle is not cruft to minimize; it is the PRIMARY
   token-burn lever (every turn re-sends the whole conversation, so cost grows
   with history; recycle resets the context window to ~the composed system
   prompt + pointer). It must run reliably and OFTEN. The #8/#10 flakiness came
   from muxr PUPPETING the TUI -- inferring idle, then send-keys-flushing a
   message that never submitted -> hang -> SIGKILL.

## Decision

**Stop inferring an unobservable state. Flip ownership so the actor emits a
positive signal, and remove the inference machinery entirely.**

1. **Remove readiness inference.** Delete the classifier (`session_readiness`,
   `classify_state_file`, `corroborate_busy`, the activity floor), the
   `ReadinessProbe`/`File`/`Command` extension surface and every
   `[tools.*.readiness]` adapter block, and the `muxr status` command. This
   supersedes ADR 0002, 0003, and 0007.

2. **Recycle becomes a positive sentinel handshake with an agent-owned flush.**
   The in-session `/recycle` skill (operator estate) FLUSHES working state to the
   durable pointer (via `durable`), verifies the pointer is cold-start-complete,
   and writes a per-session SENTINEL file as an explicit "reopen me" signal -- a
   POSITIVE done-signal, not an inferred idle. muxr then drives the `/exit`
   keystroke (an interactive agent cannot self-`/exit` -- the CLI input loop owns
   that key; muxr's external `send-keys` is what works, the lesson from the
   600s->SIGKILL incident), and a tmux `pane-exited` hook installed at
   `create_session` fires on the CLI exit and reopens fresh via
   `compose_launch_command` -- but ONLY if the sentinel is set. muxr deletes the
   sentinel on reopen. No idle inference anywhere: muxr waits for the ACTUAL
   exit, not a guessed-safe moment.

3. **Clean exit without a sentinel does not reopen.** A crash, OOM, or deliberate
   `/exit` just ends -- require-sentinel, so muxr can never spuriously resurrect a
   session the operator meant to close. `retire` tears down the pane-exited hook
   before exit so retiring never triggers a reopen. A stale sentinel found at
   next launch is logged and cleared.

4. **Upgrade is de-gated to a human-confirmed listing.** No readiness gate.
   `upgrade` prints the sessions it will relaunch with a display-only quiet-age
   column (kept from `window_activity`, now purely informational) and the human
   confirms. `--force` skips the confirm (scripting/CI); `--wait` and
   `--min-idle` are removed (they existed only to drive the deleted gate). The
   self-upgrade guard stays.

5. **`window_activity` survives as DISPLAY only.** `output_activity` /
   `list_sessions_detailed` keep exposing "seconds since last pane output" for the
   switcher recency sort and the upgrade quiet-age column. It is never again a
   gate.

## Update (2026-07-19): implemented as a detached watcher, not a pane-exited hook

Implementing Decision 2 surfaced a hard constraint. `create_session` launches the
tool via `send-keys "<tool_cmd>" Enter` INTO a persistent shell (`tmux.rs`), so
the pane's process is the SHELL, and the tool's `/exit` returns the pane to that
shell rather than ending the pane. A tmux `pane-exited`/`pane-died` hook therefore
NEVER FIRES on a recycle -- the pane never dies. The pane-exited-hook watcher in
Decision 2 and the Design-detail diagram is not buildable.

What shipped instead is the "Detached reopen process" listed under Alternatives
(promoted from rejected-fallback to primary):

- The in-session `/recycle` skill spawns a DETACHED `setsid muxr recycle
  <session>` that survives the agent's own `/exit`, then STOPS. That process IS
  the watcher -- there is no hook and no muxr daemon.
- `muxr recycle` (the detached process, or the operator's manual CLI/switcher
  call -- decision [g]) does the whole handshake in one process: write the
  sentinel -> send-keys `/exit` -> wait for the pane to return to a known shell
  via `#{pane_current_command}` (the exit-detect from Decision's Design detail,
  which is what the return-to-shell poll always was) -> `compose_launch_command`
  BEFORE the kill -> kill -> `create_session` fresh -> clear the sentinel.

The sentinel's ROLE shifts accordingly. It is no longer the reopen TRIGGER (the
detached process reopens unconditionally, as its job); it is a crash BREADCRUMB.
Written before the destructive exit and removed after a successful reopen, a
leftover flag means the watcher died mid-flight (sleep, SIGKILL, orphan reaped);
the next `muxr open` logs it and clears it. It is NEVER used to auto-reopen. This
preserves Decision 3's intent exactly: without an explicit `muxr recycle` nothing
reopens (a genuine `/exit`/crash has no watcher), and muxr never resurrects a
session from a stale flag. `retire` needs no hook teardown -- there is no hook.

Validated by a three-leg live-sim (2026-07-19, isolated tmux server + stub tool,
recorded in the `/recycle` skill's `sims/`): (A) normal recycle writes the
sentinel mid-flight, drives `/exit`, reopens fresh, clears the sentinel; (B) a
hand-planted stale sentinel + dead session is logged + cleared at the next open,
opening once with no reopen loop; (C) a direct `/exit` (muxr uninvolved) leaves
the pane at its shell and never reopens, no sentinel written.

## Consequences

- **The largest churn source is deleted, not re-patched.** ~600 lines of
  classifier + probe surface go; the remaining ~15-line `window_activity` read is
  display-only and cannot kill a session.
- **Recycle is strengthened, not removed.** It stays muxr's irreducible-job cost
  lever (ADR 0001); the fragile puppeting is replaced by a handshake where the
  agent owns flush+signal and muxr owns exit-detect+reopen.
- **The general lesson (the reason this ADR exists):** do not infer a state the
  system cannot observe. When a needed signal is unobservable from outside,
  redesign so the actor that KNOWS emits it positively. Inference in *either*
  layer (core classifier or opt-in probe) was unsafe because the signal itself
  was unknowable, not because the extension seam was wrong.
- **Non-goals:** this does NOT remove the `/durable` skill (manual flush is
  unchanged; #10 showed the auto-flush never reliably worked). It does NOT remove
  `recycle` (it is the cost lever), the switcher Recycle action, or
  `window_activity` (display).
- **Breaking:** removed config surface (`[tools.*.readiness]`, `[readiness]`) and
  the removed `muxr status` command make this a major bump. Old configs carrying
  those keys must be migrated (`muxr config migrate`); a hard cross-machine gate
  (every machine >= 3.7.0 before any v4 fragment field) applies -- see the v4
  design brief.
- ADR 0002/0003/0007 move to `Superseded by [ADR 0008]`; the trail stays.

## Design detail

### Sentinel handshake

```
operator: /recycle  (in-session skill)
  skill: durable-flush -> verify pointer cold-start-complete
       -> write ~/.local/state/muxr/recycle-<session>.flag   (positive done-signal)
  muxr : send-keys "/exit"  (external; the agent cannot self-exit)
  CLI  : exits -> pane returns to shell
  tmux : pane-exited hook (installed at create_session) fires -> muxr reopen entry
  muxr : sentinel set?  yes -> compose_launch_command + create_session (fresh) + rm sentinel
                        no  -> nothing (genuine quit / crash)
```

- **Watcher = tmux `pane-exited` hook**, installed at `create_session`
  (`tmux.rs`). Event-driven, tmux-owned, survives the CLI teardown -- no muxr
  daemon.
- **Exit-detect** (where a hook is unavailable) = return-to-shell via
  `#{pane_current_command}` in a known-shell set (robust to the pi `nono`
  wrapper).
- **Sentinel** = a per-session file under the relocated state dir
  `~/.local/state/muxr/` (state, not config). Written by the skill, deleted by
  muxr on reopen, log+cleared if stale.
- **Both trigger paths** (`/recycle` skill and `muxr recycle` CLI + switcher
  Recycle) write the sentinel and share the hook+reopen; the CLI stays for
  scripting and recovery.

### Removed surface

`session_readiness` / `classify_state_file` / `corroborate_busy` / the activity
floor and consts (`state.rs`); `ReadinessProbe` / `ReadinessConfig` / `FileProbe`
/ `CommandProbe` (`config.rs`); every `[tools.*.readiness]` block in embedded
adapters (lockstep: `builtin_adapters()` `.expect` panics if the Rust field is
gone but the embedded TOML still declares it, so both move in one commit);
`cmd_status` + the `Status` command variant (`main.rs`); `upgrade`'s
`--wait`/`--min-idle` and the gate call.

## Alternatives considered

- **Keep refining the heuristic.** Ten releases and a near-fatal near-miss are
  the evidence: the signal is unobservable from outside the runtime, so no
  refinement closes the gap. Rejected.
- **Wait for a Claude Code "interrupted" hook event.** Not in the hook surface,
  and muxr must stay runtime-agnostic -- it cannot depend on one runtime growing
  one event. Rejected.
- **Detached reopen process** (the skill spawns a `nohup`/`setsid` `muxr reopen`
  that outlives the CLI). Works, but must survive pane teardown and races the
  exit -- fragile. The tmux `pane-exited` hook is tmux-owned and strictly
  simpler. Rejected as the primary; kept as a fallback note.
- **Reopen on any clean exit (no sentinel).** Then muxr could never truly end a
  session and a crash would resurrect one. Rejected for require-sentinel.
