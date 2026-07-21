# ADR 0012: Discovery trusts an explicit harness allowlist, not ambient roots

- Status: Accepted
- Date: 2026-07-20
- Deciders: operator + P3-10 Fable adversarial pass + Fable design-review
- Amends: [ADR 0005](0005-config-discovery-open-regions.md) (the discovery
  *trust mechanism*; its open `ext` regions are unaffected)
- Supersedes: [ADR 0011](0011-fragments-carry-only-identity.md) (the fragment
  field allow-list -- mooted by this model)
- Relates to: [ADR 0001](0001-extension-architecture.md), [ADR 0006](0006-fail-loud-on-unparseable-session-files.md)
- Implemented in: 4.0.0 (`[discovery].harnesses` in `src/config.rs`)

## Context

muxr discovers per-repo `muxr.toml` fragments and merges them into one effective
config. Since 3.6 (ADR 0005) discovery has decided *what to trust by ambient
location*: `[discovery].roots` names broad namespace roots (the estate sets
`~/gitlab.com`, `~/github.com`), and muxr walks every git repo under them,
merging any `muxr.toml` it finds.

A pre-tag Fable adversarial review found that this is an auto-exec surface. A
discovered fragment is merged as a full `Config`, and a `Repo` is not identity:
it carries `launch` (`wrapper`, `append_system_prompt_file(s)`, `add_dirs`) and
`viewer` (`cmd`), which reach `sh -c` / `tmux split-window` / arbitrary-file
reads on the next session open. So a hostile `muxr.toml` in ANY repo cloned
under a root executes code with no allowlist. It is live in the estate (the
`research` category clones third-party repos under exactly these roots) and has
shipped since 3.6. Ambient trust has a second failure mode a design-review
surfaced: any cloned repo can brick muxr estate-wide simply by declaring a
duplicate `[repos.<name>]` -- duplicate-name detection is a hard error inside
`Config::load`, so an unrelated hostile fragment is a config-load DoS.

ADR 0011 tried to fix the exec surface by restricting a fragment to
`repos`/`remotes` and hard-erroring other *top-level* keys. That check is
top-level only -- it never looked inside a repo table -- so `[repos.x.launch]` /
`[repos.x.viewer]` still cross. And it cannot be tightened to a field allow-list
without breaking the estate: the operator's own harness fragments
(keaton/storr/tanuki) legitimately use `launch` (per-repo HARNESS.md) and
`viewer`. The useful fields ARE the dangerous fields. The real fault is upstream
of any field list: discovery trusts by *where a file sits*, which conflates the
operator's 3 harnesses with the 75-plus (and growing) repos that happen to live
under the roots.

## Decision

muxr discovers config ONLY from an explicit, operator-listed set of harness repo
roots:

```toml
[discovery]
harnesses = [
  "~/gitlab.com/andunn/tanuki",
  "~/gitlab.com/dunn.dev/storr",
  "~/gitlab.com/nomograph/keaton",
]
```

muxr reads `<harness>/muxr.toml` directly for each listed path -- no directory
walk, no scanning. A repo muxr was not told about is never read.

Because every discovered fragment is now operator-designated, it is **trusted**.
A trusted fragment contributes its full `[repos.*]` (the **complete** `Repo`,
including the nested `launch` / `viewer` / `ext` the estate needs) and its
`[remotes.*]`. There is no untrusted-fragment case left, so there is no field
allow-list on the repo contents.

**Scope, deliberately tight.** Every OTHER top-level key in a fragment
(`hooks`, `extensions`, `tools`, `session_env`, `layout`, `chooser`, `recycle`,
`default_tool`, a global `viewer`, and `discovery` itself) remains a **hard
error** -- not now as a security boundary but because those are
*bootstrap-owned singletons*: merging N fragment copies of a single-valued
setting is ill-defined, and `[discovery]` in a fragment would be transitive
trust delegation / recursive discovery. Reinstating per-repo
`hooks`/`extensions`/`session_env`/`tools` (the old decision `[f]`) is now
*safe* under the trust model, but it needs merge semantics of its own and is
therefore **deferred to its own ADR**, not smuggled in here.

The pre-4.0 `[discovery].roots` broad-namespace walk is removed.

## Consequences

