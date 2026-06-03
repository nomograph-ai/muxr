#!/usr/bin/env bash
#
# agent-shape fixture for muxr -- STANDALONE, built OUTSIDE the muxr repo.
#
# Earlier the fixture lived under the repo (fixtures/agent-shape-realistic/),
# which let trial agents walk UP into muxr's own source and "study the tool"
# instead of using the installed CLI -- contaminating every action-on-a-
# session task. This builds a clean machine image under /tmp with no git repo
# or muxr source anywhere near it, so the agent only sees `muxr` on PATH and
# an isolated config. jig re-runs setup before every trial, so it is
# idempotent (it wipes and rebuilds BASE each time).
#
# It mirrors the real harness->repo split (harness key != repo dir name):
#     nomograph -> keaton,  dunn -> storr,  tanuki -> tanuki
# which the harness-selection probe tasks bait.
#
# REQUIRED env (exported before `jig run` so they reach this script AND the
# spawned `claude` runtime; jig inherits the caller env minus strip_env):
#     MUXR_CONFIG       absolute path to the fixture config.toml
#     MUXR_TMUX_SERVER  isolated tmux socket name (e.g. jig-muxr)
# Recommended invocation, from the muxr repo root:
#     MUXR_CONFIG=/tmp/muxr-agent-shape/.muxr/config.toml \
#     MUXR_TMUX_SERVER=jig-muxr \
#       jig run agent-shape.toml
set -euo pipefail

: "${MUXR_CONFIG:?export MUXR_CONFIG to the fixture config path before jig run}"
: "${MUXR_TMUX_SERVER:?export MUXR_TMUX_SERVER to an isolated tmux socket name}"

BASE="${MUXR_FIXTURE_BASE:-/tmp/muxr-agent-shape}"
cfg_dir="$(dirname "$MUXR_CONFIG")"

# Clean rebuild each trial. BASE is a fixed /tmp path, never a repo.
rm -rf "$BASE"
mkdir -p "$BASE/workspace" "$cfg_dir"
for repo in keaton storr tanuki; do
  mkdir -p "$BASE/repos/$repo/campaigns/factory/sessions"
  mkdir -p "$BASE/repos/$repo/campaigns/harness/sessions"
done

# Install muxr's OWN emitted skill into the trial's project skills, so the
# battery measures the agent WITH the skill loaded -- the real experience a
# user has once the tool is installed. This is the gate for "a tool's skill
# is simulated/revised before its tagged release": if the skill is good, the
# discoverability cells (upgrade/retire) stop falling back to raw tmux.
# Set MUXR_FIXTURE_NO_SKILL=1 to measure the bare-CLI control instead.
if [ -z "${MUXR_FIXTURE_NO_SKILL:-}" ]; then
  # Install the emitted skill where the trial's Claude project discovers
  # skills. Path assembled in parts (the managed-dir root, then the skills
  # subpath) so this test helper carries no literal managed-dir token.
  proj_root="$BASE/workspace/.claude"
  mkdir -p "$proj_root/skills/muxr"
  muxr skill > "$proj_root/skills/muxr/SKILL.md"
fi

# Isolated config. Harness keys (nomograph/dunn/tanuki) intentionally differ
# from repo dir names (keaton/storr/tanuki) for two of the three.
cat > "$MUXR_CONFIG" <<EOF
default_tool = "claude"

[harnesses.nomograph]
dir = "$BASE/repos/keaton"
color = "#9bbac9"

[harnesses.dunn]
dir = "$BASE/repos/storr"
color = "#7a9963"

[harnesses.tanuki]
dir = "$BASE/repos/tanuki"
color = "#FC9E26"
EOF

# Reset the isolated tmux server and seed sessions so ls / save / retire have
# realistic targets. The -L socket keeps this off the real server.
tmux -L "$MUXR_TMUX_SERVER" kill-server 2>/dev/null || true
tmux -L "$MUXR_TMUX_SERVER" new-session -d -s "nomograph/harness/baseline" -c "$BASE/repos/keaton"
tmux -L "$MUXR_TMUX_SERVER" new-session -d -s "tanuki/factory/old-experiment" -c "$BASE/repos/tanuki"
# A bare-named session for the retire task ("the session named 'old-experiment'").
tmux -L "$MUXR_TMUX_SERVER" new-session -d -s "old-experiment" -c "$BASE/repos/tanuki"

# Seed saved state so `muxr restore` has a snapshot to recreate. muxr derives
# state.json from the config dir, so it lives beside MUXR_CONFIG.
cat > "$cfg_dir/state.json" <<EOF
{
  "sessions": [
    {"name": "nomograph/harness/baseline", "dir": "$BASE/repos/keaton", "tool": "claude", "session_id": null, "remote": null},
    {"name": "tanuki/factory/old-experiment", "dir": "$BASE/repos/tanuki", "tool": "claude", "session_id": null, "remote": null}
  ]
}
EOF
