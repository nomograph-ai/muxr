# muxr companion panes — a greenfield plan

Goal: a first-class, muxr-native **companion pane** — the runtime (AI tool) on
the left, a live review/preview surface on the right — created at session launch
and **faithfully restored**, opt-in and config-driven. Designed clean from first
principles, then landed without disturbing the load-bearing session manager.

This is `harness`-category infra. Build it in a `nomograph` (muxr) session on a
worktree; ship it as its own change, not folded into content work.

---

## 1. Why (and why the ad-hoc version failed)

The need: review markdown / mermaid / SVG (and later: logs, status, diffs)
beside the agent, without leaving the session. The shell-split approach
(`mdv.zsh`) was never adopted because it is **invisible to muxr**:

- not in `muxr ls` / `switch` / `status`;
- **dropped by `muxr save`/`restore`** — gone on reboot;
- ignored by `retire` / `recycle` / `upgrade` (leaks or dies inconsistently);
- a shell function you must remember exists.

A robust capability has to be something muxr *owns*: created, tracked, and
restored on the same path as everything else.

---

## 2. Design principles

1. **muxr-native** — the session layout is muxr's, not a user's afterthought.
2. **Restore-faithful** — a restored session is byte-identical to a fresh one,
   companion included. This is the headline robustness property.
3. **Zero state bloat** — the companion is a pure function of `(config, session
   dir/name)`. Recompute on restore; persist nothing new in `state.json`.
4. **Opt-in + config-driven** — off by default; per-repo override of a global
   default; the command is a template, the geometry is data.
5. **Terminal-robust** — the engine renders in *any* terminal (image protocol
   when present, unicode fallback otherwise). No blank panes.
6. **Single chokepoint** — one place in the code creates the layout, so launch
   and restore can never diverge.
7. **Extensible, not over-built** — the model is "auxiliary panes" (plural,
   typed); v1 ships exactly one (`preview`), shaped so a second is trivial.

---

## 3. The model

A muxr session is, today, `1 tmux session → 1 window → 1 pane` (the tool).
Greenfield generalization:

```
session
└── window
    ├── pane 0  : the runtime (AI tool)        [focus]
    └── pane 1+ : auxiliary panes (companions)  [no focus]
```

An **auxiliary pane** is `{ cmd, side, size, focus }`. The window's layout is
the tool pane plus an ordered list of aux panes, derived from config. v1 ships a
single companion (`preview`); the array shape leaves room for a logs tail, a
`synthesist status` watch, a `git status` watch, etc. without re-architecting.

**Key implementation insight:** `Tmux::create_session` (`src/tmux.rs`) is the
*single* function both launch (`src/session.rs`) and restore (`src/state.rs`,
local + remote branches) call. Build the layout there and **restore recreates
the companion for free** — no new state, no second code path. Hooks can't do
this: only `pre_create` exists (no post-create), and restore bypasses hooks
entirely.

---

## 4. Config schema

Lands in `dunn.dev/harness/muxr/config.toml` (the symlinked source of
`~/.config/muxr/config.toml`).

```toml
# Global default (optional). Off unless `enabled`.
[companion]
enabled = false
cmd     = "muxr-preview {dir}/campaigns/{campaign}"   # templated
side    = "right"      # right | left | top | bottom
size    = 42           # percent of the window
focus   = "tool"       # tool | companion  (where the cursor lands)

# Per-repo override (merges over the global; repo wins field-by-field).
[repos.tanuki.companion]
enabled = true
```

- Templating reuses muxr's existing interpolation: `{session} {repo} {campaign}
  {session_slug} {dir}`.
- `deny_unknown_fields` on the struct (consistent with muxr's config hygiene).
- Greenfield-clean extension path: promote `[companion]` to `[[aux_panes]]`
  (an array) when a second companion is wanted; the single-companion form stays
  valid as sugar for a one-element array.

---

## 5. muxr internals (the change)

Four focused edits + tests. Small surface, high leverage.

### 5.1 `src/config.rs`
- `struct Companion { enabled: bool, cmd: String, side: Side, size: u8, focus:
  Focus }` with serde defaults (`side=right`, `size=42`, `focus=tool`).
- `companion: Option<Companion>` on both `Config` (global) and `Repo` (override).
- `fn companion_for(&self, session_name: &str, dir: &Path) -> Option<Resolved>`:
  mirror `session_env_for` — split `repo/campaign`, build the slug, pick the
  repo override else the global, return `None` if absent or `!enabled`,
  interpolate the template into a literal command + geometry.

### 5.2 `src/tmux.rs::create_session`
- New param: `companion: Option<&Resolved>` (or, for the extensible shape,
  `aux: &[Resolved]`).
- After the tool `send-keys`, for each companion:
  ```
  split-window <-h|-v> -d -l <size>% -t <session> -c <dir> "<cmd>"
  ```
  `-d` keeps focus on the tool pane; the command runs directly in the new pane
  (mdv's proven idiom — no `send-keys`, no pane-id capture). `side` maps to
  `-h`/`-v` plus `-b` for left/top. Honor `focus=companion` with a trailing
  `select-pane`.

### 5.3 callers
- `src/session.rs` launch site → pass `config.companion_for(name, dir).as_ref()`.
- `src/state.rs` local-restore site → same (recomputed from config).
- remote-restore site → `None` (no local artifacts to preview).

### 5.4 CLI: `muxr pane`
- `muxr pane` — toggle the companion on the current live session
  (split if absent, kill the companion pane if present).
- `muxr pane open <file>` (stretch) — point the companion at a specific file.
- Discoverable in `--help`; documented in `muxr skill`.

---

## 6. The engine — `muxr-preview`

A small, robust previewer; the companion's default command. Promote the working
prototype (`preview.py`, currently in the concent campaign) into the harness
baseline and install it to a stable PATH location.

**Stack (all already present):** `glow` (markdown) · `mmdc` (mermaid → PNG/SVG)
· `chafa` (PNG/SVG → terminal; auto-detects kitty/sixel/iterm, **falls back to
unicode blocks so it never shows a blank pane**) · `pbcopy`/`osascript`
(clipboard).

**Behaviors:**
- **Tabulate** a set of files (a dir → glob `*.md *.mmd *.svg`, or explicit args);
  `n`/`p`/arrows to cycle.
- **Render** by type: md (glow prose + each ```mermaid block as an image), mmd
  (mmdc→chafa), svg/images (chafa). Per-block error isolation (a bad diagram
  shows an error, never crashes the loop).
