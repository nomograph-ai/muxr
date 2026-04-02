![muxr hero](hero.png)

# muxr

[![Built with GitLab](https://img.shields.io/badge/Built%20with-GitLab-orange?logo=gitlab)](https://gitlab.com/dunn.dev/muxr)
[![pipeline status](https://gitlab.com/dunn.dev/muxr/badges/main/pipeline.svg)](https://gitlab.com/dunn.dev/muxr/-/pipelines)
[![crates.io](https://img.shields.io/crates/v/muxr.svg)](https://crates.io/crates/muxr)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://gitlab.com/dunn.dev/muxr/-/blob/main/LICENSE)

**Tmux session manager for AI coding workflows.** Named sessions,
vertical-aware directories, save/restore across reboots. Works with
any terminal tool (claude, opencode, vim, shell).

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
muxr personal blog          # personal/blog

# Create sessions in background (from control plane or /shell)
muxr new work api
muxr new personal

# Switch between sessions
# Ctrl-a s  (session picker)

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
default_tool = "claude"  # or "opencode", "vim", "shell"

[verticals.work]
dir = "~/projects/work"
color = "#7aa2f7"

[verticals.personal]
dir = "~/projects/personal"
color = "#9ece6a"

[verticals.oss]
dir = "~/projects/oss"
color = "#73daca"
```

## tmux integration

muxr generates tmux status bar colors from your config. Add to
`~/.tmux.conf`:

```tmux
set -g status-left "#(muxr tmux-status)"
```

The status bar dot color matches your vertical's brand color.

## Commands

| Command | Description |
|---------|-------------|
| `muxr` | Control plane (bare shell for managing sessions) |
| `muxr <vertical> [context...]` | Create or attach to a named session |
| `muxr new <vertical> [context...]` | Create a session in the background |
| `muxr rename <name>` | Rename the current session |
| `muxr kill <name>` | Kill a session |
| `muxr kill all` | Kill all sessions |
| `muxr ls` | List active tmux sessions |
| `muxr save` | Snapshot sessions before reboot |
| `muxr restore` | Recreate sessions after reboot |
| `muxr init` | Create default config file |
| `muxr completions <shell>` | Generate shell completions (zsh, bash, fish) |
| `muxr tmux-status` | Generate tmux status-left (called by tmux) |

## How it works

muxr is a thin layer over tmux. Each session is a named tmux session
with one window running your AI coding tool (opencode, claude, etc.).

```
muxr work api
  |
  +-- tmux new-session -s "work/api" -c ~/projects/work
  +-- tmux send-keys "opencode" Enter
  +-- tmux attach -t "work/api"
```

Sessions persist across terminal restarts (tmux keeps them alive).
`muxr save` snapshots session names and directories to JSON so
`muxr restore` can recreate them after a reboot.

## License

MIT
