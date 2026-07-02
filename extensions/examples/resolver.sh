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
# This example shows two things:
#   1. REPO-SCOPING. A resolver need not be global. muxr passes the `repo` name
#      in the intent, so branch on it and emit `{}` (no override -> muxr's
#      built-in [layout] default) for any repo this resolver should NOT touch.
#      That keeps a single global [extensions].resolver from imposing one repo's
#      policy on the whole fleet -- no per-repo config field required.
#   2. The opencode case from adapters/opencode.toml: opencode has no per-pid
#      session file, so we resolve resume_id by querying `opencode session` and
#      let everything else fall through to muxr's defaults (emit only resume_id).
#
# PORTABILITY: muxr runs the resolver via `sh -c`, so a leading `~/` in the
# configured path expands. Prefer a home-relative path
# (`resolver = "~/.../resolver.sh"`) over an absolute one so the config is
# portable across machines and usernames.
#
# Requires `jq`. Replace the body with your own mapping.

set -euo pipefail

intent="$(cat)"
session="$(jq -r '.session // ""' <<<"$intent")"
repo="$(jq -r '.repo // ""' <<<"$intent")"
runtime="$(jq -r '.runtime // ""' <<<"$intent")"

# Repo-scoping: opt specific repos OUT of this resolver. For them, emit `{}` so
# muxr uses its built-in [layout] default (main checkout, no worktree, etc.).
# Everything not listed falls through to the logic below.
case "$repo" in
  docs | scratch) echo '{}'; exit 0 ;;
esac

# Default: pass the intent's resume_id straight through (no override).
resume_id="$(jq -r '.resume_id // ""' <<<"$intent")"

# opencode-only: map this tmux session to its opencode session id. (Illustrative
# -- adjust the `opencode session` parsing to your version's output.)
if [[ "$runtime" == "opencode" ]]; then
  resume_id="$(opencode session list --format json 2>/dev/null \
    | jq -r --arg s "$session" '.[] | select(.title == $s) | .id' \
    | head -n1 || true)"
fi

# Emit ONLY the field we want to override; muxr fills dir/campaign_md/log_path/
# runtime/add_dirs from its built-in [layout].
jq -nc --arg rid "$resume_id" '{resume_id: $rid}'
