# ADR 0011: Discovered fragments carry only repos/remotes (no auto-exec config)

- Status: Accepted
- Date: 2026-07-20
- Deciders: operator + P3-10 pre-tag security review
- Supersedes: decision **[f]** in the v4 design brief (§9, 2026-07-17) -- "move
  hooks/resolver/session_env/tools per-fragment for full self-containment."
- Relates to: [ADR 0005](0005-config-discovery-open-regions.md) (config
  discovery), [ADR 0001](0001-extension-architecture.md)
- Implemented in: 4.0.0 (`discover_and_merge` in `src/config.rs`)

## Context

muxr discovers per-repo `muxr.toml` fragments under `[discovery].roots` and
merges them into one effective config (ADR 0005). The v4 build initially (P3-7)
went further, per decision [f]: it PRE-MERGED a fragment's `[hooks]`,
`[extensions].resolver`, `[session_env]` and `[tools.*]` into the effective
config, and added compiled default discovery roots (`~/gitlab.com`,
`~/github.com`) plus `MUXR_ROOTS` so a bootstrap-only config self-discovers with
zero per-machine `[discovery]`.

The pre-tag security review found that this turns a `muxr.toml` in ANY discovered
repo into an AUTO-EXECUTED code surface, reached on the next `muxr <repo>
<campaign>`:

- `hooks.pre_create` runs each string via `sh -c` on every session open/restore.
- a fragment `extensions.resolver` runs via `sh -c` on open.
- fragment `session_env` injects environment (e.g. `PATH`/`LD_PRELOAD`) into the
  launched runtime.
- fragment `[tools.*]` defines the `bin`/`wrapper`/`args` a tool launches.

Combined with discovery being on-by-default over broad roots that legitimately
contain cloned/external repos (the `research` category clones third-party repos
under exactly these roots), a hostile `muxr.toml` executes with no allowlist
(unlike direnv's `direnv allow`) and as a silent post-upgrade behavior change.

## Decision

**A discovered fragment contributes ONLY `repos` and `remotes` -- its identity,
which is the safe part.** The auto-exec-adjacent config
(`hooks`/`extensions.resolver`/`tools`/`session_env`) stays in the
operator-owned bootstrap config, and is NEVER sourced from a discovered
fragment. Any top-level key in a fragment other than
`repos`/`remotes`/`schema_version` is a hard error.

Discovery stays **opt-in** via an explicit `[discovery].roots` block; there are
no compiled default roots and no `MUXR_ROOTS`. This reverts P3-7's pre-merge and
compiled roots.

The anti-drift goal that motivated [f] is preserved WITHOUT the auto-exec
surface: the bootstrap config travels via the operator layer
(`dunn.dev/harness`, symlinked into `~/.config/muxr/`), so it is byte-identical
on every machine -- there is no per-machine drift, and it is operator-owned and
trusted.

## Consequences

- Closes the "any cloned repo can run code via a config file muxr reads
  automatically" surface -- the single most severe pre-tag finding.
- Keeps discovery's real value (a repo declares its own `[repos.<name>]` identity
  with no central edit, ADR 0005) and the anti-drift win (the trusted bootstrap
  travels via the operator layer).
- The "machine residue -> zero files" aspiration softens to "one small
  `[discovery]` block plus the exec fields, all in the traveling bootstrap" --
  acceptable, because that is one operator-owned file that travels, not
  per-machine drift.
- P3-11 rollout no longer moves hooks/session_env into fragments; the estate's
  bootstrap already carries them, so no estate config change is needed.

## Alternatives considered

- **Keep [f]; document the trust model** ("only clone repos you trust under your
  roots"). Rejected: the auto-exec risk is real given external repos under the
  roots, and a documented footgun is still a footgun before an irreversible tag.
- **direnv-style per-fragment allowlist** (record a hash; `muxr config allow`).
  Rejected: a sizable new feature, over-engineered for a single-operator tool
  when the exec fields can simply live in the trusted bootstrap.
- **Opt-in roots but keep the exec fields in fragments.** Rejected: a hostile
  fragment under a configured root still auto-execs; only removes the
  default-on/silent-upgrade surprise, not the core hole.