- **The untrusted-fragment CLASS is eliminated, at the root.** No walk, no
  ambient read: a hostile `muxr.toml` anywhere under any directory is never
  opened. The nested `launch`/`viewer` bypass ADR 0011 missed, the parsing of
  untrusted TOML, the symlink-walk finding, and the duplicate-name config-load
  DoS all die together. Membership shrinks from "75+ repos -- anything that gets
  cloned" to "the 3 repos the operator names."
- **A residual trust remains, and is accepted (named, not implied away).** This
  is trust-on-path: the *future contents* of the 3 listed repos are auto-trusted
  (a merged MR, a compromised remote, or an agent in one harness editing a
  sibling's `muxr.toml` via `add_dirs` reachability, then `launch.wrapper` ->
  `sh -c` on next open). This is NOT a new class -- those repos already program
  the agent (their `HARNESS.md` is auto-appended to the system prompt; the
  bootstrap itself is `git pull`ed by a `pre_create` hook and carries
  hooks/wrappers). 0012 shrinks *membership* of an existing trust class; it does
  not add one. The direnv-style hash-allow (below) is the only alternative that
  would cover this residual, and rejecting it is exactly the choice to accept it.
- **ADR 0011 is mooted.** Its field allow-list and its security rationale for
  hard-erroring unknown fragment keys no longer apply. The top-level hard error
  stays, recast as "bootstrap-owned singletons," and `deny_unknown_fields`
  continues to catch a misspelled `dir`/`color` (typo hygiene).
- **The original intentions are served better than by ambient roots.**
  Self-contained config still lives in each harness repo and travels/versions
  with it. Zero per-machine drift holds: the harness list lives in the one
  operator-owned bootstrap (the traveling `~/.config/muxr/` symlink), and a
  machine lacking a listed repo simply finds no dir there and skips it. "muxr is
  multi-config = it reads N harness configs" becomes literal (N is the list).
- **Cost:** adding a harness is a one-line bootstrap edit rather than "drop a
  `muxr.toml` and it is picked up automatically." Harnesses are added rarely
  (three today); the zero-edit ambient ingest was precisely the mechanism that
  created the hole. Accepted trade.
- **Non-goals:** unchanged -- ADR 0005's open `[repos.<name>.ext]` regions stay
  exactly as they are; this is not a plugin system; discovery stays opt-in (an
  absent/empty `harnesses` list is a single-file config, byte-identical to
  pre-discovery).

## Design detail

