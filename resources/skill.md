---
name: muxr
description: How to drive muxr, the tmux session manager. Use when launching, attaching, listing, choosing, saving, restoring, upgrading, retiring, sharding, broadcasting to sessions, migrating the on-disk layout, or when unsure what to pass as the first argument. Resolves the common slip of passing a directory name where muxr expects a repo key from the config. Trigger on "open a muxr session", "which repo", "muxr won't open", "Unknown repo", "upgrade my running sessions", "move sessions onto a new Claude Code", "shard this out", "restore after reboot".
allowed-tools: Bash(muxr *) Bash(muxr) Read
---

# muxr

*This skill is emitted by `muxr skill` (compiled into the binary), so it
never drifts from the installed version.*

muxr manages tmux sessions for AI coding harnesses. A session name is always
two levels: **`<repo>/<campaign>`** (or `<repo>/switchboard` for the
per-repo dispatcher). One session per campaign.

- **repo** — a coding repo muxr opens, an entry in the `[repos]` map of
  muxr's config (`~/.config/muxr/config.toml`).
- **campaign** — a long-lived initiative within that repo, a directory at
  `<repo>/campaigns/<campaign>/` holding `campaign.md` (metadata +
  conventions, with `category:` in frontmatter) and `log.md` (entrypoint +
  dated log). Kebab-case, topical, never a date.

## The one thing to get right: the first arg is a repo *key*

The first positional to `muxr` is a **repo key** from `[repos]`, not an
arbitrary path. The key usually matches the repo's directory name, but the
config is the source of truth — so **inspect, don't guess**:

```bash
muxr ls          # active sessions, listed as <repo>/<campaign>
grep -A1 '\[repos' ~/.config/muxr/config.toml   # or read the keys directly
```

Passing an unknown key fails with **"Unknown repo or remote"** and lists the
known keys. Use one of those.

## Launch grammar (no subcommand keyword)

```bash
muxr <repo>              # open/attach the repo switchboard
muxr <repo> <campaign>   # open/attach (or scaffold) a campaign session
muxr                     # bare: the control-plane shell
muxr switch              # interactive chooser (see below)
```

If the campaign doesn't exist yet, muxr scaffolds a stub and the agent
onboards it conversationally on first launch.

## The chooser (`muxr switch`)

`muxr switch` opens an interactive TUI that merges everything you can act on
into one list, grouped by repo:

- **live sessions** — Enter attaches.
- **dormant campaigns** (on disk, not running) — Enter launches them.
- **`+ new campaign…`** per repo — Enter prompts for a slug and creates it.

Shards render indented under their hub. Keys: `j/k` move, `/` filter, `enter`
switch/open/create, `a` show all/active, `n` new campaign, `c` recycle a live
session (serialize → fresh), `x` archive a dormant campaign, `r` rename, `d`
kill, `q` quit.

## Sharding: many topics under one hub

When a specific question crystallizes inside a broad campaign (e.g. a
customer hub), shard it into its own sibling campaign rather than adding a
third name level:

```bash
muxr shard <new-campaign>                 # from inside a session: shard the current campaign
muxr shard <new> --repo <r> --from <hub>  # out of session: name the hub explicitly
```

The shard inherits the hub's `category:` and records `sharded_from: <hub>`,
so the chooser groups it under its hub. Then it launches.

## The system prompt is a pointer, not a snapshot

muxr composes the launch system prompt as: repo HARNESS rules + the
campaign's what/how + a **pointer** — the one-line `entrypoint` plus the
absolute paths of `campaign.md` and `log.md` and a standing instruction to
re-read them. It deliberately does **not** inline the growing log body: a fat
prompt is resent every turn (burning the context window that forces
compaction) and goes stale the moment you `/serialize`.

Because the system prompt survives `/compact`, the re-read instruction
survives even as the conversation summary decays. So the durable source of
truth is always the on-disk `campaign.md` + `log.md`, not the prompt snapshot.

**After a `/compact`, re-orient from disk** rather than trusting the lossy
summary:

```bash
muxr reorient            # nudge the current session to re-read its files now
muxr reorient <repo>/<campaign>
```

`/serialize` is how you **move the pointer**: keep `log.md`'s `entrypoint` a
tight "where we are / what's next" line so a reorient (or a fresh launch)
re-anchors in seconds.

### Recycle instead of compact-looping

`/compact` summarizes the conversation, and repeating it compounds loss — the
working context drifts from the project intention. When a session fills up or
feels drifted, **recycle** instead: serialize, then reopen a FRESH conversation
that rehydrates from the durable on-disk pointer.

```bash
muxr recycle                     # /serialize -> graceful exit -> reopen fresh from the pointer
muxr <repo> <campaign> --fresh   # open a new conversation (don't resume)
```

The previous conversation stays on disk (recoverable via `--resume`), so
recycling never destroys context — it trades a degrading summary for a clean
read of the authoritative state. Default `muxr <repo> <campaign>` resumes the
last conversation; `--fresh` / `recycle` is the deliberate reset.

## Lifecycle verbs

| Command | What it does |
|---|---|
| `muxr ls` / `muxr ls --active` | List sessions (all / only those with a running harness) |
| `muxr switch` | Interactive chooser: switch / open dormant / create |
| `muxr shard <new>` | Spin a topic out of the current campaign into a sibling |
| `muxr reorient [name]` | Nudge a session to re-read its campaign.md + log.md (use after `/compact`) |
| `muxr recycle [name]` | Serialize → exit → reopen FRESH from the pointer (the alternative to compact-looping) |
| `muxr archive <campaign>` | Move a campaign to `campaigns/archive/` so it leaves the chooser (reversible); `x` in the chooser does the same |
| `muxr save` | Snapshot all sessions (name, dir, tool, session id) |
| `muxr restore` | Recreate snapshotted sessions after a reboot, resuming each in place |
| `muxr upgrade [name]` (alias `migrate`) | Move running sessions onto the freshly installed binary, in place. `--dry-run`, `--tool`, `--model`. Omit name for all |
| `muxr retire <name>\|all` | Graceful `/exit` + kill; **drops** the session from saved state (won't return on restore) |
| `muxr kill <name>\|all` | Kill the tmux session; leaves saved state intact |
| `muxr broadcast [/cmd]` | Send a slash command (default `/reload`) to every harness session |
| `muxr rename <new>` | Rename current session: tmux + on-disk + runtime relink |
| `muxr migrate-layout <repo>` | Migrate a repo's `campaigns/` tree to the 2-level model (`--dry-run` first) |
| `muxr completions <shell>` | Shell completions (zsh, bash, fish) |
| `muxr skill` | Emit this skill file |

Notes:
- There is no `muxr list` (use `ls`), no `muxr show`, no bare `muxr status`
  (the two status commands are scoped: `tmux-status`, `claude-status`).
- `retire` vs `kill`: retire when work is **done** (drops it from restore);
  kill when you want the pane gone but intend to bring it back. `upgrade`
  relaunches live work onto a new binary — it is not kill+open.

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
