#!/usr/bin/env bash
# Example MAKE-DURABLE extension for muxr.
#
# Wired via:  [extensions]  make_durable = "/path/to/make-durable.sh"
#
# Fired just before muxr recycles or closes a session. It returns the agent-
# facing FLUSH MESSAGE -- the prompt muxr types into the live pane telling the
# runtime to serialize whatever must survive the restart (write the worklog,
# commit, persist scratch state) before it exits.
#
# Contract:
#   stdin  (context): {"session","repo","campaign","runtime"}
#   stdout (message): {"message": "<text to send to the agent>"}
#
# Notes:
#   * muxr ALWAYS appends its own exit directive after your message, so you must
#     NOT include "/exit" yourself -- and a message that forgets to exit can't
#     hang recycle.
#   * Return an empty message ({"message": ""}) to skip the flush entirely.
#   * Absent extension -> muxr uses its built-in recycle-flush prompt.
#
# Requires `jq`.

set -euo pipefail

ctx="$(cat)"
campaign="$(jq -r '.campaign // "this session"' <<<"$ctx")"

read -r -d '' msg <<EOF || true
Before we recycle: flush your durable state for ${campaign}. Update the
campaign worklog with where we are and the next concrete step, commit anything
worth keeping, and note any in-flight context you'll want on resume. Then stop.
EOF

jq -nc --arg m "$msg" '{message: $m}'
