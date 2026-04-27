# Changelog

All notable changes to this project will be documented in this file.

## [1.3.0] (2026-04-27)

### Changed (breaking)
- Sessions are topic-keyed, not date-keyed. `muxr <harness> <campaign>
  <topic>` is the launch shape; `<topic>` is mandatory and validated as
  kebab-case (lowercase letters, digits, hyphens; 1-64 chars; must start
  with a letter or digit). Session files live at
  `campaigns/<campaign>/sessions/<topic>.md` -- no date prefix, no
  date-based glob fallback. The previous `muxr <harness> <campaign>
  [<date>]` shape is gone.
- The switchboard is now a per-harness singleton, not a date-stamped
  session. `muxr <harness>` (no campaign) opens
  `campaigns/_switchboard/sessions/switchboard.md` -- one accumulating
  log per harness, scaffolded once on first launch.

### Removed
- `primitives::today()` and the `chrono` dependency. Muxr no longer
  reads the wall clock to compose session filenames.
- `primitives::first_matching_session()` and the `<date>-<suffix>.md`
  glob fallback in `resolve_or_scaffold_session`. Topic lookup is
  exact-match only.

### Added
- `primitives::validate_topic()` -- topic format gate run at every
  campaign launch. Rejects empty, too-long, slash, space, uppercase,
  and any topic with leading, trailing, or consecutive hyphens, with
  prescriptive errors that name the next action.
- `primitives::SWITCHBOARD_TOPIC` constant for the singleton switchboard
  session filename.
- Reserved-slug guard. Explicit campaign args beginning with `_` are
  rejected; the switchboard is still launched via `muxr <harness>` with
  no campaign arg. Prevents accidental collision with the
  `_switchboard` singleton.
- Extra-positional-arg check. `muxr <harness> <campaign> <topic>
  <extra>` now bails with a usage hint instead of silently dropping
  the extra arg.

### Migration
- Existing dated session files (`2026-04-24.md`, `2026-04-24-cicd.md`)
  are not migrated. They keep working as exact-match topic lookups if
  the date string is passed as a topic, but new launches should pick
  topical names. Rename a live session and its file together with
  `muxr rename <harness>/<campaign>/<topic>` -- e.g. inside a session
  named `tanuki/harness/2026-04-24`, run
  `muxr rename tanuki/harness/topic-flag` to align tmux state, the
  session file on disk, and the runtime label.
- Existing dated switchboard sessions are orphaned by the singleton
  scaffold. The new `switchboard.md` is created alongside on first
  launch; the old dated files remain on disk untouched.
- TEMPLATE.md backward compatibility: `resolve_or_scaffold_session`
  replaces both `<topic>` (new) and `<date>[-<suffix>]` (legacy)
  placeholders. Existing TEMPLATE.md files keep working; new ones
  should use `<topic>`.

## [1.2.0] (2026-04-26)

### Added
- `muxr rename` now also moves the on-disk session file at
  `<harness>/campaigns/<campaign>/sessions/<segment>.md` and triggers
  the configured runtime relink, so the harness/campaign/session/segment
  address stays coherent across tmux state, the filesystem, and the AI
  runtime in one operation. Best-effort: refuses to clobber an existing
  target file. Six new tests cover missing source, target-already-exists
  clobber refusal, cross-campaign skip, and same-campaign rename. Total
  test count went from 74 to 80.
- Hero and avatar iconography in the nomograph paper-palette OV-1 style:
  `hero.svg` shows harness inputs, the multiplexer junction, and three
  coupled output rows for tmux session / session file / runtime id;
  `avatar.svg` is four nested rectangles for harness > campaign >
  session > segment.
- `CODEOWNERS`.

### Changed
- README rewritten to lead with the harness-multiplexer scope. The
  commands table now reflects that `rename` touches tmux state, the
  session file, and the runtime id together.

## [v1.1.1] - 2026-04-24

### Fixed
- Campaign launch now composes HARNESS.md + campaign body + session body
  into a single temp file and passes it via --append-system-prompt-file.
  Previous v1.0+/1.1.0 passed both --append-system-prompt (inline) and
  --append-system-prompt-file (config HARNESS.md), which Claude rejects:
  "Cannot use both". Also the multi-line inline value tripped tmux
  send-keys shell parsing (quote> prompts).
  - Temp file location: `${TMPDIR}/muxr-prompt-<harness>-<campaign>-<date>.md`
  - Session launch is now a single clean `claude ... --append-system-prompt-file <path> ...` invocation.

## [v1.1.0] - 2026-04-24

### Added
- **Switchboard.** `muxr <harness>` with no campaign arg launches the
  per-harness switchboard -- one always-available AI session whose job
  is to orchestrate work without the human memorizing muxr commands.
  Auto-scaffolded as `campaigns/_switchboard/` on first launch.
  - Purpose-split:
    - `muxr` -- bare control-plane shell
    - `muxr <harness>` -- harness switchboard (Claude, meta-scope)
    - `muxr <harness> <campaign>` -- campaign session (Claude, work)
  - The switchboard classifies intent ("I want to work on X") into
    action (scaffold/launch a campaign, report status, archive) and
    delegates real work to campaign sessions.

### Removed
- `muxr new <harness> <args>` subcommand. Creating detached sessions
  without attaching is obsolete: campaigns scaffold automatically on
  first launch, and the switchboard handles multi-session orchestration.
- `LaunchSettings.effort`, `.permission_mode`, `.max_budget_usd` config
  fields. Never wired up. Removed from serde surface.

### Changed
- Campaign tab completion filters slugs beginning with `_`. The
  switchboard reserved slug doesn't clutter `muxr <harness> <TAB>`.

## [v1.0.5] - 2026-04-24

### Changed (breaking vs v1.0.2/1.0.3/1.0.4 only)
- Campaign scaffolding no longer stops the launch to ask terminal
  questions about paths, tree, and description. Muxr creates a stub
  campaign.md and seeds the session's entrypoint with a discovery
  instruction, then launches claude normally. Claude searches the
  configured add_dirs + synthesist state to propose candidate paths
  and a tree, confirms with the human in one exchange, and writes
  the answers into campaign.md via Edit.

  The stdin-prompt flow (v1.0.2) put cognitive load on the operator
  at exactly the wrong moment -- you just typed the campaign name,
  you haven't switched modes to "give me structured answers". The
  LLM carries that work more naturally.

## [v1.0.4] - 2026-04-24

### Added
- Tab completion for campaigns. `muxr <harness> <TAB>` now offers the
  campaign slugs that actually exist on disk for that harness
  (read from `<harness-dir>/campaigns/*/campaign.md`), not just active
  tmux session contexts.
- Tool subcommand completions updated to include `compact` and `model`
  alongside existing `upgrade` and `status`.

### Changed
- Shell completions use the new vocabulary: `harnesses` (was "verticals")
  and `tools` (was built-in harness). Internal variable renames only;
  user-facing behavior is arg-positional, unchanged.

## [v1.0.3] - 2026-04-24

### Fixed
- `muxr save` no longer records the `muxr` control-plane session, and
  `muxr restore` skips it defensively. The control plane is a bare
  shell for typing muxr commands; recording it caused restore to
  relaunch claude in that pane (since the implicit tool defaulted to
  the config's default_tool). Control plane is now ephemeral -- just
  type `muxr` to re-open it.

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
