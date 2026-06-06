//! Consistent CLI output styling for muxr's own messages.
//!
//! A `muxr <repo> <campaign>` invocation otherwise reads as a pile of
//! different programs' stdout (kit/rune hooks) plus muxr's plain `eprintln`s.
//! This module gives muxr a single house style -- a colored repo band, aligned
//! key/value detail lines, and ✓/! status marks -- so the launch is one
//! coherent surface, matching the chooser TUI. Honors `NO_COLOR` and falls
//! back to plain text when stderr is not a terminal.

use std::io::IsTerminal;

/// Color is on only for an interactive stderr with NO_COLOR unset.
fn color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal()
}

/// Whether stderr is an interactive terminal. Drives transient progress
/// lines, which only make sense on a TTY (NO_COLOR notwithstanding).
fn is_tty() -> bool {
    std::io::stderr().is_terminal()
}

/// Carriage-return + clear-to-EOL on a terminal, so a status line
/// overwrites a preceding transient `step_start` line. Empty off a
/// terminal, keeping logs line-by-line.
fn line_reset() -> &'static str {
    if is_tty() { "\r\x1b[K" } else { "" }
}

fn sgr(codes: &str, s: &str) -> String {
    if color_enabled() {
        format!("\x1b[{codes}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn rgb(hex: &str, s: &str) -> String {
    let hex = hex.trim_start_matches('#');
    if !color_enabled() || hex.len() != 6 {
        return s.to_string();
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(200);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(200);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(200);
    format!("\x1b[1;38;2;{r};{g};{b}m{s}\x1b[0m")
}

const DIM: &str = "2";
const BOLD: &str = "1";
const GREEN: &str = "32";
const YELLOW: &str = "33";

/// A bold colored repo band: ` ▌ STORR   ~/path`. Mirrors the chooser header.
pub fn band(repo: &str, detail: &str, color_hex: &str) {
    let name = rgb(color_hex, &format!("▌ {}", repo.to_uppercase()));
    if detail.is_empty() {
        eprintln!("{name}");
    } else {
        eprintln!("{name}   {}", sgr(DIM, detail));
    }
}

/// An aligned key/value detail line: `  tool      claude · resuming`.
pub fn detail(label: &str, value: &str) {
    eprintln!("  {} {value}", sgr(DIM, &format!("{label:<9}")));
}

/// Begin a long-running step: a transient dim `  ⋯ message` with no
/// newline, flushed so the user sees liveness while a slow operation
/// (e.g. a `pre_create` sync hook) blocks. The next `ok`/`warn`
/// overwrites it. No-op off a terminal, so the result line stands alone
/// in captured logs.
pub fn step_start(msg: &str) {
    if is_tty() {
        use std::io::Write;
        eprint!("  {} {msg}", sgr(DIM, "⋯"));
        let _ = std::io::stderr().flush();
    }
}

/// A success status line: `  ✓ message`. Overwrites a preceding
/// `step_start` line on a terminal.
pub fn ok(msg: &str) {
    eprintln!("{}  {} {msg}", line_reset(), sgr(GREEN, "✓"));
}

/// A warning status line: `  ! message`. Overwrites a preceding
/// `step_start` line on a terminal.
pub fn warn(msg: &str) {
    eprintln!("{}  {} {}", line_reset(), sgr(YELLOW, "!"), sgr(YELLOW, msg));
}

/// A bottom action line: `→ launching…` (bold).
pub fn action(msg: &str) {
    eprintln!("{}", sgr(BOLD, &format!("→ {msg}")));
}

/// Dim aside text, indented.
pub fn note(msg: &str) {
    eprintln!("  {}", sgr(DIM, msg));
}

/// Abbreviate a leading `$HOME` to `~` for compact path display.
pub fn abbreviate_home(p: &str) -> String {
    if let Some(home) = std::env::var_os("HOME").and_then(|h| h.into_string().ok())
        && let Some(rest) = p.strip_prefix(&home)
    {
        return format!("~{rest}");
    }
    p.to_string()
}
