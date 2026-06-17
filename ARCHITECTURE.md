# muxr architecture

muxr is a tmux session manager for AI coding harnesses: it launches, resumes,
recycles, and switches between `<repo>/<campaign>` sessions, each running a
coding runtime (Claude Code today; runtime-agnostic by design). As of **3.0.0**
the shape is a **small stable core + one subprocess extension contract** for the
fiddly bits that change. Full estate context + cutover history lives in
`nomograph/keaton` -> `campaigns/harness/sessions/baseline.md`.

## The irreducible core (small, rarely revs)
The part nothing else does, and the reason muxr exists:
- **The resume engine** -- snapshot `(name, dir, runtime, session-id)`, replay the
  runtime's `--resume <id>`; recycle (serialize -> exit -> reopen FRESH from a
  pointer); runtime-agnostic composed-context launch.
- **tmux lifecycle** -- create/attach/list/kill/rename, with optional socket
  isolation (`--server`).
- **save/restore** across reboots; the **make-durable** recycle event; the
  **pre-create hook** runner; and the **extension-invoke** mechanism below.

No dominant host absorbs this (scion = container/worktree-per-agent fleet: wrong
shape + unstable), so the core stays first-party and deliberately minimal.

## The extension contract (LOCKED at 3.0.0)
One mechanism for every fiddly bit that keeps changing. At an opinionated
chokepoint muxr OPTIONALLY invokes a configured command with structured JSON on
stdin and reads structured JSON from stdout; with nothing configured it runs a
built-in DEFAULT and behaves exactly as 2.1 (verified byte-for-byte vs the 2.1
tag). Transport is a **subprocess** (`sh -c`, `MUXR_EXTENSION_POINT` env,
fail-closed) -- it mirrors the `pre_create` hook runner and synthesist's
`discover_policy_extension`; deliberately **not WASM** (transport-per-shape, no
wasmtime in core). The contract was earned from real extractions (resolver +
make-durable, alongside the existing hook/statusline seams), not designed up
front, then **locked at 3.0.0** -- the discipline below governs further change.

Points (all default-when-absent; bare muxr with no `[extensions]` /
`[session_env]` / `[chooser]` is a fully usable launcher -- the Josh test):
1. **RESOLVER** (`[extensions].resolver`) -- intent `{session,repo,campaign,
   resume_id,model}` in, layout facts `{dir,campaign_md,log_path,runtime,
   add_dirs,resume_id}` out; any omitted field falls back to the built-in
   `[layout]` (the 2.1 config-drive layout). The single launch chokepoint
   (`resolve_layout` -> `compose_launch_command` in `src/session.rs`).
2. **MAKE-DURABLE** (`[extensions].make_durable`) -- fired before recycle/close;
   supplies the agent-facing flush message (what to serialize). muxr always
   appends its own exit directive, so a message that omits an exit can't hang
   recycle. Absent -> the built-in flush prompt.
3. **SESSION ENV** (`[session_env]`) -- generic templated `new-session -e`
   passthrough (`{session}`/`{repo}`/`{campaign}`/`{session_slug}`). Session<->tool
   coupling (e.g. `SYNTHESIST_SESSION = "{session_slug}"`) is config, not core.
4. **RUNTIME-ADAPTER** -- the `Tool` struct, pure config (args/resume_args/
   model_args/discovery/`supports_add_dirs`/...). Adding a runtime is config;
   there is no per-runtime code branch.
5. **CHOOSER** (`[chooser].command`) -- opt out of the built-in campaign-aware TUI
   to an external picker (e.g. sesh) for plain attach; built-in stays the default.
6. **PRE-CREATE HOOKS** -- the shell provisioning seam (`mise install` + the skill
   shim).

## What is deliberately NOT in core
- **The statusline.** It was Claude-Code-specific chrome (parsed CC's statusLine
  JSON) and doubled as the writer of a session-health cache. Shed in 3.0.0:
  the `claude-status` renderer, the health cache, `SessionHealth`, the
  `status_command` field, the chooser's health columns, and the health-only
  `<tool> compact`/`status` actions all removed. The statusline is now the
  runtime's own concern -> a user-owned renderer (the runtime's statusline config
  points at it). muxr neither renders nor invokes it.

## Discipline (governs change post-lock)
- Earn any new extension point from >=2 real extractions (avoid the synthesist
  v2->v3 rebuild trap). Keep the format dead-simple (subprocess + JSON, no
  wasmtime). ONE contract for all bits, not N mechanisms.
- Defaults stay behavior-compatible. A behavior change to a default, or a new
  contract shape, is a major bump -- the core/extension re-architecture is what
  earned the 3.0 major.

## History
- **2.0.1** -- last pre-cutover release (http: generic-package era).
- **2.1.0** -- config-drive resolver: a `[layout]` struct makes the harness layout
  DATA (campaigns dir, file names, archive/switchboard slugs), threaded through
  `compose_launch_command` + all callers. Non-breaking. The data foundation the
  3.0 resolver defaults to.
- **Cutover (2026-06-16)** -- kit + rune retired estate-wide; muxr installs via mise
  `gitlab:nomograph/muxr`; pre_create hooks = `mise install` + the skill shim.
- **3.0.0 (2026-06-16)** -- the extension contract + statusline shed above, built in
  phases P0-P4 (P0 contract, P1 resolver, P2 runtime-adapter, P3 make-durable +
  session-env, P4 chooser), each behavior-compatible; recycle hardened (sysinfo
  PID match so the flush wait survives a `ps`-blocking sandbox; exit directive
  always appended). Independent adversarial review confirmed GO + behavior-compat.
