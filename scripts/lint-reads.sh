#!/usr/bin/env bash
# lint-reads.sh -- enforce muxr's single file-read invariant (v3.7.0).
#
# Every PRODUCTION file read must route through src/primitives.rs
# (`read_text`, or `load_optional` / `load_campaign` / `load_log` built on it),
# so raw `fs::read_to_string` appears NOWHERE else outside primitives.rs and
# `#[cfg(test)]` modules. Centralizing the read is what lets each call site
# state its fail-loud-vs-degrade intent explicitly (`?`, a deliberate `.ok()`,
# or `load_optional`) instead of the ad-hoc `read_to_string(..).ok()` sites that
# silently swallowed real parse errors (issues #10/#11). This lint is the
# enforceable backstop for that invariant: a new raw read in production code
# fails the pipeline.
#
# Test code (inside a `#[cfg(test)]` module) may read files freely.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

fail=0
# `find` (not a flat `src/*.rs` glob) so a future submodule directory can't
# silently escape the lint.
for f in $(find src -name '*.rs' | sort); do
  [ "$f" = "src/primitives.rs" ] && continue
  # Production code is everything BEFORE the top-level test module. Anchor on a
  # COLUMN-0 `#[cfg(test)]` (the `mod tests` attr) -- NOT an indented one, which
  # marks a test-only helper item inside a production impl (e.g. config.rs's
  # `#[cfg(test)] pub fn builtin_claude`). Keying on any `#[cfg(test)]` would
  # wrongly treat all code after such a helper as test and miss production reads.
  # `|| true`: a grep with no match exits 1, which under `set -o pipefail`
  # would abort the whole lint on the first clean file. No-match is normal here.
  test_line="$(grep -n '^#\[cfg(test)\]' "$f" | head -1 | cut -d: -f1 || true)"
  limit="${test_line:-999999}"
  # Ban both the string read and the byte read (`fs::read(`); neither should
  # appear in production outside primitives.rs.
  hits="$(grep -nE 'read_to_string|fs::read\(' "$f" | awk -F: -v lim="$limit" '$1 < lim' || true)"
  if [ -n "$hits" ]; then
    echo "FAIL $f: raw file read in PRODUCTION code:" >&2
    echo "$hits" | sed 's/^/    /' >&2
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  {
    echo ""
    echo "Route production file reads through src/primitives.rs:"
    echo "  primitives::read_text(path)?              -- fail loud (the default)"
    echo "  primitives::read_text(path).ok()          -- DELIBERATE best-effort probe"
    echo "  primitives::load_optional(path, loader)?  -- split absent from present-but-broken"
    echo "  primitives::load_campaign / load_log      -- typed frontmatter loaders"
  } >&2
  exit 1
fi

echo "OK read-path invariant holds: no raw read_to_string in production code outside primitives.rs"
