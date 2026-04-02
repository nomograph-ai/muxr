# Changelog

All notable changes to this project will be documented in this file.

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
