# Changelog

All notable changes to this project will be documented in this file.

## [v0.9.7] - 2026-04-21

### Added
- `muxr ls --active` filters to sessions with a running harness process.
  Hides panes sitting at a shell prompt with nothing attached. The muxr
  control-plane session is also filtered out. Same detection as
  `muxr claude upgrade`, so the two commands target the same set.
- `muxr retire <session>` is the counterpart to `muxr new`: gracefully
  `/exit`s the harness (up to 10s, then SIGKILL), kills the tmux
  session, removes the git worktree when the vertical uses worktrees,
  and refreshes `state.json` so `muxr restore` won't recreate it.
  `retire all` retires every session. `--keep-worktree` skips the
  worktree deletion for the edge case where you want the tree on disk.
  Main checkouts (`<vertical>/default`) never have their working tree
  removed — only worktree-shaped contexts.

### Fixed
- Harness-process detection matches against full argv, not just `comm`.
  Node-based harnesses (claude-code) run as `node /path/to/claude …`
  where `comm` is `node`, so the previous comm-based check silently
  reported "no claude process" for every session and made
  `muxr claude upgrade` a no-op. The fix restores upgrade behavior and
  powers the new `ls --active` and `retire` commands.
- `wait_for_exit` polls in `harness::upgrade` no longer leak "No such
  process" to stderr once the pid is gone. The poll is a normal exit
  signal, not an error.

## [v0.4.1] - 2026-04-02

### Added
- Auto-reconnect for remote proxy sessions (#1). SSH drops reconnect
  automatically with exponential backoff (3s to 30s cap, 20 retries max).
  Clean exit (rc=0) breaks the loop. IP cache invalidated between retries.

## [v0.4.0] - 2026-04-01

### Added
- Remote proxy sessions via `[remotes]` config. `muxr lab bootc` creates
  a local tmux session that SSHes to a GCE VM and attaches to the remote
  tmux. Remote sessions appear in the local session switcher and `muxr ls`.
- TUI session switcher (`muxr switch`). Color-coded rows by vertical,
  fuzzy filter with `/`, `j/k` navigation, `d` to kill sessions.
- Activity-based sorting in switcher. Most recently used sessions at top,
  muxr control plane pinned. Groups ordered by most recent session.
- Visual group separators between verticals in the switcher.
- Age column showing time since last input (2m, 1h, 3d).
- Kill from switcher with `y/n` confirmation, re-enters picker after kill.
- `muxr <remote> ls` lists running GCE instances and their tmux sessions.
- Remote-aware `muxr ls` tags proxy sessions as `(remote)`.
- Remote session save/restore -- proxy sessions reconnect via SSH on
  `muxr restore`.
- Name collision validation -- config rejects a name used as both vertical
  and remote.
- `Remote::instance_name()` method, replaces `/` with `-` for valid GCE
  instance names from nested contexts.

### Changed
- `muxr ls` now loads config to detect remote sessions.
- `muxr new` supports remote verticals.
- Completions include remote names alongside verticals.
- `color_for()` checks both verticals and remotes.

## [v0.2.0] - 2026-03-30

### Added
- Default to Claude Code as the session tool.
- Claude session ID discovery via process tree traversal.
- `muxr save` captures active Claude session IDs.
- `muxr restore` passes `--resume <id>` to resume Claude sessions.

### Fixed
- `--claude` flag wiring through session creation.
- Window name kept as `claude` (dropped `--name` flag).

## [v0.1.0] - 2026-03-29

### Added
- Initial release. Named tmux sessions organized by verticals.
- `muxr <vertical> [context...]` to create or attach to sessions.
- `muxr new` for background session creation.
- `muxr save` / `muxr restore` for session persistence across reboots.
- `muxr ls`, `muxr kill`, `muxr rename`.
- `muxr tmux-status` for color-coded tmux status bar integration.
- `muxr completions` for zsh, bash, fish.
- `muxr init` for default config creation.
