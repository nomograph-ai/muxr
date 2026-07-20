---
name: muxr
description: How to drive muxr, the tmux session manager. Use when launching, attaching, listing, choosing, saving, restoring, upgrading, retiring, sharding, broadcasting to sessions, migrating the on-disk layout, or when unsure what to pass as the first argument. Resolves the common slip of passing a directory name where muxr expects a repo key from the config. Trigger on "open a muxr session", "which repo", "muxr won't open", "Unknown repo", "upgrade my running sessions", "move sessions onto a new Claude Code", "shard this out", "restore after reboot".
allowed-tools: Bash(muxr *) Bash(muxr) Read
---

# muxr

*This skill is emitted by `muxr skill` (compiled into the binary), so it
never drifts from the installed version.*

muxr manages tmux sessions for AI coding tools. A session name is always
two levels: **`<repo>/<campaign>`** (or `<repo>/switchboard` for the
per-repo dispatcher). One session per campaign.

- **repo** -- a coding repo muxr opens, an entry in the `[repos]` map of
  muxr's config (`~/.config/muxr/config.toml`).
- **campaign** -- a long-lived initiative within that repo, a directory at
  `<repo>/campaigns/<campaign>/` holding `campaign.md` (metadata +
  conventions, with `category:` in frontmatter) and `log.md` (entrypoint +
  dated log). Kebab-case, topical, never a date.

## Intent → command (reach for the verb; don't hand-roll tmux)

muxr owns tmux session lifecycle. For any of these, run the muxr verb
**directly** -- do not first inspect raw `tmux`/files or build your own
`tmux` script, and do not use raw `tmux` to do the job muxr has a verb for.

| You want to… | Run |
|---|---|
| see what's running | `muxr ls` |
| **save layout before a reboot** | `muxr save` (never hand-roll a `tmux` save script) |
| bring it back after reboot | `muxr restore` |
| finish a session for good | `muxr retire <name>` |
| open / resume a campaign | `muxr <repo> <campaign>` |
| pick one interactively | `muxr switch` |
| continue cleanly after a full context | `muxr recycle` (not repeated `/compact`) |
| re-anchor after a `/compact` | `muxr reorient` |
| spin a topic off | `muxr shard <new>` |
| declutter a stale campaign | `muxr archive <campaign>` |
| move sessions onto a new tool binary | `muxr upgrade` |

## The one thing to get right: the first arg is a repo *key*

The first positional to `muxr` is a **repo key** from `[repos]`, not an
arbitrary path. The key usually matches the repo's directory name, but the
config is the source of truth -- so **inspect, don't guess**:

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

- **live sessions** -- Enter attaches.
- **dormant campaigns** (on disk, not running) -- Enter launches them.
- **`+ new campaign…`** per repo -- Enter prompts for a slug and creates it.

Shards render indented under their hub. Keys: `j/k` move, `/` filter, `enter`
switch/open/create, `a` show all/active, `n` new campaign, `c` recycle a live
session (flush → fresh), `x` archive a dormant campaign, `r` rename, `d`
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
campaign's what/how + a **pointer** -- the one-line `entrypoint` plus the
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

### Flushing state to the pointer (the serialize procedure)

