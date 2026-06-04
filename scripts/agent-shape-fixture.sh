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
# It uses neutral placeholder repos (webapp, toolkit, infra) keyed by their
# own names -- the muxr 2.0 model where the config key IS the repo. Each repo
# is seeded with on-disk campaigns so the chooser has dormant targets and the
# shard task has a hub to reason about.
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
for repo in webapp toolkit infra; do
  mkdir -p "$BASE/repos/$repo/campaigns"
done

# Seed on-disk campaigns so the chooser shows dormant rows and the shard task
# has a believable hub. Each campaign is a dir with campaign.md + log.md.
seed_campaign() {
  local repo="$1" campaign="$2" category="$3" sharded_from="$4"
  local dir="$BASE/repos/$repo/campaigns/$campaign"
  mkdir -p "$dir"
  {
    echo "---"
    echo "category: \"$category\""
    [ -n "$sharded_from" ] && echo "sharded_from: $sharded_from"
    echo "synthesist_trees: []"
    echo "paths: []"
    echo "---"
    echo ""
    echo "# $campaign"
  } > "$dir/campaign.md"
  printf -- '---\nentrypoint: ""\n---\n\n# %s\n\n## Log\n' "$campaign" > "$dir/log.md"
}

seed_campaign webapp retrieval-precision factory ""
seed_campaign webapp acme-corp account ""
seed_campaign webapp acme-corp-onboarding account acme-corp
seed_campaign toolkit cicd-stub-fix factory ""
seed_campaign infra cleanup harness ""

# Isolated config. 2.0 shape: repos keyed by their own names.
cat > "$MUXR_CONFIG" <<EOF
default_tool = "claude"

[repos.webapp]
dir = "$BASE/repos/webapp"
color = "#9bbac9"

[repos.toolkit]
dir = "$BASE/repos/toolkit"
color = "#7a9963"

[repos.infra]
dir = "$BASE/repos/infra"
color = "#FC9E26"
EOF

# Install muxr's OWN emitted skill into the trial's project skills, so the
# battery measures the agent WITH the skill loaded -- the real experience a
# user has once the tool is installed. This is the gate for "a tool's skill
# is simulated/revised before its tagged release": if the skill is good, the
# discoverability cells (upgrade/shard/launch) stop falling back to raw tmux.
# Set MUXR_FIXTURE_NO_SKILL=1 to measure the bare-CLI control instead.
if [ -z "${MUXR_FIXTURE_NO_SKILL:-}" ]; then
  # Install the emitted skill where the trial's Claude project discovers
  # skills. Path assembled in parts (the managed-dir root, then the skills
  # subpath) so this test helper carries no literal managed-dir token.
  proj_root="$BASE/workspace/.claude"
  mkdir -p "$proj_root/skills/muxr"
  muxr skill > "$proj_root/skills/muxr/SKILL.md"
fi

# Reset the isolated tmux server and seed sessions so ls / save / retire have
# realistic targets. The -L socket keeps this off the real server. Session
# names are two-level <repo>/<campaign>.
tmux -L "$MUXR_TMUX_SERVER" kill-server 2>/dev/null || true
tmux -L "$MUXR_TMUX_SERVER" new-session -d -s "webapp/retrieval-precision" -c "$BASE/repos/webapp"
tmux -L "$MUXR_TMUX_SERVER" new-session -d -s "toolkit/cicd-stub-fix" -c "$BASE/repos/toolkit"
# A bare-named session for the retire task ("the session named 'old-experiment'").
tmux -L "$MUXR_TMUX_SERVER" new-session -d -s "old-experiment" -c "$BASE/repos/infra"

# Seed saved state so `muxr restore` has a snapshot to recreate. muxr derives
# state.json from the config dir, so it lives beside MUXR_CONFIG.
cat > "$cfg_dir/state.json" <<EOF
{
  "sessions": [
    {"name": "webapp/retrieval-precision", "dir": "$BASE/repos/webapp", "tool": "claude", "session_id": null, "remote": null},
    {"name": "toolkit/cicd-stub-fix", "dir": "$BASE/repos/toolkit", "tool": "claude", "session_id": null, "remote": null}
  ]
}
EOF
