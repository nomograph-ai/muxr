# ADR 0006: Fail loud on present-but-unparseable session files

- Status: Accepted
- Date: 2026-07-07
- Implemented in: 3.6.1

## Context

`compose_launch_command` rebuilds a session's launch (composed HARNESS prompt +
campaign `--add-dir` paths + resume) from `campaign.md` and `log.md`. Both were
loaded best-effort and every failure collapsed to an empty default via
`unwrap_or_default`. A MISSING file and a PRESENT-BUT-UNPARSEABLE file therefore
produced the same result: an empty entrypoint and no campaign paths.

Degrading on a missing file is intentional -- an archived-but-still-running
session legitimately has no log body, and the relaunch must still carry the
repo-level prompt and resume. Degrading silently on a file that exists but fails
to parse is a bug: a single unescaped inner `"` inside a double-quoted
`entrypoint:` scalar made a `log.md` unparseable, and the next recycle/upgrade
relaunched the live session stripped of its composed prompt and every
`--add-dir` path -- booting healthy on the surface, with no error surfaced. This
was the highest-impact of the recycle papercuts (a one-character typo silently
de-fangs a live session) and is inconsistent with muxr's own resolver-extension
contract, which is deliberately fail-closed-and-loud.

## Decision

Distinguish ABSENT from PRESENT-BUT-UNPARSEABLE. A new `primitives::load_optional`
returns `Ok(None)` when the file does not exist (degrade), `Ok(Some(..))` when it
parses, and `Err` when it exists but cannot be read or parsed. Each relaunch path
then handles the error without silently degrading:

- **recycle**: pre-flight validates `campaign.md` + `log.md` BEFORE the
  destructive flush/exit/kill, so a broken file refuses the recycle and leaves
  the live session running untouched.
- **upgrade**: composes BEFORE sending the exit and SKIPS the session loud on a
  parse error, rather than exiting it and relaunching name+resume only.
- **restore**: surfaces the error loud and SKIPS that session rather than
  bringing it back stripped of its rules.
- **open** (fresh launch): already propagated the error; unchanged.

## Consequences

- A corrupt `campaign.md`/`log.md` is caught at the file that is broken, named in
  the error, and never silently strips a live session.
- Missing files still degrade cleanly: the archived-but-running case is
  preserved (recycle/upgrade/restore all still relaunch via resume/`--continue`).
- `upgrade` composes before the exit, marginally widening the readiness->exit
  TOCTOU window (a few milliseconds of file reads); accepted as the correct
  trade against silently de-fanging a live session, and dwarfed by the
  subsequent wait-for-exit.
- `Tool::restore_command` is removed: restore now routes entirely through the
  shared composer (whose `continue_fallback` already handles the no-session-id
  `--continue` case), so the two paths can no longer diverge.

## Alternatives considered

- **Leave the best-effort degrade, document it.** Rejected: the failure is
  silent and hits live sessions; documentation does not surface it at the moment
  a session is de-fanged.
- **Abort the whole batch on one broken session (upgrade/restore).** Rejected:
  one corrupt session should not block upgrading/restoring the healthy ones;
  skip-loud-and-continue localizes the blast radius.
- **Keep composing after the exit in upgrade (preserve the tight TOCTOU).**
  Rejected: it forces an exit-then-degrade on a parse error, which is the exact
  silent-strip this ADR removes.