- `[discovery] { harnesses: Vec<String>, fragment = "muxr.toml" }`. Each entry
  is a repo-root path, **tilde-expanded only** (parity with the old `roots`:
  `shellexpand::tilde`, no env vars). Entries are canonicalized and de-duplicated
  (so `~/x`, the absolute form, and a symlink to the same dir read once, not
  twice into a spurious "duplicate repo" error). **List order** is the merge
  order (deterministic; the operator's own document); duplicate repo/remote
  names across fragments or vs the base still fail loud; repo/remote/tool
  collisions are re-validated after merge (unchanged from 0005).
- **Present-vs-absent, per ADR 0006** (this is the fail-loud boundary):
  - listed dir does NOT exist -> **silent skip** (the machine lacks that repo --
    the cross-machine story).
  - listed dir EXISTS but has no `muxr.toml` (a typo that resolves to a real
    dir, a listed non-repo, or a listed plain file) -> **fail loud**. Present but
    not what was named is a broken config, not an absent repo.
  - `muxr.toml` present but unparseable -> **hard error** (already the case via
    `parse_fragment`; stated here so it is not re-litigated).
- **Merge scope:** a trusted fragment contributes the full `Repo` (nested
  `launch`/`viewer`/`ext`, no stripping) + `remotes`. Any other top-level key ->
  hard error, message: "bootstrap-owned; a fragment carries only [repos.*] and
  [remotes.*]" (the P3-10 `offenders` check stays; only its wording changes).
  `[discovery]` inside a fragment is explicitly among the rejected keys (no
  recursive discovery).
- `[discovery].roots` is removed. A v4 binary that meets the old `roots` key
  fails loud with a message pointing at `harnesses`. The existing `rename_hint`
  machinery checks *top-level* keys only, so a NESTED-key hint entry
  (`discovery.roots -> discovery.harnesses`) must be added for the error to be
  actionable; otherwise the operator gets a raw serde "unknown field `roots`".
- **Migration is a single lockstep commit** to the estate bootstrap
  (`dunn.dev/harness/muxr/base.toml`): swap `[discovery].roots -> harnesses`
  AND stamp `schema_version = 2`. The stamp is load-bearing: without it a
  straggler 3.7.0 binary meeting the file gives a raw "unknown field `harnesses`"
  instead of the intended "upgrade muxr" (schema-version) message. `muxr config
  migrate` cannot mechanically convert `roots` (the walk found repos
  dynamically); it may offer a one-shot helper that walks the old roots ONCE and
  prints the discovered repo paths for the operator to paste -- convenience,
  never automatic trust.
- **Key-name caution:** `harnesses` reads right for this estate but is the exact
  word the retired v1->v2 rename rewrites (`KNOWN_RENAMES` and
  `FRAGMENT_MIGRATIONS` map the segment `harnesses -> repos`). It is safe as used
  -- `harnesses = [...]` is a key-value under `[discovery]`, not a table header,
  and both mechanisms act only on top-level keys / table headers -- but it is one
  refactor from corruption (`muxr config migrate` on a `[discovery.harnesses]`
  *header* would rewrite it to `[discovery.repos]`). Ship a regression test
  pinning that migrate + rename-hint leave `[discovery] harnesses = [...]`
  untouched.
- **Rollout skew:** a 3.7.0 binary meeting `[discovery].harnesses` (+
  `schema_version = 2`) fails loud "upgrade muxr" -- no silent mis-parse. The
  ~25 live sessions are unaffected: config load happens at `muxr` invocation, not
  in running tmux panes; the worst skew outcome is a fail-loud on the next
  invocation, which is the designed behavior.

### Implementation sweep (so the 0011 removal does not half-land)
- Flip [ADR 0011](0011-fragments-carry-only-identity.md) Status -> "Superseded
  by [0012]"; update `docs/adr/README.md`.
- Rewrite the `discover_and_merge` P3-10 security doc-comment and the
  `offenders`-check message to the bootstrap-owned-singleton rationale.
- Retarget the tests `discovery_rejects_exec_fields_in_fragment` /
  `discovery_rejects_disallowed_fragment_key` (they keep passing but assert the
  old security framing) + add: present-dir-no-fragment fail-loud, absent-dir
  skip, duplicate/aliased-entry dedupe, and the `harnesses`-key migrate/rename
  no-op.
- Estate: the 3 fragments' header comments say "discovered via [discovery].roots
  in the machine-global base" -- update in the base.toml swap commit (stale, not
  dangerous).

## Alternatives considered

- **A field allow-list on discovered repos (ADR 0011, tightened to nested
  fields).** Rejected: the estate's own fragments need `launch`/`viewer`, and
  those are the dangerous fields -- no allow-list both keeps the feature and
  closes the hole. It also leaves the ambient-trust class intact for whatever
  field is added next.
- **Trust by git origin namespace** (auto-trust a fragment whose repo origin is
  under an operator-owned namespace; no explicit list). Rejected: a clone
  controls its own `origin` git config, so it is spoofable -- not a robust trust
  boundary. An operator-owned list is.
- **Keep broad roots but narrow them by hand.** Rejected: still ambient -- it
  trusts everything under the narrowed root and does not design out the class;
  the operator re-loses the boundary the first time a clone lands there.
- **Soft-deprecate `roots`** (read-with-warning for one release instead of
  hard-removing). Rejected: a warning keeps the auto-exec walk alive for a full
  release, defeating the security fix; at N=1 operator controlling both sides of
  a lockstep swap in an already-breaking 4.0.0, the compat shim is pure cost.
- **direnv-style per-fragment hash allow (`muxr config allow`).** Closes the
  hole AND the trust-on-path residual and keeps zero-edit drop-in, but it is a
  sizable new feature (a trust store, a re-approve-on-change flow) --
  over-engineered for a single-operator tool at N=3. The explicit list is
  simpler and sufficient; accepting the residual (above) is the price.
- **No discovery at all -- a generic bootstrap `include = [paths]`.** Rejected:
  it converges to this design with worse semantics. `harnesses` *is*
  include-by-path, but scoped (skip-if-absent for a machine lacking the repo;
  restricted fragment schema) where a generic include would drag in the
  singleton-merge ambiguity of the deferred `[f]` and offer no place to enforce
  the repos+remotes-only scope.
