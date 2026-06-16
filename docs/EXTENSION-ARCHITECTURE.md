# muxr: extension architecture & the 3.0 plan

Durable design record (2026-06-16). Self-contained so a fresh/compacted session
can build toward 3.0 without re-deriving. Full estate context + cutover history:
`nomograph/keaton` -> `campaigns/harness/sessions/baseline.md` (2026-06-16 entries).

## Shipped in 3.0.0-rc.1 (2026-06-16)
All of P0-P4 landed on branch `feat/3.0-extensions`; 134 tests, clippy clean;
behavior-compatible (no `[extensions]`/`[session_env]`/`[chooser]` => 2.1).
- **P0** `src/extension.rs` -- `invoke(cmd, point, input)`: `sh -c`, JSON
  stdin -> JSON stdout, `MUXR_EXTENSION_POINT`, fail-closed-and-loud.
- **P1** RESOLVER (`[extensions].resolver`) at `resolve_layout` in session.rs;
  default = the 2.1 `[layout]`; override dir relocates campaign/log defaults.
- **P2** runtime-adapter: `supports_add_dirs` capability replaces the one
  hardcoded `bin != "pi"` branch. statusline stays a contract instance
  (runtime-invoked via its own hook -- transport intentionally not muxr's).
- **P3** MAKE-DURABLE event (`[extensions].make_durable`) at the recycle flush;
  generic `[session_env]` (templated, `new-session -e`) which generalizes +
  closes muxr!4; folded in muxr!3 transition UX (`ui::step_start`).
- **P4** `[chooser].command` -- config-gated external picker; built-in TUI stays
  default (full shed to sesh would lose campaign/health/lifecycle).

RC intent: validate the contract through real daily use, THEN tag 3.0.0 (and
lock the contract). The plan/discipline below is the original design rationale.

## Where muxr is (history)
- **2.0.1** — last pre-cutover release (http: generic-package era).
- **2.1.0 (2026-06-16)** — config-drive resolver: a `[layout]` config struct makes
  the harness layout DATA (campaigns dir, campaign/log file names, archive +
  switchboard slugs), threaded through `compose_launch_command` (src/session.rs) +
  all callers; retired the delegating free-fns + ARCHIVE_DIR/SWITCHBOARD consts.
  Non-breaking (defaults reproduce the built-in 2-level model). 130 tests green.
  **This is the DATA FOUNDATION the 3.0 resolver-extension defaults to.**
- **Cutover (2026-06-16)** — kit + rune retired estate-wide. muxr now installs via
  mise `gitlab:nomograph/muxr` (lock + checksum). pre_create hooks =
  `["mise install", "mise exec -- just --justfile ~/gitlab.com/dunn.dev/runes/justfile skills"]`.
  muxr config = `dunn.dev/pi/configs/muxr/config.toml` (symlinked to
  ~/.config/muxr/config.toml; currently on branch `harness/always-sync` -- open:
  land on pi main?).
- **Open branches (unmerged):** muxr!3 (transition UX -- session-create progress);
  muxr!4 (SYNTHESIST_SESSION -- REWORK to a generic templated env-passthrough).
- **The irreducible core** (two scans confirmed nothing else does this): the RESUME
  engine -- snapshot `(name, dir, runtime, session-id)` -> replay the runtime's
  `--resume <id>`; recycle; runtime-agnostic composed-context launch. No dominant
  host (scion = container/worktree-per-agent fleet: wrong shape + unstable).

## 3.0 thesis
Small stable CORE + **ONE subprocess extension format** for every fiddly bit that
keeps changing (layout, statusline, session-coupling, runtime specifics, serialize).
Contract (estate-wide "common pattern"): at an opinionated chokepoint, OPTIONALLY
invoke an extension with structured JSON in -> structured JSON out; run a built-in
DEFAULT if none configured. Transport = **SUBPROCESS** (mirrors muxr's existing
`status_command` + synthesist's `discover_policy_extension`), **NOT WASM**
(transport-per-shape; no wasmtime in muxr core).

## Extension points (fiddly bits -> extensions)
1. **RESOLVER** `resolve(intent) -> {dir, add_dirs, prompt, runtime, resume_id}`.
   Default = the 2.1 config-drive layout. Subsumes the migrate-layout saga.
2. **STATUSLINE** `render(session) -> string`. Default generic; ext = runtime-aware
   (CC context/cost/cache, Pi, opencode). Already pluggable via `status_command`.
3. **RUNTIME-ADAPTER** launch + resume per runtime (cc/pi/opencode). Mostly the
   existing `Tool` abstraction -- formalize so adding a runtime = an adapter.
4. **MAKE-DURABLE** a core lifecycle EVENT fired pre-recycle/close; ext supplies
   what-to-serialize (the harness worklog flush). serialize stops being a skill.
5. **SESSION ENV / COUPLING** generic templated `env` passthrough (reworked muxr!4);
   SYNTHESIST_SESSION becomes config/ext, not core.
6. **PRE-CREATE HOOKS** stay the shell provisioning seam (mise install + the shim).

## Core that stays (small, rarely revs)
tmux lifecycle (create/attach/list/kill/rename); the RESUME engine; recycle (fires
make-durable); save/restore; the hook runner + the extension-invoke mechanism.
CHOOSER -> shed to `sesh` (scan: commodity).

## Plan (earn-the-contract; each phase = coordinated multi-worker build)
- **P0** define the subprocess contract (invoke shape, JSON in/out, default-when-
  absent, discovery). Do NOT lock until extracted from >=2 real cases.
- **P1** RESOLVER as the first extension (default = 2.1 config-drive; `compose_launch_
  command` in src/session.rs is the single chokepoint). Highest value; proves it.
- **P2** STATUSLINE + RUNTIME-ADAPTER (2nd/3rd case -> LOCK the contract).
- **P3** MAKE-DURABLE event + generic env-passthrough (fold in muxr!3 transition UX).
- **P4** shed CHOOSER -> sesh.
Bare muxr (no extensions) must stay a usable launcher with sane defaults (Josh test).

## Discipline
Earn the contract from >=2 real extractions (avoid the synthesist v2->v3 rebuild
trap). Keep the format dead-simple (subprocess + JSON, no wasmtime). ONE contract for
all bits, not N mechanisms. Defaults stay behavior-compatible; the core/extension
re-architecture earns the 3.0 major bump.

## To race toward it (fresh-session entry)
Start at **P1**: extract the resolver behind the subprocess contract, defaulting to
the config-drive layout already shipped in 2.1. Mirror synthesist's
`discover_policy_extension` shape (nomograph/extension + synthesist!14, HELD) in
subprocess form. Then P2 locks the contract.
