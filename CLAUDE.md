# muxr

Tmux session manager for AI coding workflows. Organizes sessions into
verticals (local dirs) and remotes (GCE instances) with color-coded TUI
switching, save/restore across reboots, and Claude Code statusline
integration.

## Build

```bash
cargo build --release       # binary at target/release/muxr
cargo test                  # run all tests
cargo clippy -- -D warnings # lint (also enforced at crate level)
```

CI uses `nomograph/pipeline/rust-cli@v2.4.6` with `cargo-deny` for
advisories and license audit. `audit_allow_failure: false` -- all gates
are hard failures.

## Architecture

Single binary, 8 modules, no runtime dependencies beyond tmux.

| Module | Responsibility |
|--------|---------------|
| `main.rs` | CLI (clap derive), command dispatch |
| `config.rs` | TOML config: verticals, remotes, hooks, colors |
| `tmux.rs` | Tmux subprocess wrapper (server isolation via `-L`) |
| `switcher.rs` | TUI session picker (ratatui + crossterm) |
| `claude_status.rs` | Claude Code statusline: reads JSON stdin, outputs ANSI |
| `state.rs` | Save/restore: session snapshot to JSON, Claude session discovery |
| `remote.rs` | GCE remote sessions: IP resolution, mosh/ssh reconnect loops |
| `init.rs` | Config file creation |
| `completions.rs` | Shell completion generation (zsh, bash, fish) |

## Key types

- `Config` -- deserialized from `~/.config/muxr/config.toml`
- `Vertical` -- local directory + color
- `Remote` -- GCE project/zone/user + color + connect method
- `Tmux` -- subprocess wrapper; `server` field enables socket isolation
- `SessionHealth` -- cached context/cost data for the TUI switcher

## Conventions

- All errors use `anyhow::Result` with `.context()` or `bail!()`.
  Error messages should be prescriptive: name the next action, not just
  the problem. ("Run `muxr init` to create one." not "Config not found.")
- `#![deny(warnings, clippy::all)]` is enforced at crate level. No new
  `#[allow]` without justification.
- Tmux session names use `vertical/context` format. The `Tmux::target()`
  method wraps names with `=name:` to avoid tmux's session/window
  target syntax conflicts.
- Config path: `~/.config/muxr/config.toml`. State path:
  `~/.config/muxr/state.json`. Health cache: `~/.config/muxr/health/`.
- `--server` flag isolates tmux sockets for testing and demo recordings.
- Remote IP resolution is cached in `/tmp/muxr-ip-<instance>` with 5-min TTL.

## Testing

Tests use `tempfile` for config fixtures and `Tmux::new(Some(...))` for
socket-isolated tmux servers that do not collide with production sessions.

Pure functions to test directly: `hex_to_ansi`, `format_duration`,
`format_age`, `parse_hex_color`, `context_bar`, `cache_ratio`,
`tool_command`, `health_filename`, `Remote::instance_name`,
`Config::color_for`, `Config::all_names`, `filter_entries`.
