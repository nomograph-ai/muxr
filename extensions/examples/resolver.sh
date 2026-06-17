#!/usr/bin/env bash
# Example RESOLVER extension for muxr.
#
# Wired via:  [extensions]  resolver = "/path/to/resolver.sh"
#
# The resolver is muxr's single launch chokepoint. muxr runs this command with
# the launch INTENT as JSON on stdin, and reads layout FACTS as JSON on stdout.
# Any field you omit falls back to muxr's built-in `[layout]` default, so a
# resolver can override just one thing (e.g. resume_id) and inherit the rest.
#
# Contract (fail-closed: a non-zero exit or unparseable stdout aborts launch):
#
#   stdin  (intent):  {"session","repo","campaign","resume_id","model"}
#   stdout (facts):   {"dir","campaign_md","log_path","runtime","add_dirs","resume_id"}
#
# This example shows the opencode case from adapters/opencode.toml: opencode has
# no per-pid session file, so we resolve resume_id by querying `opencode session`
# and let everything else fall through to muxr's defaults (emit only resume_id).
#
# Requires `jq`. Replace the body with your own mapping.

set -euo pipefail

intent="$(cat)"
session="$(jq -r '.session // ""' <<<"$intent")"
runtime="$(jq -r '.runtime // ""' <<<"$intent")"

# Default: pass the intent's resume_id straight through (no override).
resume_id="$(jq -r '.resume_id // ""' <<<"$intent")"

# opencode-only: map this tmux session to its opencode session id. (Illustrative
# -- adjust the `opencode session` parsing to your version's output.)
if [[ "$runtime" == "opencode" ]]; then
  resume_id="$(opencode session list --json 2>/dev/null \
    | jq -r --arg s "$session" '.[] | select(.title == $s) | .id' \
    | head -n1 || true)"
fi

# Emit ONLY the field we want to override; muxr fills dir/campaign_md/log_path/
# runtime/add_dirs from its built-in [layout].
jq -nc --arg rid "$resume_id" '{resume_id: $rid}'
