# Harness Plugins

## Problem

muxr hardcodes Claude Code knowledge in three places:
- `tmux.rs`: tool_command() matches on "claude" to add `--name` and `--resume`
- `state.rs`: session discovery chains through pgrep + `~/.claude/sessions/{pid}.json`
- `claude_status.rs`: entire module for Claude-specific statusline

This works but isn't extensible. Adding a second harness would mean more hardcoded
matches. Operations like "restart all sessions on a new model" require knowing how
each tool handles resume, model selection, and session persistence.

## Design

### Principles

1. **muxr is a tmux manager first.** Harness features are opt-in. No harness
   config = pure tmux session management with current behavior preserved.
2. **Built-in defaults, config overrides.** Claude definition ships with muxr.
   Users only write config to customize or add new harnesses.
3. **Harness trees are subcommands.** `muxr claude upgrade`, not `muxr upgrade claude`.
   Each harness gets its own namespace, discovered from config at startup.

### Command structure

```
# Core (always available)
muxr <vertical> [context...]      open/attach session
muxr switch                       TUI picker
muxr save / restore / kill        session lifecycle
muxr ls                           list sessions
muxr new <vertical> [context...]  create without attaching

# Harness trees (opt-in per config, one tree per harness)
muxr claude upgrade [--model <model>]   restart all sessions on new model
muxr claude status                      show harness status across sessions

muxr opencode upgrade                   same pattern, different tool
muxr cursor status
```

### Config

```toml
default_tool = "claude"

[verticals.work]
dir = "~/projects/work"
color = "#7aa2f7"
tool = "claude"              # optional per-vertical override

[verticals.personal]
dir = "~/projects/personal"
color = "#9ece6a"
tool = "opencode"            # this vertical uses a different tool

# Harness definitions. Claude ships as a built-in default.
# Only define [harnesses.claude] if you need to override the defaults.
# Other harnesses must be configured explicitly.

[harnesses.opencode]
bin = "opencode"
args = []
resume_args = []
session_discovery = "none"
```

**Built-in claude definition** (compiled into muxr, used when no
`[harnesses.claude]` config exists):

```rust
HarnessConfig {
    bin: "claude",
    args: ["--name", "{name}"],
    resume_args: ["--resume", "{session_id}"],
    model_args: ["--model", "{model}"],
    session_discovery: SessionDiscovery::File {
        pattern: "~/.claude/sessions/{pid}.json",
        id_key: "sessionId",
    },
    status_command: Some("muxr claude-status"),
}
```

Users can override any field by defining `[harnesses.claude]` in config.
The config definition fully replaces the built-in (no merging).

**Tool resolution order:** `--tool` flag > vertical's `tool` > `default_tool`

### Name collision validation

Config::load() validates that harness names do not collide with:
- Vertical names
- Remote names
- Reserved names: `init`, `ls`, `save`, `restore`, `new`, `rename`,
  `kill`, `switch`, `tmux-status`, `claude-status`, `completions`

### Implementation: external_subcommand

Clap derive macros generate subcommands at compile time. Harness trees
are dynamic (from config). Use `#[command(external_subcommand)]`:

```rust
#[derive(Subcommand)]
enum Commands {
    Init,
    Ls,
    Save,
    // ... existing variants ...
    #[command(external_subcommand)]
    External(Vec<std::ffi::OsString>),
}
```

The `External` variant catches `muxr claude upgrade --model opus-4-7`.
The match arm checks if args[0] matches a configured harness, then
parses the remaining args by hand (upgrade/status + their flags).

To make harness subcommands visible, append them to help output via
`Command::after_help()` at startup after loading config.

Completions: extend `completions.rs` to enumerate `config.harnesses.keys()`
and generate sub-completions for `upgrade` and `status` under each.

### muxr claude upgrade

For each tmux session running the harness:

1. **Discover** the harness process via recursive pgrep walk
   (not single-level -- handles mise/nvm wrappers in the process tree)
2. **Verify** the PID is still the expected binary (`ps -p <pid> -o comm=`)
3. **Save** the session ID via the harness's discovery method
4. **SIGTERM** the harness process, wait up to 10s
5. **SIGKILL** if still alive after timeout
6. **Poll** pane for shell prompt (`tmux capture-pane -p`, look for `$`/`%`/`#`),
   backoff up to 5s
7. **Send** rebuilt command with resume args + new model via `tmux send-keys`

All interpolated values are shell-escaped before sending to tmux.

**Scope:** Local sessions only. Remote (GCE) sessions do not participate
in harness operations -- processes are on the remote machine.

**Multi-pane:** Operates on first pane only. Warns if session has multiple
panes with harness processes.

### muxr claude status

Reads health cache files (`~/.config/muxr/health/<session>.json`) and/or
invokes the harness's `status_command` to display per-session status.
Shows: model, context usage, session age, cost.

### What changes in muxr

| Module | Change |
|--------|--------|
| config.rs | `HarnessConfig` struct, `harnesses` field, collision validation, built-in claude default |
| main.rs | `External` variant, harness dispatch, after_help for harness subcommands |
| tmux.rs | `tool_command()` looks up harness config first, falls back to built-in, falls back to raw binary |
| state.rs | `discover_session_id()` uses harness discovery config, recursive pgrep |
| **new: harness.rs** | `upgrade()` and `status()` implementations, generic over HarnessConfig |
| completions.rs | Enumerate harness names + sub-completions |
| claude_status.rs | Unchanged |

### What doesn't change

- Session management (create, attach, switch, kill, save, restore)
- TUI switcher
- Remote/GCE integration
- Pre-create hooks
- `muxr save` continues to capture harness session IDs (uses harness
  discovery config instead of hardcoded claude paths)

### Validation

- `muxr claude upgrade --model claude-opus-4-7` restarts all local
  claude sessions in-place on the new model
- `muxr claude status` shows model + context for all sessions
- `muxr --help` lists harness subcommands from config
- `muxr save` / `muxr restore` still captures and resumes claude sessions
- No harness config = identical behavior to current muxr
- Per-vertical `tool` override works (`muxr work` uses claude,
  `muxr personal` uses opencode)
- Name collision between harness and vertical is caught at config load
