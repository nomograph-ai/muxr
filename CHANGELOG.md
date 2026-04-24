# Changelog

All notable changes to this project will be documented in this file.

## [v1.0.2] - 2026-04-24

### Added
- Interactive campaign scaffolding at launch. `muxr <harness> <unknown>`
  now prompts the human through creating the campaign (paths,
  synthesist tree, one-line description) instead of erroring out.
  Keeps the launch single-command from the muxr control plane --
  no mkdir / cp / $EDITOR dance before you can start a session on
  a new work-body.

## [v1.0.1] - 2026-04-24

### Fixed
- `child_pids` (used by `muxr save` Claude session discovery) no longer
  uses `pgrep -P <ppid>` alone. macOS pgrep requires a pattern argument
  and silently returns nothing without one, breaking session discovery
  for every tmux session on macOS. Replaced with `ps -A -o pid,ppid`
  parsed in-process. Cross-platform. Surfaced during first real muxr
  save/restore dogfood on macOS.

## [v1.0.0] - 2026-04-24

First stable release. Muxr graduates from tmux session manager to
opinionated harness manager with first-class campaign/session primitives.

### Added
- Campaign/session primitives. A **campaign** is a long-lived body of
  work; a **session** is an ephemeral episode. Files live at
  `campaigns/<slug>/campaign.md` and
  `campaigns/<slug>/sessions/<date>[-<suffix>].md`.
- `muxr <harness> <campaign>` launches today's session for a campaign.
  If the session file does not exist, muxr scaffolds one from
  `campaigns/TEMPLATE/sessions/TEMPLATE.md`. If any same-day suffixed
  file exists (e.g., `2026-04-24-cicd.md`), muxr attaches to it rather
  than creating a new file.
- `muxr <harness> <campaign> <date>` re-attaches a specific past
  session.
- System prompt composition. At launch, muxr merges the campaign body
  and session body into `append_system_prompt`, so Claude enters the
  session already knowing the campaign conventions, entrypoint, and
  recent log.
- Campaign `paths:` passed as `--add-dir`. The runtime knows the full
  work surface declared by the campaign; no cwd roulette.
- Schema validation at launch: session's `campaign:` must match the
  requested slug, or muxr errors out before starting tmux.
- Announce entrypoint, synthesist trees, and path count when creating a
  new tmux session on a campaign.
- New module `primitives` handles frontmatter parsing, campaign/session
  file resolution, scaffolding, and prompt composition.
- Dependencies: `serde_yaml_ng` for frontmatter, `chrono` for session
  date generation.

### Removed (breaking)
- Git worktree support. The `worktree` field on verticals is gone, and
  `tmux::create_worktree` / `tmux::remove_worktree` / `tmux::is_git_repo`
  / `tmux::worktree_path` have been deleted along with their tests.
  Concurrency moves to synthesist sessions and git branches; muxr no
  longer manages filesystem splits.
- `muxr <harness> fork` subcommand. Forking was only useful with
  worktrees; without them, forking would just give two Claude sessions
  fighting over the same working directory.
- `--keep-worktree` flag from `muxr retire`. No worktrees means nothing
  to keep.
- All `use_worktree` branching in `cmd_open`, `cmd_new`, `cmd_kill`,
  `cmd_retire`.

### Renamed (breaking)
- Type `Vertical` → `Harness`. The thing that maps to a directory is now
  called a harness everywhere -- in code, in config, in the CLI.
- Type `HarnessConfig` → `Tool`. Tool definitions (claude, opencode)
  were previously called "harnesses" internally; that name moved to
  the named project estate.
- Type `HarnessLaunchSettings` → `LaunchSettings`.
- Config key `[verticals.<name>]` → `[harnesses.<name>]`.
- Config key `[verticals.<name>.harness]` → `[harnesses.<name>.launch]`.
- Config key `[harnesses.<name>]` (tool defs) → `[tools.<name>]`.
- Module `src/harness.rs` → `src/tool.rs` (tool operations:
  upgrade, compact, model switch, status).
- Methods `Config::harness_for` → `Config::tool_for`,
  `Config::harness_names` → `Config::tool_names`.
- Legacy `muxr <harness> <context>` launch path is **removed**.
  Every launch now requires a campaign: `muxr <harness> <campaign>`.

### Config migration

Rewrite `~/.config/muxr/config.toml`:
- Rename `[verticals.X]` table headers to `[harnesses.X]`
- Rename `[verticals.X.harness]` to `[harnesses.X.launch]`
- If you had `[harnesses.<tool>]` tool definitions, rename to `[tools.<tool>]`
- Remove any `worktree = true|false` lines (no-op)

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