"Serializing" is not a special command -- it's **flushing your state into every
locale you've been working in so a fresh session can resume**. muxr owns the
`log.md` format, so the procedure lives here (no separate skill to drift out of
sync with the layout). A campaign's work spans repos: the narrative/pointer
lives in the harness repo, but deliverables land in the **project repos** (the
campaign's `paths:`). Flush to all of them:

1. **The pointer** -- edit `campaigns/<campaign>/log.md`: set `entrypoint:` to a
   tight, current "where we are / what's next" line (the first thing a fresh
   session reads), and append a dated entry under `## Log` with state,
   decisions, and open threads.
2. **Each project repo you touched** -- make in-flight work durable: commit it,
   or record the branch + uncommitted changes + next step in the log entry so
   nothing is stranded outside the harness repo.

Keep `campaign.md` (the what/how) updated too when conventions change. This
multi-locale flush is what makes a fresh launch or a `reorient` re-anchor in
seconds, across every repo the campaign spans.

### Recycle instead of compact-looping

`/compact` summarizes the conversation, and repeating it compounds loss -- the
working context drifts from the project intention. When a session fills up or
feels drifted, **recycle** instead: flush state to the pointer, then reopen a
FRESH conversation that rehydrates from it.

```bash
muxr recycle <repo>/<campaign>   # or: muxr switch -> c
muxr <repo> <campaign> --fresh   # open a new conversation (don't resume)
```

`muxr recycle` is a positive-signal handshake, entirely muxr-owned (no external
skill):

1. muxr sends a **flush prompt** into the pane; the agent flushes its state to
   `log.md` (the procedure above) and, when done, writes a small done-signal file
   whose path the prompt hands it.
2. muxr **waits for that signal** -- the agent's positive "flush complete," never
   a guess from idle bytes -- then drives `/exit`, waits for the pane to return to
   its shell, and reopens FRESH from the pointer.

The previous conversation stays on disk (recoverable via `--resume`), so
recycling never destroys context -- it trades a degrading summary for a clean
read of the authoritative state. If the flush never signals, muxr aborts and
leaves the session untouched (fail-safe: no signal, no exit).

Run it from the control shell or `muxr switch` (`c`). To recycle the session you
are **inside**, spawn it detached so it survives your own exit, then stop:

```bash
setsid muxr recycle "$(tmux display-message -p '#{session_name}')" </dev/null \
  >/tmp/muxr-recycle.log 2>&1 &    # then STOP; muxr drives the flush + exit + reopen
```

The flush prompt is a muxr default (generic, references this log.md procedure); a
harness overrides it via `[recycle].flush_prompt` in its config -- tokens
`{session} {repo} {campaign} {log} {sentinel}` -- e.g. to compose a richer
`durable`-style flush.

## Lifecycle verbs

| Command | What it does |
|---|---|
| `muxr ls` / `muxr ls --active` | List sessions (all / only those with a running tool) |
| `muxr switch` | Interactive chooser: switch / open dormant / create |
| `muxr shard <new>` | Spin a topic out of the current campaign into a sibling |
| `muxr reorient [name]` | Nudge a session to re-read its campaign.md + log.md (use after `/compact`) |
| `muxr recycle [name]` | Flush (via a prompt muxr sends) → exit → reopen FRESH from the pointer (the alternative to compact-looping) |
| `muxr archive <campaign>` | Move a campaign to `campaigns/archive/` so it leaves the chooser (reversible); `x` in the chooser does the same |
| `muxr save` | Snapshot all sessions (name, dir, tool, session id) |
| `muxr restore` | Recreate snapshotted sessions after a reboot, resuming each in place |
| `muxr upgrade [name]` (alias `migrate`) | Move running sessions onto the freshly installed binary, in place. `--dry-run`, `--tool`, `--model`. Omit name for all |
| `muxr retire <name>\|all` | Graceful `/exit` + kill; **drops** the session from saved state (won't return on restore) |
| `muxr kill <name>\|all` | Kill the tmux session; leaves saved state intact |
| `muxr broadcast [/cmd]` | Send a slash command (default `/reload`) to every tool session |
| `muxr rename <new>` | Rename current session: tmux + on-disk + runtime relink |
| `muxr config migrate` | Rewrite this repo's `muxr.toml` fragment to the current schema (`--write` applies; dry-run by default) |
| `muxr migrate-layout <repo>` | Migrate a repo's `campaigns/` tree to the 2-level model (`--dry-run` first) |
| `muxr completions <shell>` | Shell completions (zsh, bash, fish) |
| `muxr skill` | Emit this skill file |

Notes:
- There is no `muxr list` (use `ls`), no `muxr show`, no `muxr status`.
  `tmux-status` emits the tmux status-left config; the in-pane statusline is the
  runtime's own concern (point its statusline command at your renderer).
- `retire` vs `kill`: retire when work is **done** (drops it from restore);
  kill when you want the pane gone but intend to bring it back. `upgrade`
  relaunches live work onto a new binary -- it is not kill+open.

## Upgrading running sessions onto a new tool version

When a new tool binary (e.g. a new Claude Code) lands and you want your
long-running sessions on it without losing their conversations, use
`muxr upgrade` (alias `muxr migrate`) -- NOT raw `tmux`, and don't hand-roll
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
muxr restore     # after reboot -- recreates each session, resuming in place
```

The bare `muxr` control-plane shell is intentionally not saved or restored;
relaunch it manually after `muxr restore`.
