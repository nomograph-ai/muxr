![muxr hero](hero.svg)

# muxr

[![Built with GitLab](https://img.shields.io/badge/Built_with-GitLab-FC6D26?logo=gitlab)](https://gitlab.com/dunn.dev/muxr)
[![pipeline](https://gitlab.com/dunn.dev/muxr/badges/main/pipeline.svg)](https://gitlab.com/dunn.dev/muxr/-/pipelines)
[![crates.io](https://img.shields.io/crates/v/muxr.svg)](https://crates.io/crates/muxr)
[![license](https://img.shields.io/badge/license-MIT-green)](LICENSE)

**Tmux session manager for AI coding workflows.** Named sessions,
vertical-aware directories, save/restore across reboots, remote proxy
sessions, TUI switcher. Works with any terminal tool (claude, vim, shell).

## Install

```bash
# From crates.io
cargo install muxr

# Pre-built binaries (macOS arm64, Linux amd64)
# See https://gitlab.com/dunn.dev/muxr/-/releases
```

## Quick start

```bash
# Create config with your verticals
muxr init
# Edit ~/.config/muxr/config.toml

# Control plane (bare shell for managing sessions)
muxr

# Open a session
muxr work                   # work/default
muxr work api               # work/api
muxr work api auth          # work/api/auth

# Create sessions in background
muxr new work api

# Interactive session switcher (TUI)
muxr switch

# Before reboot
muxr save

# After reboot
muxr restore

# List active sessions
muxr ls
```

## Config

`~/.config/muxr/config.toml`:

```toml
default_tool = "claude"

[verticals.work]
dir = "~/projects/work"
color = "#7aa2f7"

[verticals.personal]
dir = "~/projects/personal"
color = "#9ece6a"

# Remote GCE instances (optional)
[remotes.lab]
project = "my-gcp-project"
zone = "us-central1-a"
user = "my_user"
color = "#4285F4"
connect = "ssh"              # "ssh" or "mosh"
instance_prefix = "lab-"     # muxr lab foo -> instance lab-foo
```

## Session switcher

`muxr switch` opens an interactive TUI picker:

- Color-coded rows by vertical
- Sorted by most recent activity, grouped by vertical
- Fuzzy filter with `/`
- `j`/`k` or arrow keys to navigate
- `d` to kill a session (with confirmation)
- `Enter` to switch, `q` to quit

Bind it in tmux for fast switching:

```tmux
bind s display-popup -E -w '80%' -h '80%' "muxr switch"
```

## Remote sessions

Remote sessions create a local tmux proxy that SSHes to a GCE VM and
attaches to the remote tmux. They appear in `muxr ls` and the switcher
alongside local sessions.

```bash
muxr lab trustchain          # SSH to lab-trustchain, attach remote tmux
muxr lab ls                  # List running instances and their sessions
```

Connections auto-reconnect on SSH drops with exponential backoff. Clean
exit (`exit` in remote tmux) disconnects normally.

## tmux integration

muxr generates tmux status bar colors from your config:

```tmux
set -g status-left "#(muxr tmux-status)"
```

The status bar dot color matches your vertical's configured color.

## Commands

| Command | Description |
|---------|-------------|
| `muxr` | Control plane (bare shell) |
| `muxr <vertical> [context...]` | Create or attach to a session |
| `muxr <remote> [context...]` | Create or attach to a remote proxy session |
| `muxr <remote> ls` | List instances and remote tmux sessions |
| `muxr new <vertical> [context...]` | Create a session in the background |
| `muxr switch` | Interactive TUI session switcher |
| `muxr rename <name>` | Rename the current session |
| `muxr kill <name>` | Kill a session |
| `muxr kill all` | Kill all sessions |
| `muxr ls` | List active sessions |
| `muxr save` | Snapshot sessions before reboot |
| `muxr restore` | Recreate sessions after reboot |
| `muxr init` | Create default config file |
| `muxr completions <shell>` | Generate shell completions (zsh, bash, fish) |
| `muxr tmux-status` | Generate tmux status-left (called by tmux) |

## How it works

muxr is a thin layer over tmux. Each session is a named tmux session
with one window running your tool.

```
muxr work api
  +-- tmux new-session -s "work/api" -c ~/projects/work
  +-- tmux send-keys "claude" Enter
  +-- tmux attach -t "work/api"
```

Remote sessions work the same way, but the tool command is an SSH
connection wrapped in a reconnect loop:

```
muxr lab trustchain
  +-- tmux new-session -s "lab/trustchain" -c ~
  +-- tmux send-keys "gcloud compute ssh ... -- tmux new-session -A -s trustchain" Enter
  +-- tmux attach -t "lab/trustchain"
```

Sessions persist across terminal restarts (tmux keeps them alive).
`muxr save` snapshots session state to JSON so `muxr restore` can
recreate them after a reboot -- including reconnecting remote sessions.

## License

MIT

---

Built in the Den by Tanuki and Andrew Dunn, 2026.
