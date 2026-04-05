use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::PathBuf;

use crate::config::Config;
use crate::tmux::Tmux;

/// Cached health data for the switcher to read.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct SessionHealth {
    pub context_pct: u32,
    pub cache_pct: Option<u32>,
    pub cost_usd: f64,
    pub exceeds_200k: bool,
}

/// Health cache directory: ~/.config/muxr/health/
fn health_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(home.join(".config").join("muxr").join("health"))
}

/// Convert a session name to a safe filename (replace / with --).
fn health_filename(session_name: &str) -> String {
    format!("{}.json", session_name.replace('/', "--"))
}

/// Write health cache for a session.
fn write_health(session_name: &str, health: &SessionHealth) {
    let Some(dir) = health_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(health_filename(session_name));
    if let Ok(json) = serde_json::to_string(health) {
        let _ = std::fs::write(path, json);
    }
}

/// Read health cache for a session. Returns None if no cache exists.
pub fn read_health(session_name: &str) -> Option<SessionHealth> {
    let dir = health_dir()?;
    let path = dir.join(health_filename(session_name));
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

// -- ANSI colors (muted palette) --

const RST: &str = "\x1b[0m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const WHITE: &str = "\x1b[37m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const CYAN: &str = "\x1b[36m";
const MAGENTA: &str = "\x1b[35m";

// Nerd Font GitLab tanuki (U+F296)
const GL_ICON: &str = "\u{f296}";

// -- JSON schema from Claude Code stdin --

#[derive(Deserialize, Default)]
struct StatusInput {
    #[serde(default)]
    model: ModelInfo,
    #[serde(default)]
    context_window: ContextWindow,
    #[serde(default)]
    cost: CostInfo,
    #[serde(default)]
    rate_limits: Option<RateLimits>,
    #[serde(default)]
    workspace: Workspace,
    #[serde(default)]
    worktree: Option<WorktreeInfo>,
    #[serde(default)]
    agent: Option<AgentInfo>,
    #[serde(default)]
    exceeds_200k_tokens: bool,
}

#[derive(Deserialize, Default)]
struct ModelInfo {
    #[serde(default)]
    #[allow(dead_code)]
    id: String,
    #[serde(default)]
    display_name: String,
}

#[derive(Deserialize, Default)]
struct ContextWindow {
    #[serde(default)]
    used_percentage: Option<f64>,
    #[serde(default)]
    #[allow(dead_code)]
    context_window_size: u64,
    #[serde(default)]
    current_usage: Option<CurrentUsage>,
}

#[derive(Deserialize, Default)]
struct CurrentUsage {
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[derive(Deserialize, Default)]
struct CostInfo {
    #[serde(default)]
    total_cost_usd: f64,
    #[serde(default)]
    total_duration_ms: u64,
    #[serde(default)]
    total_lines_added: u64,
    #[serde(default)]
    total_lines_removed: u64,
}

#[derive(Deserialize, Default)]
struct RateLimits {
    #[serde(default)]
    five_hour: Option<RateWindow>,
    #[serde(default)]
    seven_day: Option<RateWindow>,
}

#[derive(Deserialize, Default)]
struct RateWindow {
    #[serde(default)]
    used_percentage: f64,
}

#[derive(Deserialize, Default)]
struct Workspace {
    #[serde(default)]
    project_dir: String,
    #[serde(default)]
    current_dir: String,
}

#[derive(Deserialize, Default)]
struct WorktreeInfo {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize, Default)]
struct AgentInfo {
    #[serde(default)]
    name: String,
}

// -- Git info via libgit2 --

struct GitInfo {
    branch: String,
    dirty: bool,
}

fn git_info(project_dir: &str) -> Option<GitInfo> {
    let repo = git2::Repository::open(project_dir).ok()?;
    let head = repo.head().ok()?;
    let branch = head
        .shorthand()
        .unwrap_or("HEAD")
        .to_string();

    // Check for any changes (staged or unstaged)
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(false);

    let dirty = repo
        .statuses(Some(&mut opts))
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    Some(GitInfo { branch, dirty })
}

// -- Bar rendering --

fn context_bar(used_pct: u32, width: usize) -> String {
    let filled = (used_pct as usize * width / 100).min(width);
    let empty = width - filled;

    let bar_color = if used_pct >= 80 {
        RED
    } else if used_pct >= 50 {
        YELLOW
    } else {
        GREEN
    };

    let mut bar = String::with_capacity(width + 40);
    bar.push_str(bar_color);
    for _ in 0..filled {
        bar.push('\u{2588}'); // █
    }
    bar.push_str(DIM);
    for _ in 0..empty {
        bar.push('\u{2592}'); // ▒
    }
    bar.push_str(RST);
    bar
}

// -- Duration formatting --

fn format_duration(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        let h = s / 3600;
        let m = (s % 3600) / 60;
        if m == 0 { format!("{h}h") } else { format!("{h}h{m}m") }
    } else if s >= 60 {
        let m = s / 60;
        let sec = s % 60;
        if sec == 0 { format!("{m}m") } else { format!("{m}m{sec}s") }
    } else {
        format!("{s}s")
    }
}

// -- Cache ratio --

fn cache_ratio(usage: &Option<CurrentUsage>) -> Option<u32> {
    let u = usage.as_ref()?;
    let total = u.cache_creation_input_tokens + u.cache_read_input_tokens;
    if total == 0 {
        return None;
    }
    Some((u.cache_read_input_tokens * 100 / total) as u32)
}

/// Run the claude-status command. Reads JSON from stdin, outputs formatted status.
pub fn run(tmux: &Tmux) -> Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("Failed to read stdin")?;

    let status: StatusInput = serde_json::from_str(&input).unwrap_or_default();

    // Get muxr session name from tmux
    let session_name = tmux
        .display_message("#{session_name}")
        .unwrap_or_default();

    // Resolve vertical color from muxr config
    let vertical = session_name.split('/').next().unwrap_or(&session_name);
    let config = Config::load().ok();
    let hex_color = config
        .as_ref()
        .map(|c| c.color_for(vertical).to_string())
        .unwrap_or_else(|| "#8a7f83".to_string());
    let ansi_color = hex_to_ansi(&hex_color);

    // -- Line 1: session identity + git --
    let mut line1 = String::new();

    // Colored dot + session name
    line1.push_str(&ansi_color);
    line1.push_str(GL_ICON);
    line1.push(' ');
    line1.push_str(BOLD);
    line1.push_str(WHITE);
    line1.push_str(&session_name);
    line1.push_str(RST);

    // Git info
    let project_dir = if status.workspace.project_dir.is_empty() {
        &status.workspace.current_dir
    } else {
        &status.workspace.project_dir
    };

    if !project_dir.is_empty() {
        if let Some(git) = git_info(project_dir) {
            line1.push_str("  ");
            line1.push_str(CYAN);
            line1.push_str(&git.branch);
            line1.push_str(RST);
            if git.dirty {
                line1.push(' ');
                line1.push_str(YELLOW);
                line1.push('*');
                line1.push_str(RST);
            }
        }
    }

    // Lines changed
    if status.cost.total_lines_added > 0 || status.cost.total_lines_removed > 0 {
        line1.push_str("  ");
        line1.push_str(GREEN);
        line1.push_str(&format!("+{}", status.cost.total_lines_added));
        line1.push_str(RST);
        line1.push(' ');
        line1.push_str(RED);
        line1.push_str(&format!("-{}", status.cost.total_lines_removed));
        line1.push_str(RST);
    }

    // Worktree badge
    if let Some(ref wt) = status.worktree {
        if !wt.name.is_empty() {
            line1.push_str("  ");
            line1.push_str(MAGENTA);
            line1.push_str("wt:");
            line1.push_str(&wt.name);
            line1.push_str(RST);
        }
    }

    // Agent badge
    if let Some(ref agent) = status.agent {
        if !agent.name.is_empty() {
            line1.push_str("  ");
            line1.push_str(DIM);
            line1.push_str("agent:");
            line1.push_str(&agent.name);
            line1.push_str(RST);
        }
    }

    // -- Line 2: model + context bar + cache + cost + duration + rate limits --
    let mut line2 = String::new();

    // Model name
    line2.push_str(BOLD);
    line2.push_str(WHITE);
    line2.push_str(&status.model.display_name);
    line2.push_str(RST);

    // 1M badge -- only shown when actually past 200k
    if status.exceeds_200k_tokens {
        line2.push_str(" 1M");
    }

    // Context bar
    let used_pct = status.context_window.used_percentage.unwrap_or(0.0) as u32;
    line2.push_str("  ");
    line2.push_str(&context_bar(used_pct, 20));
    line2.push_str(&format!("  {used_pct:>3}%"));

    // Cache ratio
    if let Some(ratio) = cache_ratio(&status.context_window.current_usage) {
        line2.push_str("  ");
        line2.push_str(DIM);
        line2.push_str(&format!("cache {ratio}%"));
        line2.push_str(RST);
    }

    // Cost
    line2.push_str("  ");
    line2.push_str(DIM);
    if status.cost.total_cost_usd > 0.0 {
        line2.push_str(&format!("${:.2}", status.cost.total_cost_usd));
    } else {
        line2.push_str("$0.00");
    }
    line2.push_str(RST);

    // Duration
    line2.push_str("  ");
    line2.push_str(DIM);
    line2.push_str(&format_duration(status.cost.total_duration_ms));
    line2.push_str(RST);

    // Rate limits -- only shown above 50%
    if let Some(ref rl) = status.rate_limits {
        if let Some(ref five) = rl.five_hour {
            let pct = five.used_percentage as u32;
            if pct > 50 {
                let color = if pct >= 80 { RED } else { YELLOW };
                line2.push_str("  ");
                line2.push_str(color);
                line2.push_str(&format!("5h:{pct}%"));
                line2.push_str(RST);
            }
        }
        if let Some(ref seven) = rl.seven_day {
            let pct = seven.used_percentage as u32;
            if pct > 50 {
                let color = if pct >= 80 { RED } else { YELLOW };
                line2.push_str("  ");
                line2.push_str(color);
                line2.push_str(&format!("7d:{pct}%"));
                line2.push_str(RST);
            }
        }
    }

    // -- Cache health for switcher --
    if !session_name.is_empty() {
        write_health(
            &session_name,
            &SessionHealth {
                context_pct: used_pct,
                cache_pct: cache_ratio(&status.context_window.current_usage),
                cost_usd: status.cost.total_cost_usd,
                exceeds_200k: status.exceeds_200k_tokens,
            },
        );
    }

    // -- Output --
    println!("{line1}");
    print!("{line2}");

    Ok(())
}

/// Convert a hex color (#FC6D26) to ANSI 24-bit escape sequence.
fn hex_to_ansi(hex: &str) -> String {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return format!("\x1b[37m"); // fallback white
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(255);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255);
    format!("\x1b[38;2;{r};{g};{b}m")
}