- **Live-reload** on save (mtime poll; dependency-free and robust).
- **Clipboard**: `c` copies source, `y` copies the rendered PNG (paste into
  slides/docs).
- **Quit/reload/open**: `q` / `r` / `o`.

**Home + install:** `dunn.dev/harness/` (sibling to `mdv.zsh`), installed to
`~/.local/bin/muxr-preview` via the harness `justfile`. Make it `mise`-pure
(pin its interpreter/deps); no brew assumptions.

**Why custom, not off-the-shelf:** `glow` is md-only; `yazi` uses native image
protocols with no unicode fallback (blank on a plain terminal); `presenterm` is
slideshow-paged and doesn't cycle *files*. None cover tabulate + multi-format +
chafa-fallback + clipboard. A ~150-line engine on the proven stack does.

---

## 7. Implementation order

1. **Engine first (de-risk, standalone):** promote + harden `muxr-preview`,
   install it, use it by hand. It is useful immediately and unblocks review.
2. **Config:** `Companion` struct + `companion_for` + tests.
3. **Layout in `create_session`:** the split; unit-test the arg-building.
4. **Callers:** launch + local-restore wired; remote = None.
5. **`muxr pane` verb.**
6. **Docs:** `muxr skill` + config example + a short README note.

---

## 8. Testing

- **Unit:** `companion_for` (global vs repo override, disabled, templating);
  `create_session` arg-building (None vs Some → records the split); config parse
  with/without `[companion]` (and `deny_unknown_fields` rejection).
- **Integration (the robustness proof):** on a **throwaway** session — launch
  with a companion → assert two panes; `muxr save` → kill → `muxr restore` →
  **assert the companion came back**. This is the test that proves the design.
- **Manual:** focus stays on the tool; quitting the companion doesn't kill the
  session; `retire`/`recycle` clean up the whole window.

---

## 9. Rollout / adoption (don't break the estate)

muxr runs *this* session and every other harness; a bad launch/restore change
breaks session creation everywhere.

1. Develop on a worktree off `origin/main` (already set up:
   `nomograph/muxr` → `feat/companion-pane`).
2. `cargo test` green.
3. The save→restore round-trip on a throwaway session (§8).
4. Build the binary; install to a *new* mise version.
5. `muxr upgrade` to move live sessions onto it — **only after an operator
   checkpoint**. `upgrade` is in-place and reversible (pin back the old
   version) if anything is off.

---

## 10. Risks + mitigations

| Risk | Mitigation |
|---|---|
| Breaks launch/restore estate-wide | single chokepoint; full test incl. restore round-trip; staged `upgrade` with rollback |
| Companion shrinks the tool pane uncomfortably | configurable `size`; `focus=tool`; `muxr pane` toggle to drop it on demand |
| Blank pane on a plain terminal | chafa unicode fallback (no native-protocol dependency) |
| First render slow (mmdc ×N) | render lazily / show a "rendering…" line; cache by mtime |
| Focus stolen on create | `split-window -d`; explicit `select-pane` on the tool |
| State drift | recompute from config on restore; persist nothing |

---

## 11. Future (the shape pays off)

- **Multiple aux panes** (`[[aux_panes]]`): a logs tail, a `synthesist status`
  watch, a `git status` watch — same lifecycle, same restore-faithfulness.
- **Per-campaign companion** (campaign.md front-matter overrides the repo
  default) so each campaign points the review pane at its own artifacts.
- **`muxr pane open`** to retarget the companion live.
- **Layout presets** (`muxr <repo> <campaign> --layout review`).

---

## 12. Definition of done

- `enabled` per-repo → launching the session opens tool-left / preview-right,
  focus on the tool.
- `muxr save` → `muxr restore` brings the companion back, unchanged.
- `muxr pane` toggles it on a live session.
- `muxr-preview` renders md/mermaid/svg, tabulates, live-reloads, copies — in any
  terminal.
- `cargo test` green; docs + config example shipped; adopted via `muxr upgrade`
  after the operator checkpoint.
