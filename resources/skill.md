---
name: muxr
description: How to drive muxr, the tmux session manager. Use when launching, attaching, listing, saving, restoring, upgrading, retiring, or broadcasting to sessions, or when unsure what to pass as the first argument. Resolves the common slip of passing a repository/directory name where muxr expects a harness key from the config. Trigger on "open a muxr session", "which harness", "muxr won't open", "Unknown harness", "upgrade my running sessions", "move sessions onto a new Claude Code", "restore after reboot".
allowed-tools: Bash(muxr *) Bash(muxr) Read
---

# muxr

*This skill is emitted by `muxr skill` (compiled into the binary), so it
never drifts from the installed version.*

muxr manages tmux sessions for AI coding harnesses. A session name is
always `<harness>/<campaign>/<topic>` (or `<harness>/switchboard` for the
singleton dispatcher).

## The one thing that trips people up: harness key != repo name

The first positional to `muxr` is a **harness key** — an entry in the
`[harnesses]` map of muxr's config (`~/.config/muxr/config.toml`). It is
**not** the repository or directory name. A harness is *rooted in* a repo
whose directory name often differs from the key.

So passing a repo/directory name where a harness key is expected fails with
**"Unknown harness"**. The mapping is config-defined and varies per machine
— so **inspect first, don't guess from the directory name**:

```bash
muxr ls          # active sessions, listed as <harness>/<campaign>/<topic>
# or read the keys directly:
grep -A1 '\[harnesses' ~/.config/muxr/config.toml
```

Use the harness key the config defines, never the repo folder name.

## Launch grammar (no subcommand keyword)

```bash
muxr <harness>                      # open/attach the harness switchboard
muxr <harness> <campaign> <topic>   # open/attach a campaign session
```

- `<campaign>` is a category of work — a directory under
  `<repo>/campaigns/`.
- `<topic>` is a kebab-case initiative slug (letter/digit start, no
  slashes). Topical, never a date.
- If the campaign or session doesn't exist yet, muxr scaffolds it and you
  confirm in-flow.

## Lifecycle verbs

| Command | What it does |
|---|---|
| `muxr ls` / `muxr ls --active` | List sessions (all / only those with a running harness) |
| `muxr switch` | Interactive TUI picker |
| `muxr save` | Snapshot all sessions (name, dir, tool, session id) |
| `muxr restore` | Recreate snapshotted sessions after a reboot, resuming each in place |
| `muxr upgrade [name]` (alias `migrate`) | Move running sessions onto the freshly installed binary, in place. `--dry-run`, `--tool`, `--model`. Omit name for all; pass one for a single session |
| `muxr retire <name>\|all` | Graceful `/exit` + kill; **drops** the session from saved state (won't return on restore) |
| `muxr kill <name>\|all` | Kill the tmux session; leaves saved state intact |
| `muxr broadcast [/cmd]` | Send a slash command (default `/reload`) to every harness session |
| `muxr rename <new>` | Rename current session: tmux + on-disk session file + runtime relink |
| `muxr completions <shell>` | Shell completions (zsh, bash, fish) |
| `muxr skill` | Emit this skill file |

Notes:
- There is no `muxr list` (use `ls`), no `muxr show`, no bare `muxr status`
  (the two status commands are scoped: `tmux-status`, `claude-status`).
- `retire` vs `kill`: retire when the work is **done** (drops it from
  restore); kill when you want the pane gone but intend to bring it back.
  `upgrade` relaunches live work onto a new binary — it is not kill+open.

## Upgrading running sessions onto a new harness version

When a new harness binary (e.g. a new Claude Code) lands and you want your
long-running sessions on it without losing their conversations, use
`muxr upgrade` (alias `muxr migrate`) — NOT raw `tmux`, and don't hand-roll
session discovery:

```bash
muxr upgrade --dry-run     # see exactly what would relaunch, touch nothing
muxr upgrade <one-session> # try a single session first
muxr upgrade               # then all of them
muxr upgrade --model opus  # also switch model on relaunch
```

Each target gets a graceful `/exit`, then a relaunch with its full composed
command (system prompt + working dirs + `--resume`) on the binary `muxr` now
resolves to. Run it from the `muxr` control shell, not from inside an agent
session (process discovery can't see sibling panes from within one).

## Save / restore around reboots

```bash
muxr save        # before reboot
muxr restore     # after reboot — recreates each session, resuming in place
```

The bare `muxr` control-plane shell is intentionally not saved or restored;
relaunch it manually after `muxr restore`.
