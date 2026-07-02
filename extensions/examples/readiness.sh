#!/usr/bin/env bash
# Example READINESS probe (Command type) for muxr -- reclaims interrupted-but-idle
# sessions that a plain File probe would strand as "busy".
#
# Wire it as an OPT-IN Command probe (the shipped claude adapter uses a simple
# File probe; this replaces that block):
#
#   [tools.claude.readiness]
#   type = "command"
#   argv = ["/abs/path/to/readiness.sh", "{session_id}", "{pid}"]
#
# NOTE: a Command probe's argv[0] is executed DIRECTLY (not via a shell), so it
# is not tilde-expanded -- give an ABSOLUTE path (or a bare name on PATH).
#
# WHY THIS EXISTS
#   muxr's built-in File probe trusts a `busy` state file until it is older than
#   STALE_BUSY_SECS (1h). But an INTERRUPTED Claude turn fires no `Stop` hook, so
#   the `busy` written by UserPromptSubmit/PreToolUse is never cleared to `idle`.
#   The session then reads Busy("turn in flight") for up to an hour and every
#   readiness-gated `muxr upgrade` skips it -- even though the pane is idle and
#   waiting. This probe does NOT trust `busy` blindly: it corroborates the claim
#   against real tmux pane activity. Busy + quiet pane => stale/interrupted =>
#   reclaim.
#
# CONTRACT (muxr Command probe): exit 0 = Safe (ok to relaunch), non-zero = Busy.
#   A spawn error or a timeout falls through to muxr's own tmux-activity floor;
#   this script mirrors that floor when it has no state file to read, so it is a
#   safe superset of the File probe.
#
# muxr interpolates {session_id} and {pid} (the tmux pane_pid) into argv. We use
# session_id to locate the state file and pane_pid to recover the pane's
# session_activity from tmux.
#
# Dependency-light on purpose (tmux, awk, sed, date, cat -- no jq), matching the
# hook that writes the state file so it runs in a minimal probe environment.

set -eu

session_id="${1:-}"
pane_pid="${2:-}"

# Quiet period (seconds) a session must be idle before it is Safe. A Command
# probe does not receive muxr's own min_idle, so we define our own; override
# with MUXR_READINESS_MIN_IDLE to match your `muxr upgrade` expectations.
MIN_IDLE="${MUXR_READINESS_MIN_IDLE:-45}"

STATE_FILE="$HOME/.config/muxr/readiness/${session_id}.json"
now="$(date +%s)"

# Recover the pane's last tmux activity (epoch) by matching the pane_pid muxr
# passed. session_activity is a per-session value; every pane in the session
# reports it, so matching any pane with this pane_pid yields the right one.
# Empty when tmux is not running or the pane is gone -> "activity unknown".
pane_activity=""
if [ -n "$pane_pid" ]; then
  pane_activity="$(tmux list-panes -a -F '#{pane_pid} #{session_activity}' 2>/dev/null \
    | awk -v p="$pane_pid" '$1 == p { print $2; exit }')"
fi

# True when the pane is provably quiet for at least MIN_IDLE. Unknown activity is
# NOT provably quiet (returns false), so we never reclaim on missing evidence.
pane_quiet() {
  case "$pane_activity" in
    "" | *[!0-9]*) return 1 ;;
  esac
  [ $(( now - pane_activity )) -ge "$MIN_IDLE" ]
}

# Parse the state file (best-effort, sed only).
state=""
since=""
if [ -f "$STATE_FILE" ]; then
  contents="$(cat "$STATE_FILE" 2>/dev/null || true)"
  state="$(printf '%s' "$contents" |
    sed -n 's/.*"state"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
  since="$(printf '%s' "$contents" |
    sed -n 's/.*"since"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p' | head -1)"
fi

case "$state" in
  idle)
    # Safe once the quiet period has elapsed since the idle transition.
    if [ -n "$since" ] && [ "$(( now - since ))" -ge "$MIN_IDLE" ]; then
      exit 0
    fi
    # Missing timestamp or too recent: corroborate against the pane.
    if pane_quiet; then exit 0; fi
    exit 1
    ;;
  busy)
    # THE FIX: an interrupted turn leaves `busy` stuck. If the pane has been
    # quiet for >= MIN_IDLE, no turn is really in flight -> reclaim as Safe.
    if pane_quiet; then exit 0; fi
    exit 1
    ;;
  *)
    # No state file / unparseable: behave exactly like muxr's tmux floor.
    if pane_quiet; then exit 0; fi
    exit 1
    ;;
esac
