# Companion pane (design spec, ready to build)

A muxr session may carry an optional **companion pane**: the runtime (AI tool)
on the left, a review/preview pane on the right. Created at launch AND faithfully
recreated on restore. Opt-in, config-driven, per-repo overridable.

## Why this shape (the key insight)

`Tmux::create_session` (`src/tmux.rs`) is the SINGLE chokepoint both launch
(`src/session.rs`) and restore (`src/state.rs`, two branches) call. Put the
companion split there and **restore recreates it for free** — no `state.json`
schema change. The companion command is recomputed from config + the session's
dir on restore, so nothing extra is persisted.

Hooks can't do this: only `pre_create` exists (no post-create), and restore
bypasses hooks entirely. So it must be in `create_session`.

## The change (4 edits + tests)

1. **`src/config.rs`** — add:
   ```rust
   #[derive(Debug, Clone, Deserialize, Serialize)]
   #[serde(deny_unknown_fields)]
   pub struct Companion {
       #[serde(default)] pub enabled: bool,         // opt-in
       pub cmd: String,                              // templated (see below)
       #[serde(default = "default_companion_side")] pub side: String, // "h"|"v"
       #[serde(default = "default_companion_size")] pub size: u8,      // pane %
   }
   ```
   - `companion: Option<Companion>` on both `Config` (global default) and `Repo`
     (override). `#[serde(default)]`.
   - `fn companion_for(&self, session_name, dir) -> Option<ResolvedCompanion>`:
     mirror `session_env_for` — split `repo/campaign`, build the slug, pick
     `repos[repo].companion` else global, return `None` if absent or `!enabled`,
     interpolate `{session} {repo} {campaign} {session_slug} {dir}` into `cmd`.

2. **`src/tmux.rs::create_session`** — add param `companion: Option<&ResolvedCompanion>`.
   After the tool `send-keys`, if present:
   ```text
   split-window <-h|-v> -d -l <size>% -t <target> -c <dir> "<cmd>"
   ```
   `-d` keeps focus on the tool pane (pane 0); the cmd runs in the new pane
   directly (mdv's proven idiom — no send-keys / pane-id capture).

3. **Callers** pass `config.companion_for(name, dir).as_ref()`:
   - `src/session.rs` launch site
   - `src/state.rs` local-restore site
   - remote-restore site → pass `None` (no local artifacts to preview)

4. **Tests**: `companion_for` resolution (global vs repo override, disabled,
   templating); a `create_session` arg test (companion None vs Some records the
   split). Keep `deny_unknown_fields` coverage.

## Config (lands in dunn.dev/harness/muxr/config.toml)

```toml
[repos.tanuki.companion]
enabled = true
cmd = "muxr-preview campaigns/{campaign}"   # globs that dir's md/svg, tabulated
side = "h"
size = 45
```

## The engine

`muxr-preview` = the previewer (cycle files · live-reload · clipboard · md via
glow · mermaid via mmdc→chafa · svg via chafa; chafa falls back to unicode so it
works in any terminal). Promote the working `preview.py` (currently in the
concent campaign) into the harness baseline (`dunn.dev/harness/`), installed to a
stable PATH location (e.g. `~/.local/bin/muxr-preview`) via the harness justfile.
Given a dir, it globs `*.md`/`*.svg`/`*.mmd` and tabulates them.

## Safety / adoption

- Build on this worktree (`feat/companion-pane`, from `origin/main`).
- `cargo test` + a manual `muxr save` → `muxr restore` round-trip on a THROWAWAY
  session to prove the companion comes back.
- Only then `muxr upgrade` to move live sessions onto the new binary. Checkpoint
  with the operator before that estate-wide adoption.

## Home

This is `harness`-category work. Build it in a muxr/harness session, not a
content session.
