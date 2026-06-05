![muxr hero](hero.svg)

# muxr

[![pipeline](https://gitlab.com/nomograph/muxr/badges/main/pipeline.svg)](https://gitlab.com/nomograph/muxr/-/pipelines)
[![license](https://img.shields.io/badge/license-MIT-green)](LICENSE)
[![built with GitLab](https://img.shields.io/badge/built_with-GitLab-FC6D26?logo=gitlab)](https://gitlab.com/nomograph/muxr)
[![crates.io](https://img.shields.io/crates/v/nomograph-muxr.svg)](https://crates.io/crates/nomograph-muxr)

Harness session multiplexer for AI coding workflows. Owns the address
`<repo>/<campaign>` across tmux, the filesystem, and your AI runtime, so
a rename or move stays coherent in all three.

## What it is

muxr is a tmux + AI runtime session manager. Sessions live as
typed records on disk; tmux state and the runtime command stay in
lockstep with that record. `muxr save` snapshots; `muxr restore`
brings everything back.

## The problem

You work across multiple projects. Each one needs its own terminal
session with the right working directory and the right tool running.
You open tabs, cd around, lose track of what is where. When you
reboot, everything is gone.

muxr organizes tmux sessions into repos (local directory trees) and
remotes (GCE instances). Each session knows where it lives and what tool to
run. `muxr save` snapshots everything. `muxr restore` brings it back.

## Install

```bash
cargo install nomograph-muxr
```

Pre-built binaries for macOS arm64 and Linux amd64 are available in
[releases](https://gitlab.com/nomograph/muxr/-/releases).

## Quick start

```bash
muxr init                                # create config
muxr work                                # open the work repo switchboard
muxr work retrieval-precision            # open work/retrieval-precision campaign session
muxr switch                              # interactive chooser to jump or launch
muxr save                                # snapshot before reboot
muxr restore                             # bring everything back
```

Sessions are addressed as `<repo>/<campaign>` -- two levels. Campaigns are
kebab-case and name the initiative (e.g. `cicd-stub-fix`,
`retrieval-precision`); they are not date-stamped. The per-repo switchboard
is a singleton at `<repo>/switchboard`. When a topic crystallizes inside a
broad campaign, `muxr shard <new>` spins it out into a sibling campaign
instead of adding a third name level.

## Config

One file: `~/.config/muxr/config.toml`

```toml
default_tool = "claude"

[repos.work]
dir = "~/projects/work"
color = "#7aa2f7"

[repos.work.launch]
append_system_prompt_file = "HARNESS.md"   # optional repo-level prompt
add_dirs = ["~/docs/shared"]               # optional base --add-dir paths

[repos.personal]
dir = "~/projects/personal"
color = "#9ece6a"

[remotes.lab]
project = "my-gce-project"
zone = "us-central1-a"
user = "deploy"
color = "#d29922"
# connect = "mosh"       # default; set to "ssh" for gcloud compute ssh
# instance_prefix = ""   # optional prefix for GCE instance names
```

Repos are local directory trees; the table key is the name you pass as the
first argument to `muxr`. Remotes are GCE instances resolved via `gcloud`.
Each gets a color that shows up in the chooser and the tmux status bar.
Remotes require the [gcloud CLI](https://cloud.google.com/sdk/docs/install).

## How sessions work

muxr is a thin layer over tmux. Each session gets a named tmux session,
the right working directory, and your default tool running.

```
muxr work retrieval-precision
  tmux new-session -s "work/retrieval-precision" -c ~/projects/work
  tmux send-keys "claude" Enter
  tmux attach -t "work/retrieval-precision"
```

Session names follow the pattern `<repo>/<campaign>`. The campaign is
mandatory and validated as kebab-case. Sessions persist across terminal
restarts because tmux keeps them alive.

## The chooser

`muxr switch` opens an interactive chooser that merges everything you can
act on into one list, grouped by repo:

- **live sessions** -- Enter attaches.
- **dormant campaigns** (on disk, not running) -- Enter launches them, so
  every campaign is visible at a glance, not just the running ones.
- **`+ new campaign…`** per repo -- Enter prompts for a slug and creates it.

Shards render indented under their hub. Remote sessions appear alongside
local ones.

| Key | Action |
|-----|--------|
| `j` / `k` | Navigate |
| `Enter` | Switch / open dormant / create (context-sensitive) |
| `n` | New campaign in the selected repo |
| `r` | Rename a live session |
| `d` | Kill a live session (with confirmation) |
| `/` | Fuzzy filter |
| `q` | Quit |

Bind it in tmux for instant access:

```tmux
bind s display-popup -E -w '80%' -h '80%' "muxr switch"
```

## Remote sessions

Remote sessions create a local tmux proxy that connects to a GCE
instance and attaches to the remote tmux. The default connect method is
mosh; set `connect = "ssh"` for gcloud compute ssh. Sessions appear in
`muxr ls` and the switcher alongside local sessions.

```bash
muxr lab trustchain             # connect to remote, attach tmux
muxr lab ls                     # list remote sessions
```

Connections auto-reconnect on drops with exponential backoff.

## The system prompt is a pointer

muxr composes the launch prompt as repo HARNESS rules + the campaign's
what/how + a **pointer**: the one-line `entrypoint` plus the absolute paths
of `campaign.md`/`log.md` and an instruction to re-read them. It does not
inline the growing log body -- a fat prompt is resent every turn (burning the
context window) and goes stale the moment you `/serialize`.

Because the system prompt survives `/compact`, the re-read instruction
survives too, so the durable source of truth stays the on-disk files. After a
compaction, re-anchor from disk instead of the lossy summary:

```bash
muxr reorient                   # nudge the current session to re-read its files
muxr reorient work/api          # or a named session
```

Keep `log.md`'s `entrypoint` a tight "where we are / what's next" line --
that's the pointer you move as work advances.

## Save and restore

```bash
muxr save                       # snapshot all sessions to JSON
muxr restore                    # recreate after reboot
```

Restore recreates local sessions with the correct directory and tool,
rebuilding each session's full launch command (system prompt + working
dirs + resume) so a restored session is identical to a freshly opened one.
Remote sessions re-establish connections.

## Upgrading running sessions

```bash
muxr upgrade                    # move every claude session onto the
                                # freshly installed binary, in place
muxr upgrade --dry-run          # show what would happen, touch nothing
muxr upgrade work/retrieval-precision  # upgrade just one session
muxr upgrade --model opus-4-8   # also switch model on relaunch
```

For each target muxr discovers the session id, sends a graceful `/exit`,
waits for the harness to quit, then relaunches it with the full composed
command and `--resume`. Because the binary name is resolved fresh, the
relaunch picks up a newly installed harness version (e.g. a new Claude
Code release) without losing the conversation, harness rules, or working
directories. Run it from the `muxr` control shell, not from inside an
agent session.

## Commands

| Command | What it does |
|---------|-------------|
| `muxr` | Control plane (bare shell) |
| `muxr <repo>` | Open the repo switchboard singleton |
| `muxr <repo> <campaign>` | Open or attach to a campaign session |
| `muxr <remote> [context...]` | Create or attach to a remote session |
| `muxr switch` | Interactive chooser: switch / open dormant / create |
| `muxr shard <new>` | Spin a topic out of the current campaign into a sibling (`--repo`, `--from`) |
| `muxr reorient [name]` | Nudge a live session to re-read its campaign.md + log.md (use after `/compact`) |
| `muxr ls` | List active sessions |
| `muxr save` | Snapshot session state |
| `muxr restore` | Recreate sessions after reboot |
| `muxr kill <name>` | Kill a session |
| `muxr kill all` | Kill all sessions |
| `muxr retire <name>\|all` | Graceful `/exit` + kill; drops from saved state |
| `muxr upgrade [name]` | Relaunch sessions in place on the new binary (`--dry-run`, `--tool`, `--model`) |
| `muxr broadcast [/cmd]` | Send a slash command to every harness session |
| `muxr rename <name>` | Rename: tmux + session file on disk + runtime relink |
| `muxr migrate-layout <repo>` | Migrate a repo's `campaigns/` tree to the 2-level model (`--dry-run`, `--keep-archives`) |
| `muxr init` | Create default config |
| `muxr completions <shell>` | Shell completions (zsh, bash, fish) |
| `muxr tmux-status` | tmux status bar integration |

## License

MIT

---

Part of [Nomograph Labs](https://nomograph.ai).
