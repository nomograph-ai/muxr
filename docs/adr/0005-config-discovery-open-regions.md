# ADR 0005: Config discovery + open extension regions (config, not a rebuild)

- Status: Accepted (discovery mechanism amended by [ADR 0012](0012-discovery-trusts-explicit-harness-allowlist.md))
- Date: 2026-07-07
- Implemented in: 3.6.0

> **Amendment (v4.0.0, ADR 0012):** the ambient `[discovery].roots` namespace
> WALK described below is replaced by an explicit `[discovery].harnesses`
> allowlist read directly (no directory walk). Trust is by explicit list, not by
> location. The open extension regions (`ext`) and the fail-loud-on-present /
> skip-on-absent posture in this ADR stand unchanged; only the discovery
> mechanism changed. Read this ADR's discovery sections as historical.

## Context

muxr read one config file with a rigid, `deny_unknown_fields` schema, and that
file was a single shared artifact across machines. Two recurring pains fell out
of that shape. First, a machine that lacked something the shared config named
(a repo it hasn't cloned, an absolute-path extension it doesn't have) could not
use the config as-is, so it carried a per-machine override -- exactly the drift
we wanted to kill. Second, any preference or launcher field that was not already
in muxr's schema (statusline chrome, a new launch hint) meant editing a Rust
struct: a compile and a release. The operator's directive was to move the seams
so estate- and preference-shaped concerns leave the core: adding chrome or a
launcher field should be a config change, never a muxr rebuild.

## Decision

muxr does two things. (1) **Discovery**: when `[discovery].roots` is set, muxr
finds `muxr.toml` fragments at git-repo roots under those namespace roots and
merges their `repos`/`remotes` into the loaded config. Config becomes drop-in
per repo; a repo absent on a machine is simply not discovered, so the machine
has zero knowledge of it. (2) **Open regions**: each repo carries an open
`[repos.<name>.ext]` namespace that muxr never interprets and hands to
extensions verbatim (the resolver intent's `ext`, and the `muxr config` query).
Preference/launcher data lives there as config. The core repo keys keep
`deny_unknown_fields`, so a typo in `dir`/`color` still fails loud; only `ext`
is open.

## Consequences

- Retires per-machine config overrides at the root; cross-machine knowledge
  becomes a property of what is cloned, not what is hand-edited out.
- Preferences (chrome, glyph) live outside the core and are read by extensions
  (the statusline, the glyph builder) via `muxr config`; a new preference field
  never triggers a muxr release.
- Backward-compatible and opt-in: empty `[discovery].roots` (the default) is
  byte-identical to the pre-3.6 single-file config.
- Non-goals: muxr does not parse or validate `ext`; this is not a plugin system.
  Fragment discovery is a bounded 2-level namespace/repo walk, not a general
  recursive scan.

## Design detail

- `[discovery] { roots: Vec<String>, fragment = "muxr.toml" }`. Walk is
  `<root>/<ns>/<repo>/<fragment>`, gated on `<repo>/.git`, sorted for a
  deterministic merge; duplicate repo/remote names fail loud; repo/remote/tool
  collisions are re-validated after the merge.
- `Repo.ext: toml::Table` (a named open sub-table, NOT `#[serde(flatten)]` --
  flatten is incompatible with `deny_unknown_fields`, which would forfeit the
  typo-catching on core keys).
- `ResolveIntent` gains `ext` (omitted when empty, so intents stay
  byte-compatible for repos with no `ext`). `muxr config` emits
  `{ "repos": { "<name>": { "color", "ext" } } }`.

## Alternatives considered

- **A config preprocessor** that assembles `config.toml` from fragments via a
  build step. Rejected: "write configs, then run a preprocessor" is poor DX for
  anyone who runs the tool, and it keeps muxr blind to the fragment model.
- **Hand-maintained per-machine configs.** Simple, but reintroduces drift and
  is not self-describing; zero-knowledge stops being automatic.
- **`#[serde(flatten)]` catch-all on `Repo`.** Incompatible with
  `deny_unknown_fields`; adopting it would silently absorb a typo in a core key.
