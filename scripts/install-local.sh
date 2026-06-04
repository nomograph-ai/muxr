#!/usr/bin/env bash
#
# Build muxr (release) and install it to ~/.cargo/bin/muxr for local use.
#
# Why here: ~/.cargo/bin sits ahead of the kit/mise-managed muxr install dir
# on PATH, so this wins resolution AND survives the `kit sync` that runs from
# muxr's own pre_create hook (kit only manages the mise copy). Reverting is
# just `rm ~/.cargo/bin/muxr`.
#
# Why the re-sign: a plain `cp` of a freshly built arm64 binary invalidates its
# code signature, and macOS then SIGKILLs it on exec (rc=137, "killed: 9").
# Re-signing ad-hoc fixes it. `cargo install` would also work but rebuilds from
# scratch; this reuses the existing release artifact.
set -euo pipefail

cd "$(dirname "$0")/.."
cargo build --release

dest="$HOME/.cargo/bin/muxr"
cp target/release/muxr "$dest"

if command -v codesign >/dev/null 2>&1; then
  codesign --remove-signature "$dest" 2>/dev/null || true
  codesign -s - -f "$dest"
fi

hash -r 2>/dev/null || true
echo "installed muxr $("$dest" --version | awk '{print $2}') -> $dest"
