# ADR 0004: Companion pane (auxiliary review/preview panes)

- Status: Accepted (implemented in v3.5.0)
- Date: 2026-07-02
- Relates to: [ADR 0001](0001-extension-architecture.md)
- Implemented in: `src/config.rs` (`Companion` + `companion_for`) and `src/tmux.rs::create_session` (the split), threaded through the launch / recycle / local-restore call sites; remote + bare sessions pass `None`.

## Context

A muxr session today is one tmux session -> one window -> one pane (the
runtime). Operators frequently want a review surface *beside* the agent --
markdown, mermaid, SVG (later: logs, status, diffs) -- without leaving the
session. Ad-hoc shell-splits were never adopted because
they are invisible to muxr: absent from `ls` / `switch` / `status`, dropped by
`save` / `restore`, and ignored by `retire` / `recycle` / `upgrade`, so they
leak or die inconsistently. A robust review surface has to be something muxr
*owns* -- created, tracked, and restored on the same path as everything else.

## Decision

Add an optional, opt-in **companion pane**: the runtime on the left, a
review/preview pane on the right, created at launch AND faithfully recreated on
restore. Config-driven, off by default, per-repo overridable.

The load-bearing placement: `Tmux::create_session` is the single chokepoint that
both launch (`src/session.rs`) and restore (`src/state.rs`) call. Put the
companion split there and **restore recreates it for free** -- the companion
command is recomputed from config + the session dir, so nothing new persists in
`state.json`. (Hooks cannot do this: only `pre_create` exists, and restore
bypasses hooks entirely.)

Generalize to **auxiliary panes** (plural, typed). v1 ships exactly one
(`preview`), shaped so a second is trivial:

```
session -> window
  |-- pane 0 : runtime (AI tool)    [focus]
  `-- pane 1+: auxiliary panes       [no focus]
```

## Design detail

### Config (`src/config.rs`)

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Companion {
    #[serde(default)] pub enabled: bool,          // opt-in
    pub cmd: String,                               // templated
    #[serde(default = "default_companion_side")] pub side: String, // "h"|"v"
    #[serde(default = "default_companion_size")] pub size: u8,      // pane %
}
```

- `companion: Option<Companion>` on both `Config` (global default) and `Repo`
  (per-repo override), `#[serde(default)]`.
- `companion_for(session_name, dir) -> Option<ResolvedCompanion>` mirrors
  `session_env_for`: split `repo/campaign`, build the slug, pick
  `repos[repo].companion` else the global, return `None` if absent or
  `!enabled`, and interpolate `{session} {repo} {campaign} {session_slug} {dir}`
  into `cmd`.

### Chokepoint (`src/tmux.rs::create_session`)

Add a `companion: Option<&ResolvedCompanion>` param. After the tool `send-keys`,
if present:

```
split-window <-h|-v> -d -l <size>% -t <target> -c <dir> "<cmd>"
```

`-d` keeps focus on pane 0; the cmd runs directly in the new pane (no send-keys
or pane-id capture needed).

### Callers

Pass `config.companion_for(name, dir).as_ref()` at the launch site
(`src/session.rs`) and the local-restore site (`src/state.rs`); pass `None` at
the remote-restore site (no local artifacts to preview).

### The previewer engine

The companion `cmd` is any program. v1 pairs with a `muxr-preview` previewer
(cycle files, live-reload, clipboard; markdown via a pager, mermaid via `mmdc`,
SVG via `chafa`, with a unicode fallback so it renders in *any* terminal -- no
blank panes). It lives in the operator's estate, installed to a stable `PATH`
location -- muxr only splits the pane and runs the configured command, so the
previewer is operator-owned (per ADR 0001) and evolves without muxr changes.

```toml
[repos.<name>.companion]
enabled = true
cmd = "muxr-preview campaigns/{campaign}"
side = "h"
size = 45
```

### Tests

`companion_for` resolution (global vs repo override, disabled, templating); a
`create_session` arg test (companion `None` vs `Some` records the split); keep
the `deny_unknown_fields` coverage.

## Consequences

- **Restore-faithful:** a restored session is byte-identical to a fresh one,
  companion included -- the headline robustness property, earned by the
  single-chokepoint placement.
- **Zero state bloat:** the companion is a pure function of `(config, session
  dir/name)`; nothing new persists in `state.json`.
- **Terminal-robust:** the previewer falls back to unicode, so no blank panes.
- **Core stays small (ADR 0001):** muxr adds a pane split + a config struct; the
  previewer and its renderers are operator-owned. The `cmd` is just a command.
- Estate-wide adoption is via `muxr upgrade`; gate it behind a throwaway
  `save` / `restore` round-trip that proves the companion returns, and
  checkpoint with the operator before moving live sessions.

## Alternatives considered

- **Ad-hoc shell-split.** Rejected: invisible to muxr
  (not in `ls` / `switch` / `status`, dropped by `save` / `restore`, mishandled
  by `retire` / `recycle` / `upgrade`). A capability muxr does not own cannot be
  restore-faithful.
- **Persist the companion in `state.json`.** Unnecessary and a schema change:
  recomputing from config on restore is simpler and cannot drift from the launch
  path.
- **A post-create hook.** No post-create hook exists, and restore bypasses hooks
  entirely; the chokepoint is `create_session`.
