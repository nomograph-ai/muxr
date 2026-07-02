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

## The runtime-adapter shape (what a new runtime implements)
A runtime adapter is a `[tools.<name>]` block -- pure config, no code. It is the
contract any CLI (Claude, pi, opencode, ...) follows to be drivable by muxr. The
fields, and where they bend per runtime:

| field | purpose | `{...}` tokens |
| --- | --- | --- |
| `bin` | executable | -- |
| `args` | initial launch | `{name}` |
| `resume_args` | resume by id | `{session_id}` |
| `continue_args` | resume when no id is known | -- |
| `fork_args` | branch a new id off an existing conversation | `{session_id}` |
| `model_args` | set model at launch | `{model}` |
| `rename_command` / `model_switch_command` / `exit_command` | slash commands typed into the LIVE pane (not launch flags); `exit_command` drives recycle + `muxr upgrade` | `{name}` / `{model}` |
| `prompt_mode` | `"file"` (`--append-system-prompt-file`) or `"string"` | -- |
| `supports_add_dirs` | whether the CLI takes `--add-dir` for extra roots | -- |
| `wrapper` | optional command prefixed ahead of `bin` (e.g. a sandbox) | -- |
| `session_discovery` | `{type="file", pattern="…/{pid}.json", id_key="…"}` to recover the conversation id, or `{type="none"}` | `{pid}` |

The seams the subprocess contract covers: a runtime with no per-pid session file
sets `session_discovery.type = "none"` and recovers its id in a **resolver**; a
runtime with no `--name`/`--add-dir` simply omits them and sets
`supports_add_dirs = false`. Worked example with all three bends:
`extensions/adapters/opencode.toml`.

## Reference extensions & distribution
`extensions/` ships the reference set and is the canonical home for the shape:
- `adapters/{claude,pi,opencode}.toml` -- the declarative adapters above. As of
  **3.1** claude + pi ARE muxr's built-in defaults: the binary embeds these two
  files (`include_str!`) and parses them into the adapter table at load, so core
  carries no hand-written per-runtime struct and `tool_for`/`tool_names` resolve
  generically. opencode is a config-only third-party port (a worked example, not
  a default).
- `examples/{resolver,make-durable}.sh` -- copyable templates for the two
  subprocess points (JSON in/out).

Distribution has no registry: declarative adapters are TOML (blessed set ships
in the release; your own live in / are `include`d from your estate repo);
subprocess extensions are scripts that live in your estate repo, invoked by
absolute path. The dir is part of this repo while the contract settles, designed
to split into a standalone `muxr-extensions` bundle once external runtimes adopt.

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

## Decision records
Architecture decisions live as numbered records in [`docs/adr/`](docs/adr/) --
one file per decision, with an RFC-style `Design detail` section where the
mechanism warrants it. A record is an RFC while `Proposed` and an ADR once
`Accepted`; supersede rather than rewrite. See [`docs/adr/README.md`](docs/adr/README.md).
The extension contract and the small-stable-core posture in this document are
recorded as [ADR 0001](docs/adr/0001-extension-architecture.md); the sections
above are its living reference.

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
- **3.0.1 (2026-06-17)** -- macOS harness-detection fix: `pid_runs_bin` matches the
  executable `name()` too (sysinfo's `cmd()` argv is empty on macOS, so 3.0.0's
  argv-only match reported "no harness process" -> broke recycle + `muxr upgrade`).
- **3.1.0 (2026-06-17)** -- zero runtime knowledge in core: the built-in claude + pi
  adapters became the shipped `extensions/adapters/*.toml` (embedded via
  `include_str!`), `tool_for`/`tool_names` resolve generically through the adapter
  table, and the hardcoded per-runtime branches were removed. Additive,
  behavior-compatible (so a minor bump). Closes nomograph/muxr#4.
