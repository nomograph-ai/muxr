# muxr extensions

muxr core is a runtime-agnostic tmux session engine: it knows the *verbs*
(`open · save · restore · recycle · upgrade · list · switch · kill · rename ·
fork · model-switch`) and nothing about any particular coding CLI. Everything
runtime-specific lives here, behind one of two extension surfaces.

This directory ships **reference** extensions. It is part of the muxr repo for
now (so the contract and its examples version atomically while the contract
settles); it is designed to split into a standalone `muxr-extensions` bundle
once external runtimes adopt and the contract is locked.

## Two kinds of extension

### 1. Declarative adapters (`adapters/*.toml`) -- reusable

A **runtime adapter** is pure config describing how to drive one CLI: launch /
resume / continue / fork / set-model / exit, and how to recover its session id.
Adding a runtime is writing one of these `[tools.<name>]` blocks -- there is no
per-runtime code in muxr. See the full field reference in
[`../ARCHITECTURE.md`](../ARCHITECTURE.md) (extension point #4).

Shipped here:
- [`adapters/claude.toml`](adapters/claude.toml) -- mirrors the compiled-in default.
- [`adapters/pi.toml`](adapters/pi.toml) -- string prompt mode, no `--add-dir`.
- [`adapters/opencode.toml`](adapters/opencode.toml) -- a third-party port done by
  config alone, annotated with where the shape **bends** (no `--name`, no
  `--add-dir`, no per-pid session file -> resume needs a resolver).

The adapter shape is the contract a new runtime (opencode, pi, anything)
implements. If your CLI's session id isn't a per-pid file, set
`session_discovery.type = "none"` and resolve the id in a resolver (below).

### 2. Imperative extensions (`examples/*.sh`) -- opinionated, user-owned

For logic a template can't express, muxr invokes a **subprocess**: JSON on
stdin, JSON on stdout, fail-closed, run a built-in default when absent. Two
points exist today:

- [`examples/resolver.sh`](examples/resolver.sh) -- `[extensions].resolver`. The
  single launch chokepoint: launch intent in, layout facts out. Omitted fields
  fall back to muxr's built-in `[layout]`.
- [`examples/make-durable.sh`](examples/make-durable.sh) -- `[extensions].make_durable`.
  Supplies the pre-recycle flush message; muxr always appends its own exit
  directive after it.

These are templates to copy, not drop-in installs -- they encode *your*
workflow. In practice they live in your own estate repo (e.g. a `configs/`
dir), referenced by absolute path in your muxr config. They are deliberately
**not** something muxr distributes for you; that is the whole point of the
subprocess contract (any language, any logic).

## Using an adapter

muxr does not yet auto-discover this directory. Until adapter `include`/glob
lands (tracked for 3.1), wire one of two ways:

1. **Copy** the `[tools.<name>]` block from an adapter into your muxr config.
2. **Reference** a subprocess extension by absolute path:

   ```toml
   [extensions]
   resolver     = "~/gitlab.com/dunn.dev/<estate>/configs/resolver.sh"
   make_durable = "~/gitlab.com/dunn.dev/<estate>/configs/make-durable.sh"
   ```

## Distribution

- **Adapters** are TOML. The blessed set ships in the muxr release; your own
  live in (or are `include`d from) your estate repo -- the same repo that pins
  muxr via mise. No registry, no package manager.
- **Subprocess extensions** distribute through git, in your estate repo. muxr's
  only job is to invoke them by path.
