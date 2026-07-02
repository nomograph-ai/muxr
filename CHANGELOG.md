# Changelog

All notable changes to this project will be documented in this file.

## [3.5.0] (2026-07-02)

Companion panes: an optional review/preview pane beside the runtime, created at
launch and faithfully recreated on restore (ADR 0004).

### Added
- **Companion pane (ADR 0004).** Opt-in, config-driven: `[companion]` (global)
  or `[repos.<name>.companion]` (per-repo override), with `enabled` / `cmd`
  (templated: `{session} {repo} {campaign} {session_slug} {dir}`) / `side`
  (`h`|`v`) / `size` (percent). muxr splits an auxiliary pane at the
  `create_session` chokepoint, so launch, recycle, and local restore all
  recreate it identically with no `state.json` change; focus stays on the
  runtime pane. What the pane renders is an operator-owned command (per ADR
  0001); remote and bare sessions get no companion.

## [3.4.0] (2026-07-02)

Extension levers for the readiness gate and the resolver, plus an ADR/RFC
decision-record convention. The behavior lands as extension examples + config;
the only core change is an additive `repo_dir` field on the resolver intent, per
the small-stable-core posture (ADR 0001).

### Added
- **Interrupt-reclaim readiness probe (#6).** `extensions/examples/readiness.sh`,
  a Command probe that corroborates a `busy` state file against tmux pane
  activity and reclaims an interrupted-but-idle session instead of stranding it
  for up to `STALE_BUSY_SECS`. Shipped as a commented opt-in in the Claude
  adapter; the File probe stays the default. See ADR 0003.
- **Repo-scoped, portable resolver (#7).** `extensions/examples/resolver.sh`
  gains a `.repo` opt-out branch (repo-scoping via the launch intent, no per-repo
  schema field); docs recommend `~/`-relative resolver paths for portability.
- **Decision records (`docs/adr/`).** An ADR/RFC convention (RFC while
  `Proposed`, ADR once `Accepted`) with a template + index: 0001 the
  small-stable-core / extension architecture posture, 0002 readiness-gated
  upgrade, 0003 interrupt reclaim.
- **`repo_dir` on the resolver `ResolveIntent`.** The launch intent now carries
  the resolved repo checkout dir, so a resolver need not re-derive it (additive).

### Changed
- deps: pipeline component to v5.3.1; lock-file maintenance.

## [3.3.2] (2026-06-22)

Config robustness (#3): unknown, misspelled, or renamed config keys now fail
loudly instead of being silently dropped. From an adversarial review.

### Fixed
- **Silent config drops on a schema bump (#3).** The config structs used
  `#[serde(default)]` with no `deny_unknown_fields`, so a renamed top-level
  table like `[harnesses.*]` (the old name for `[repos.*]`) parsed to an empty
  `repos` and surfaced much later as a baffling "unknown repo". Every config
  struct now `deny_unknown_fields`, so an unknown/misspelled/renamed key is a
  hard parse error that names the offending key and lists the valid ones. A
  `KNOWN_RENAMES` table adds an actionable hint when a known-old key is still
  present (e.g. ``hint: `[harnesses.*]` was renamed to `[repos.*]` ``).
- **Silent drops inside the probe blocks (review follow-up).**
  `[tools.*.readiness]` and `[tools.*.session_discovery]` are internally-tagged
  enums, which serde forbids combining with `deny_unknown_fields`; their
  payloads moved into dedicated `deny_unknown_fields` structs (`FileProbe`,
  `CommandProbe`, `FileDiscovery`). A typo like `idle_valeu` -- which would have
  silently disabled the `muxr upgrade` quiet-period guard and let a busy session
  be relaunched -- is now rejected. The flat TOML format is unchanged.

### Internal
- `Config::load()` split into a testable `Config::parse(content, source)`; the
  strict-parse, rename-hint, and collision checks are now unit-tested directly.

## [3.3.1] (2026-06-21)

Closes the loose ends left open by the 3.3.0 hardening review.

### Fixed
- **Isolated-server (`-L`) support in `upgrade`.** The exit-send, relaunch-send,
  model-switch send, and `wait_for_prompt` capture used a bare `tmux` and so hit
  the default socket; they now route through the `-L`-aware handle (new
  `Tmux::send_keys` / `Tmux::capture_pane`). `MUXR_TMUX_SERVER` setups now work.
- **`muxr <tool> upgrade --name X`** now honors the filter (was ignored —
  upgraded every session); the dispatch path also picks up
  `DEFAULT_MIN_IDLE_SECS` instead of a stray hardcoded `20`.
- **Narrowed the readiness→exit TOCTOU window.** On a confirmed-Safe verdict,
  `upgrade` now sends the exit command BEFORE composing the relaunch (compose
  happens while the session exits), instead of composing in between.
- **`muxr status` / multi-session `upgrade` no longer re-query tmux per
  session** — activity is fetched once per sweep (the `--wait` poll still reads
  the single session live). `session_readiness` takes the activity in.

### Added
- Test for `ReadinessProbe::Disabled` (explicit opt-out resolves to `None`).

## [3.3.0] (2026-06-21)

Hardening pass on the readiness gate (3.2.0), from an adversarial review.

### Fixed
- **`activity_floor` false-SAFE on lookup failure.** A failed tmux activity
  lookup returned `Safe` (via `unwrap_or(0)`); it now returns `Unknown` so the
  gate is conservative on missing data.
- **Stale `busy` files block upgrades forever.** A session killed mid-turn (no
  `Stop`) left a permanent `busy` file. A `busy` file older than
  `STALE_BUSY_SECS` (1h) now classifies `Unknown` → falls through to the floor.
- **Idle with a missing `since`** no longer reads `Safe` with zero cooldown
  (now `Unknown` → floor).
- **`--dry-run --wait N` no longer sleeps** for N seconds; dry-run reports the
  live verdict and never polls, and labels each row would-upgrade vs would-skip.
- **`upgrade` hardcoded `/exit`** — now uses `tool_def.exit_command` (Pi
  `/quit`), so non-Claude runtimes exit cleanly.

### Added
- **Default cooldown raised to 180s** (`DEFAULT_MIN_IDLE_SECS`, was 20s), shared
  by `upgrade` and `status`, so a between-turns gap no longer reads `Safe`.
- **`muxr status --min-idle`** and a `quiet <age>` column (time since last tmux
  activity), with a single activity fetch per run.
- **Self-upgrade guard** — `upgrade` skips the session it is invoked from.
- **`ReadinessProbe::Disabled`** — explicit per-tool opt-out (does not inherit
  the builtin probe).
- **`Command` probe timeout** (`PROBE_TIMEOUT_SECS`, 10s) so a hung probe cannot
  block muxr.

## [3.2.0] (2026-06-20)

### Added
- **Readiness-gated `upgrade`** and a read-only **`muxr status`** command.
  `muxr upgrade` now checks whether each session is at a safe-to-relaunch
  boundary before relaunching it, so a fleet migration no longer interrupts
  in-flight turns. Runtime-agnostic and extension-based: a declarative
  `ReadinessProbe` (`File`/`Command`/`None`) on the `Tool` descriptor —
  mirroring `session_discovery` — read generically by core, with a universal
  tmux-activity floor when no probe is declared or it returns `Unknown`.
  `Unknown` is treated as not-safe unless `--force`.
  - New flags: `--force`, `--wait <secs>`, `--min-idle <secs>`.
  - `muxr status` prints per-session readiness (`SAFE`/`BUSY`/`UNKNOWN`).
  - The Claude adapter ships a `[readiness]` File probe; `pi`/`opencode`
    default to the floor. The producer side (a runtime's turn-boundary hooks
    writing the state file) lives in the operator's harness, not muxr.
  - Design: [ADR 0002](docs/adr/0002-readiness-gated-upgrade.md).

## [3.1.1] (2026-06-17)

Fixes from an adversarial review of the 3.0.1/3.1.0 cluster (the prior tags
shipped on green CI without review).

### Fixed
- **Harness detection robustness** (`pid_runs_bin`): added `exe()` file-stem as a
  third match signal (a path-exec'd binary, populated on macOS even when `cmd()`
  is empty) alongside `name()` and the `cmd()` argv tokens. The doc comment no
  longer overstates coverage: it now documents the real macOS limitation -- a
  harness launched as a separate INTERPRETER process whose name appears only in
  argv (`node /…/claude.js`) is undetectable on macOS (sysinfo `cmd()` is empty
  there), which is acceptable because claude 2.x ships as a native binary caught
  by `name()`. Tests strengthened: the by-name test is now explicitly scoped to
  the native-binary signal, plus a no-false-match guard and a dead-pid guard.
- **opencode resolver example** used a non-existent flag (`opencode session list
  --json`); corrected to `--format json`. As written it silently no-op'd and
  never resumed.
- Stale doc comment claiming add-dirs are skipped "when `bin == "pi"`" corrected
  to the capability-based reality (`supports_add_dirs` / `emits_add_dirs()`); no
  per-bin branching exists.

### Packaging
- `Cargo.toml` switched from an `exclude` blocklist to an explicit `include`
  ALLOWLIST. Two files are embedded via `include_str!` (the shipped adapter
  TOMLs + `resources/skill.md`); an allowlist guarantees they're in the
  published tarball so an accidental exclude can never silently publish a crate
  that fails to compile downstream. Also drops the `extensions/examples/*.sh`
  templates from the crates.io tarball (they belong in your estate repo, not the
  Rust crate).

## [3.1.0] (2026-06-17)

**Core carries zero runtime knowledge.** The built-in Claude + Pi adapters are no
longer hand-written Rust structs -- they ARE the shipped `extensions/adapters/
{claude,pi}.toml` files, embedded at compile time (`include_str!`) and parsed once
into the adapter table. `tool_for`/`tool_names` resolve generically through that
table; the per-runtime `match tool { "claude" => ..., "pi" => ... }` and the
hardcoded claude/pi name injection are gone. Adding or shipping a runtime is now
purely a matter of adapter TOML.

Behavior is preserved byte-for-byte (the bare-launcher test): a config naming `claude`
composes the identical launch/resume command as 3.0.x; the existing
`builtin_*_harness` / `tool_for_returns_builtin_*` / `tool_names_includes_*` tests
now assert against the shipped TOML. The shipped default set stays exactly
`{claude, pi}` (locked by test); `opencode.toml` remains a worked example, not a
default. This is the additive completion of the 3.0 runtime-agnostic cut
(nomograph/muxr#4); a user-facing adapter `include`/glob is deferred.

## [3.0.1] (2026-06-17)

### Fixed
- macOS harness detection regression: `pid_runs_bin` now matches the executable
  `name()` in addition to the `cmd()` argv. The 3.0.0 `ps` -> `sysinfo` migration
  matched argv ALONE, but `sysinfo`'s `cmd()` (argv via `KERN_PROCARGS2`) is
  restricted and comes back empty on macOS -- so every live session reported "no
  harness process," breaking recycle's flush wait and `muxr upgrade` (both
  silently skipped every session). Matching `name()` is the reliable macOS signal;
  `cmd()` still covers Linux and wrapper-launched binaries (e.g. `node /…/claude`).
  Guarded by a regression test that spawns a known binary and asserts detection.

## [3.0.0] (2026-06-16)

**muxr 3.0: a small runtime-agnostic core + one subprocess extension contract.**
The 3.0 line re-architects muxr around a single extension mechanism (JSON in ->
JSON out, run a built-in default when absent) and sheds the Claude-Code-specific
statusline + session-health out of core. A config with no `[extensions]` /
`[session_env]` / `[chooser]` reproduces 2.1 launch/resume behavior byte-for-byte
(independently verified against the 2.1 tag). Feature breakdown in the rc entries
below:
- **rc.1** -- the `[extensions]` subprocess contract + RESOLVER (default = the 2.1
  `[layout]`), generic `[session_env]` passthrough, the make-durable event,
  `[chooser]` delegation, and the `supports_add_dirs` runtime capability.
- **rc.2** -- the statusline + health cache removed from core (runtime-agnostic);
  the statusline is now the runtime's own concern, pointed at a user-owned renderer.

### Fixed (rc.2 -> 3.0.0 hardening)
- `muxr init` now writes a config that parses: `repos` is `#[serde(default)]`, so
  a fresh config (no repos yet) loads instead of failing with "missing field repos".
- recycle is robust to a `make_durable` extension whose message omits an exit
  instruction: muxr always appends its own exit directive, so recycle can no longer
  hang until the SIGKILL timeout.
- recycle locates the harness process via `sysinfo` instead of shelling `ps`, so the
  agent-paced flush wait still works under a sandbox that blocks the `ps` binary
  (previously the wait silently truncated to 5s and killed the flush).
- `wait_for_exit` / `wait_for_prompt` use `saturating_mul` (no overflow on a
  pathological `--wait`).
- Removed dangling references to retired commands (`claude-status`, `<tool> compact`,
  `<tool> status`) from shell completions and the emitted skill.

### Changed
- Dropped the orphaned `compact_command` `Tool` field (its only reader, the retired
  bulk-compact action, is gone).

## [3.0.0-rc.2] (2026-06-16)

**The statusline leaves core -- muxr is now runtime-agnostic.** The bundled
`muxr claude-status` renderer was Claude-Code-specific chrome (it parsed Claude
Code's statusLine JSON) that had leaked into core, and it doubled as the writer
of the session-health cache the chooser displayed. Health was only ever
populated by Claude Code -- Pi and opencode never wrote it. All of it is gone
(~530 lines): muxr now has zero Claude-Code knowledge. The statusline is a
fully external concern -- the runtime's own statusline config points at a
user-owned renderer script. Non-breaking for launch/resume/recycle; only the
statusline rendering moves out (point your runtime's statusline at your own
command).

### Removed
- **`muxr claude-status`** subcommand + `src/claude_status.rs` (the renderer).
- **Session-health cache** (`SessionHealth`, read/write) and the chooser's four
  health columns (context bar / context% / cache% / cost) -> collapsed to one
  status cell (live / open / kill?).
- **`status_command`** field on `[tools.<name>]` (vestigial -- only ever pointed
  at the now-removed renderer; unknown keys are ignored, so existing configs
  keep parsing).
- **`muxr <tool> compact [threshold]`** and **`muxr <tool> status`** -- both were
  health-only (bulk-compact by context %, ctx/cost listing); retired as feature
  sprawl. `muxr <tool> upgrade` and `model` remain.

### Migration
Set your runtime's statusline to your own renderer. For Claude Code, in
`~/.claude/settings.json`: `"statusLine": { "type": "command", "command":
"<your-renderer>" }`. A drop-in port of the old look (plus per-repo brand-mark
glyphs) lives at `dunn.dev/pi/configs/scripts/muxr-statusline.py`.

## [3.0.0-rc.1] (2026-06-16)

**One small stable core plus one subprocess extension contract for every
fiddly bit.** 3.0 re-architects muxr around a single extension mechanism: at
an opinionated chokepoint muxr OPTIONALLY runs a configured command (`sh -c`)
with structured JSON on stdin and reads structured JSON from stdout, falling
back to a built-in default when none is configured. The transport is a
subprocess (mirroring the existing `status_command` and `pre_create` hooks),
deliberately not WASM or a plugin ABI -- the social contract between tools
stays thin: JSON in, JSON out, default when absent. A config with no
`[extensions]`/`[session_env]`/`[chooser]` reproduces 2.1 behavior exactly, so
this is non-breaking for existing setups despite the major bump (the bump is
for the core/extension re-architecture). Release candidate: shipping to
validate the contract through real use before tagging 3.0.0.

### Added
- **`[extensions]` subprocess contract** (`extension.rs`): `invoke(cmd, point,
  input)` runs `sh -c cmd` with JSON on stdin, parses JSON from stdout, exports
  `MUXR_EXTENSION_POINT`, inherits stderr, and fails closed and loud.
- **RESOLVER extension** (`[extensions].resolver`): the launch chokepoint
  (`compose_launch_command`) now resolves layout facts (`dir`, `campaign_md`,
  `log_path`, `runtime`, `add_dirs`, `resume_id`) through `resolve_layout`. The
  default reproduces the 2.1 config-drive `[layout]` exactly; a configured
  resolver may override any field (omitted -> default) and add `--add-dir`s.
  Overriding `dir` relocates the campaign/log path defaults with it. A resolver
  error aborts the launch (no silent wrong-campaign fallback).
- **MAKE-DURABLE event** (`[extensions].make_durable`): the recycle flush is now
  a lifecycle event. A configured command supplies the agent-facing flush
  message; absent -> the built-in self-contained prompt; an empty message means
  "nothing to flush, just exit". Serialization stops being baked into muxr.
- **`[session_env]` generic env-passthrough**: per-session tmux variables
  (`new-session -e`, tmux 3.2+) templated with `{session}`, `{repo}`,
  `{campaign}`, `{session_slug}` (path-safe). Session<->tool coupling (e.g.
  `SYNTHESIST_SESSION = "{session_slug}"`) is now config, not core.
- **`[chooser].command`**: opt out of the built-in TUI to an external picker
  (e.g. sesh) for plain attach. Absent -> the built-in campaign-aware TUI.
- **`supports_add_dirs`** on `[tools.<name>]`: a runtime opts out of `--add-dir`
  via this capability instead of muxr branching on the binary name.

### Changed
- Adding a runtime is now pure config: the one hardcoded `if self.bin != "pi"`
  branch suppressing `--add-dir` is replaced by the `supports_add_dirs`
  capability (Claude: yes, Pi: no, by built-in default).
- Session creation shows liveness during slow `pre_create` hooks and the
  blocking launch (transient `step_start` lines overwritten by the result),
  and the launch line names the tool, instead of a silent pause.

## [2.1.0] (2026-06-16)

**Config-drive resolver: the layout is now data, not compiled-in.** The
filesystem layout of a muxr-managed repo (campaigns directory, per-campaign
`campaign.md`/`log.md` file names, the reserved `archive` directory, and the
`switchboard` slug) is read end-to-end from a `[layout]` config struct rather
than from hard-coded constants. A repo can override any of these via
`[layout]` in `config.toml`; omitting the section reproduces the built-in
2-level model exactly, so the change is non-breaking. This lays the resolver
boundary that a future subprocess-based layout extension can hook (mirroring
`status_command`), without introducing a trait or wasmtime.

### Added
- **`[layout]` config struct** (`campaigns_dir`, `campaign_file`, `log_file`,
  `archive_dir`, `switchboard_slug`), each with a default that reproduces
  today's behavior. `[layout]` in `config.toml` genuinely overrides the
  built-in 2-level model.

### Changed
- The layout-dependent primitives (campaign/log/dir path construction;
  `list_campaigns` / `scaffold_*` / `archive_campaign` / `campaign_file` /
  `resolve_or_scaffold_session`) and `compose_launch_command` now read
  `config.layout` end-to-end; all callers (session, main, switcher) thread it
  through. The delegating free functions and the `ARCHIVE_DIR` / `SWITCHBOARD`
  constants are retired. No behavior change with default layout.

## [2.0.1] (2026-06-04)

First published 2.0 release. Identical in features to 2.0.0 below; the
v2.0.0 tag failed the `readme-shape` CI gate (Unicode em dashes, a house
rule) before building or publishing, and tags are immutable, so the fix
ships as 2.0.1.

### Fixed
- Replaced Unicode em dashes with `--` across README, the emitted skill,
  CHANGELOG, and src strings/comments. No behavior change.

## [2.0.0] (2026-06-04, unpublished -- superseded by 2.0.1)

**Breaking: the repo/campaign redesign.** Sessions are now addressed as
two levels, `<repo>/<campaign>`, instead of three
(`<harness>/<campaign>/<topic>`). The first argument is a **repo** key
(`[repos]` in config, usually the repo's directory name), replacing the
old harness-key indirection where the config key differed from the repo it
opened. The old middle "category" segment becomes `category:` frontmatter,
and depth beyond two levels is handled by **sharding** a hub campaign into
sibling campaigns, not a third name segment. This release absorbs the
1.5.0 in-place-upgrade work (which was built but never tagged).

### Added
- **`muxr shard <new>`** -- spin a topic that crystallized inside the
  current campaign out into its own sibling campaign, inheriting the hub's
  `category:` and recording `sharded_from:` lineage, then launch it.
  `--repo`/`--from` name the hub explicitly for out-of-session use.
- **Launch chooser** (`muxr switch`, rebuilt) -- merges live sessions,
  dormant on-disk campaigns, and a per-repo "+ new campaign" affordance
  into one list grouped by repo, with shards indented under their hub.
  Enter switches / opens / creates depending on the row; `n` creates in the
  selected repo. Surfacing every campaign (not just the running ones) is
  the hygiene-visibility fix for accumulated, never-reviewed sessions.
- **`muxr migrate-layout <repo>`** -- migrate a repo's `campaigns/` tree
  from the old 3-level layout to the 2-level model
  (`campaigns/<campaign>/{campaign.md,log.md}`), inheriting category
  trees/paths into frontmatter. `--dry-run` prints the plan and the
  session-name rewrites; `--keep-archives` preserves stale files. It is
  filesystem-only and git-reversible; the live `state.json`/tmux cutover
  stays a human-gated step. Collision-safe: when two categories share a
  session stem (e.g. many `bootstrap.md`), the loser falls back to
  `<category>-<stem>`; dotted/invalid stems are sanitized
  (`v0.1-release` -> `v0-1-release`). A category dir is removed ONLY when
  every file under it migrated cleanly -- any skipped/unexpected file
  keeps the dir intact so nothing is ever silently deleted.
- `primitives::list_campaigns` -- campaign discovery (name + category +
  `sharded_from`) shared by the chooser, migration, and shard.
- `category:` and `sharded_from:` campaign frontmatter fields.
- **System prompt is now a pointer, not a snapshot.** `compose_prompt` emits
  HARNESS + the campaign's what/how + a pointer block (the one-line
  `entrypoint` plus the absolute `campaign.md`/`log.md` paths and a standing
  "re-read after `/compact`" instruction). It no longer inlines the growing
  log body, which bloated every turn (accelerating context exhaustion) and
  went stale on `/serialize`. The on-disk files are the source of truth; the
  prompt survives compaction, so the re-read directive does too.
- **`muxr reorient [name]`** -- inject a one-line nudge into a live session to
  re-read its `campaign.md` + `log.md` now. The explicit, on-demand companion
  to the standing pointer; run it right after a `/compact` to re-anchor from
  current disk state in seconds instead of a lossy summary.
- **Resumable dormant campaigns.** Opening a campaign that isn't running now
  consults the saved state for its last conversation id and relaunches with
  `--resume`, so it picks up where it left off instead of starting cold.
  `SavedState` gains `load()` + `session_id_for()`.
- **`muxr recycle [name]` + `--fresh`** -- the deliberate alternative to
  compact-looping. `recycle` asks the live session to flush its state into
  `log.md` (set a tight `entrypoint:` + append a dated log entry -- the
  procedure lives in the muxr skill, so it doesn't depend on a drifting
  external `/serialize` command), then **waits for the agent to actually
  exit** (agent-paced, no wall-clock guess, since a flush can take a long
  time) and reopens the session FRESH so it rehydrates from that pointer.
  `--fresh` on open starts a new conversation instead of resuming. The prior
  conversation stays on disk (recoverable via `--resume`), so recycling never
  destroys context -- it trades a degrading summary for a clean read of the
  authoritative on-disk state. The serialize/flush procedure is documented in
  the emitted skill.
- **`muxr archive <campaign>` + chooser `x`.** Move a campaign to
  `campaigns/archive/` so it leaves the chooser while staying on disk
  (reversible); `list_campaigns` skips the archive dir. Refuses a campaign
  with a live session. Prunes launcher sprawl without deleting anything.
- **Chooser UX.** Defaults to active/live sessions only (`a` toggles the full
  launcher view of dormant campaigns + create rows); bold colored per-repo
  header bands replace thin separators as the large visual differentiator
  between harnesses.
- **Fix: `compose_launch_command` now folds the plural
  `append_system_prompt_files`.** It previously read only the singular field
  and left the array set, so launch preferred the raw array and silently
  dropped the composed campaign+log -- every repo using base+overlay HARNESS
  files got no campaign/log in its prompt at all.

### Changed
- Config table renamed `[harnesses]`/`[verticals]` -> `[repos]`; struct
  `Harness` -> `Repo`. `muxr init`, the default template, the README, and
  the emitted skill all describe the two-level `<repo>/<campaign>` model.
- On-disk layout is one directory per campaign:
  `campaigns/<campaign>/{campaign.md,log.md}` (was
  `campaigns/<category>/sessions/<topic>.md`).
- The per-repo switchboard is `<repo>/switchboard` (was the
  `_switchboard` category specialcasing).
- agent-shape battery retargeted to 2.0: neutral placeholder repos, a
  two-level launch + chooser + shard probe set, a `launch_arg_correct`
  judge field (was `harness_arg_correct`), and `shard`/`migrate-layout`
  added to the cross-checked command list.
- `shard`, `skill`, and `migrate-layout` added to the reserved repo names.

## [1.5.0] (2026-06-02)

In-place session upgrade: move long-running harness sessions onto a newly
installed binary (e.g. a new Claude Code release) without losing their
conversation, harness rules, or working directories.

### Added
- `muxr upgrade [NAME]` (visible alias `migrate`) -- move running sessions
  onto the freshly installed harness binary, resuming each conversation in
  place. Graceful `/exit`, then relaunch on the binary the tool now resolves
  to. Flags: `--tool` (default `claude`), `--model` (switch model on
  relaunch), `--dry-run` (compose and print every relaunch without touching
  a session). No NAME upgrades every session running the selected tool; a
  NAME upgrades one. The previous `muxr <tool> upgrade` form still works and
  now also accepts `--dry-run`.
- `MUXR_CONFIG` env override for the config path (and `state.json` derives
  from its directory), so tests and harness fixtures can isolate config
  without hijacking `$HOME`.
- `upgrade`, `retire`, and `broadcast` added to the reserved harness names
  so a harness cannot shadow a built-in command (`retire`/`broadcast` were
  already commands but were missing from the list).
- `muxr skill` -- muxr now emits its OWN usage skill (launch grammar, the
  harness-key vs repo-name mapping, lifecycle verbs), compiled in from
  `resources/skill.md` so it never drifts from the binary. Replaces the
  separate registry entry; the tool ships its skill with its tag, matching
  `rune`/`kit`/`synthesist`.
- jig agent-shape battery: `upgrade` + harness-selection probe tasks, a
  launch-grammar / harness-vs-repo rubric, a standalone (out-of-repo)
  fixture, and a `harness_arg_correct` judge field. The fixture now installs
  the emitted skill into the trial workspace, so the battery measures the
  agent WITH the skill loaded -- the real experience -- which is the gate
  that closes the upgrade/retire/harness-selection discoverability cells.

### Changed
- **One launch composer for `open`, `restore`, and `upgrade`.** All three
  now build the relaunch through a single `compose_launch_command`, so a
  restored or upgraded session keeps its full composed HARNESS prompt and
  every campaign `--add-dir` path. Previously `restore` and `upgrade` took a
  lossy path (`restore_command` / `launch_command`) that emitted only
  `--name` and `--resume`, silently dropping harness rules and working dirs.
  The three paths now produce an identical command modulo the resume id, and
  the binary is resolved fresh each relaunch (this is what lets an upgrade
  pick up a new harness version).
- Composition now **degrades gracefully** instead of all-or-nothing: an
  archived-but-still-running session (its `.md` moved to `sessions/archive/`)
  still relaunches with the harness-level prompt and campaign `--add-dir`s,
  skipping only the missing session body. The bare name+resume relaunch
  remains as a last-resort fallback for genuinely unrecoverable cases
  (unknown harness, unparseable session name).
- Renamed the internal concept `vertical` -> `harness` throughout (params,
  `--help`, errors, comments) so the tool speaks one word; the field that
  was a resolved `Tool` is now `tool_def`. The first slug component is a
  harness *key*, which may differ from the repo directory name.

### Fixed
- Harness-process detection (`has_harness_process`, used by `upgrade` and
  `ls --active`) read argv via `sysinfo`'s `process.cmd()`, which is
  reliably EMPTY on macOS -- so every running session was reported as "no
  harness process" and skipped, making `upgrade` inert. Detection now reads
  argv via `ps -o args=` (the PID tree still comes from sysinfo). The three
  copies of the per-pid argv check are unified onto `state::pid_runs_bin`.

## [1.4.0] (2026-05-10)

### Added
- `LaunchSettings.append_system_prompt_files` -- array of file paths
  that are concatenated (newline-separated) before delivery to the
  tool. Enables base + overlay prompt composition for harness shared
  instructions (e.g. a shared `HARNESS-base.md` stacked with a
  per-harness `HARNESS.md` overlay). For `prompt_mode = "string"` (Pi)
  the composition is inlined into `--append-system-prompt`; for
  `prompt_mode = "file"` (Claude Code) it is materialised into a single
  temp file and passed via `--append-system-prompt-file`. Backward-
  compatible: the singular `append_system_prompt_file` still works; the
  array takes precedence when both are set (with a logged warning).

## [1.3.1] (2026-05-07)

### Fixed
- `muxr save` now correctly discovers session IDs under sandboxed
  runtimes. Previously `descendant_pids` shelled out to `/bin/ps`,
  which is denied by some sandbox profiles (e.g. `nono`'s
  `dangerous_commands_macos` group). Switched to the `sysinfo`
  crate (libproc on macOS, /proc on Linux) so process-tree walks
  work in-process without shell-out. Same fix applies to
  `has_harness_process`.
- `tool_for(<builtin>)` now treats user `[tools.<builtin>]` config
  as a PARTIAL override on top of the built-in definition. Previously
  the user config replaced the builtin wholesale, which silently
  collapsed every unspecified field to its type-default. The
  customer-visible bug: a user's `[tools.pi]` block declaring only
  `bin` and `prompt_mode` wiped `session_discovery`, `resume_args`,
  `continue_args`, and the slash-command quartet. After this fix,
  user-specified fields win and unspecified fields fall back to the
  builtin (per-field heuristic on type-default-equivalence). Two
  new tests pin the merge semantics:
  `harness_config_partially_overrides_builtin` and
  `pi_partial_override_keeps_session_discovery`.

### Added
- `sysinfo = "0.32"` dependency (default features off; `system`
  feature only). Native process metadata replaces the `/bin/ps`
  shell-out.

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
  named `work/harness/2026-04-24`, run
  `muxr rename work/harness/topic-flag` to align tmux state, the
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
  removed -- only worktree-shaped contexts.

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
