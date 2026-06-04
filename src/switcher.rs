use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use std::collections::{HashMap, HashSet};
use std::io;

use crate::claude_status::{self, SessionHealth};
use crate::config::Config;
use crate::primitives;
use crate::tmux::Tmux;

/// What a selectable row represents. The chooser merges three planes into one
/// list: live tmux sessions, dormant on-disk campaigns (visible so hygiene is
/// easy -- the no-review bloat fix), and a per-repo "create" affordance.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// The `muxr` control-plane shell. Pinned to the top, switch-only.
    Control,
    /// A live tmux campaign session. Switchable, killable, renamable.
    Running,
    /// An on-disk campaign with no live session. Selecting it launches it.
    Dormant,
    /// The "+ new campaign" row for a configured repo. Selecting it prompts
    /// for a slug and launches a fresh campaign.
    NewStub,
    /// A visual group separator -- not selectable.
    Separator,
}

struct Entry {
    /// Group key: the repo (or the raw first name segment for unknown/remote
    /// sessions). Empty for the control row and separators.
    repo: String,
    /// The campaign slug (the launch target / display label). Empty for stubs.
    campaign: String,
    /// The full tmux session name `<repo>/<campaign>` for running sessions,
    /// or the would-be name for dormant ones. Empty for stubs and separators.
    name: String,
    color: Color,
    activity: u64,
    health: Option<SessionHealth>,
    kind: Kind,
    /// True if this campaign was sharded from a hub in the same repo; rendered
    /// indented under its hub.
    is_shard: bool,
}

impl Entry {
    fn separator() -> Entry {
        Entry {
            repo: String::new(),
            campaign: String::new(),
            name: String::new(),
            color: Color::DarkGray,
            activity: 0,
            health: None,
            kind: Kind::Separator,
            is_shard: false,
        }
    }
    fn is_separator(&self) -> bool {
        self.kind == Kind::Separator
    }
    /// Selectable rows can be acted on with Enter.
    fn selectable(&self) -> bool {
        self.kind != Kind::Separator
    }
}

fn parse_hex_color(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return Color::Gray;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(128);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(128);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(128);
    Color::Rgb(r, g, b)
}

/// One campaign candidate while building a repo's group, before ordering.
struct Cand {
    info: primitives::CampaignInfo,
    running: bool,
    activity: u64,
    health: Option<SessionHealth>,
}

/// Build the merged chooser list: control plane (pinned), then one group per
/// repo containing its campaigns (running + dormant, shards indented under
/// their hub) and a "create" row, then any remaining live sessions whose repo
/// is not configured (remotes / unknown) so switching to them still works.
fn build_entries(config: &Config, tmux: &Tmux) -> Result<Vec<Entry>> {
    let sessions = tmux.list_sessions_detailed()?;

    // Index live sessions by name; pull the control plane aside.
    let mut running: HashMap<String, u64> = HashMap::new();
    let mut control: Option<Entry> = None;
    for s in &sessions {
        if s.name == "muxr" {
            control = Some(Entry {
                repo: String::new(),
                campaign: "control plane".to_string(),
                name: "muxr".to_string(),
                color: Color::Cyan,
                activity: s.activity,
                health: None,
                kind: Kind::Control,
                is_shard: false,
            });
        } else {
            running.insert(s.name.clone(), s.activity);
        }
    }

    let mut groups: HashMap<String, Vec<Entry>> = HashMap::new();
    let mut covered: HashSet<String> = HashSet::new();

    // 1. Configured repos: enumerate on-disk campaigns, mark which are live.
    for repo in config.repos.keys() {
        let color = parse_hex_color(config.color_for(repo));
        let Ok(dir) = config.resolve_dir(repo) else {
            continue;
        };
        let campaigns = primitives::list_campaigns(&dir).unwrap_or_default();

        // Resolve run-state + health for each campaign.
        let present: HashSet<String> = campaigns.iter().map(|c| c.name.clone()).collect();
        let mut cands: Vec<Cand> = campaigns
            .into_iter()
            .map(|info| {
                let name = format!("{repo}/{}", info.name);
                let running_now = running.contains_key(&name);
                let activity = running.get(&name).copied().unwrap_or(0);
                let health = if running_now {
                    claude_status::read_health(&name)
                } else {
                    None
                };
                covered.insert(name);
                Cand {
                    info,
                    running: running_now,
                    activity,
                    health,
                }
            })
            .collect();

        // Partition into top-level campaigns and shards-of-a-present-hub.
        let mut shards_by_hub: HashMap<String, Vec<Cand>> = HashMap::new();
        let mut tops: Vec<Cand> = Vec::new();
        // Drain cands, routing shards under their hub.
        for cand in cands.drain(..) {
            match &cand.info.sharded_from {
                Some(hub) if present.contains(hub) => {
                    shards_by_hub.entry(hub.clone()).or_default().push(cand);
                }
                _ => tops.push(cand),
            }
        }
        // Top-levels: live first, then by recent activity, then name.
        tops.sort_by(|a, b| {
            b.running
                .cmp(&a.running)
                .then(b.activity.cmp(&a.activity))
                .then(a.info.name.cmp(&b.info.name))
        });
        for v in shards_by_hub.values_mut() {
            v.sort_by(|a, b| a.info.name.cmp(&b.info.name));
        }

        let group = groups.entry(repo.clone()).or_default();
        for top in &tops {
            group.push(cand_entry(repo, color, top, false));
            if let Some(shards) = shards_by_hub.get(&top.info.name) {
                for sh in shards {
                    group.push(cand_entry(repo, color, sh, true));
                }
            }
        }
        // Per-repo create affordance.
        group.push(Entry {
            repo: repo.clone(),
            campaign: String::new(),
            name: String::new(),
            color,
            activity: 0,
            health: None,
            kind: Kind::NewStub,
            is_shard: false,
        });
    }

    // 2. Live sessions not covered by any on-disk campaign: archived-but-still
    //    -running campaigns, or sessions whose repo isn't configured (remotes,
    //    stale names). Keep them visible so switching still works.
    let mut leftover: Vec<(&String, u64)> = running
        .iter()
        .filter(|(name, _)| !covered.contains(*name))
        .map(|(n, a)| (n, *a))
        .collect();
    leftover.sort_by(|a, b| a.0.cmp(b.0));
    for (name, activity) in leftover {
        let (repo, campaign) = match name.split_once('/') {
            Some((r, c)) => (r.to_string(), c.to_string()),
            None => (name.clone(), String::new()),
        };
        let color = parse_hex_color(config.color_for(&repo));
        let health = claude_status::read_health(name);
        groups.entry(repo.clone()).or_default().push(Entry {
            repo,
            campaign,
            name: name.clone(),
            color,
            activity,
            health,
            kind: Kind::Running,
            is_shard: false,
        });
    }

    // Order groups: those with a live session first, then by recency, then name.
    let mut group_order: Vec<(String, bool, u64)> = groups
        .iter()
        .map(|(repo, entries)| {
            let has_live = entries.iter().any(|e| e.kind == Kind::Running);
            let max_activity = entries.iter().map(|e| e.activity).max().unwrap_or(0);
            (repo.clone(), has_live, max_activity)
        })
        .collect();
    group_order.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(b.2.cmp(&a.2))
            .then(a.0.cmp(&b.0))
    });

    let mut entries: Vec<Entry> = Vec::new();
    if let Some(c) = control {
        entries.push(c);
    }
    for (repo, _, _) in &group_order {
        if !entries.is_empty() {
            entries.push(Entry::separator());
        }
        if let Some(group_entries) = groups.remove(repo) {
            entries.extend(group_entries);
        }
    }

    Ok(entries)
}

fn cand_entry(repo: &str, color: Color, cand: &Cand, is_shard: bool) -> Entry {
    let name = format!("{repo}/{}", cand.info.name);
    Entry {
        repo: repo.to_string(),
        campaign: cand.info.name.clone(),
        name,
        color,
        activity: cand.activity,
        health: cand.health.clone(),
        kind: if cand.running {
            Kind::Running
        } else {
            Kind::Dormant
        },
        is_shard,
    }
}

fn entry_matches(e: &Entry, q: &str) -> bool {
    e.name.to_lowercase().contains(q)
        || e.repo.to_lowercase().contains(q)
        || e.campaign.to_lowercase().contains(q)
}

/// Compute the visible row indices.
///
/// When `show_all` is false (the default), only the control plane and LIVE
/// sessions are shown -- dormant on-disk campaigns and the "+ new campaign"
/// rows are hidden, so the list is what's actually running rather than a wall
/// of every campaign on disk. `a` toggles the full launcher view. A query
/// filters within the current mode. Group separators orphaned by hidden rows
/// are trimmed.
fn filter_entries(entries: &[Entry], query: &str, show_all: bool) -> Vec<usize> {
    let q = query.to_lowercase();
    let mut kept: Vec<usize> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let keep = match e.kind {
            Kind::Separator => true, // provisional; orphans trimmed below
            Kind::NewStub => show_all && query.is_empty(),
            Kind::Dormant => show_all && (query.is_empty() || entry_matches(e, &q)),
            Kind::Control | Kind::Running => query.is_empty() || entry_matches(e, &q),
        };
        if keep {
            kept.push(i);
        }
    }
    // Trim leading / duplicate / trailing separators left behind by hidden rows.
    let mut result: Vec<usize> = Vec::with_capacity(kept.len());
    for idx in kept {
        if entries[idx].is_separator() {
            if result
                .last()
                .map(|&l| !entries[l].is_separator())
                .unwrap_or(false)
            {
                result.push(idx);
            }
        } else {
            result.push(idx);
        }
    }
    while result
        .last()
        .map(|&l| entries[l].is_separator())
        .unwrap_or(false)
    {
        result.pop();
    }
    result
}

/// Format a unix timestamp as relative time (e.g., "2m", "1h", "3d").
fn format_age(activity: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if activity == 0 || activity > now {
        return String::new();
    }

    let age = now - activity;
    if age < 60 {
        format!("{age}s")
    } else if age < 3600 {
        format!("{}m", age / 60)
    } else if age < 86400 {
        format!("{}h", age / 3600)
    } else {
        format!("{}d", age / 86400)
    }
}

/// Action the chooser returns to main.
pub enum Action {
    /// Attach to an existing tmux session by name.
    Switch(String),
    /// Launch a campaign (existing dormant one, or a freshly named new one):
    /// (repo, campaign).
    Open(String, String),
    Kill(String),
    Rename(String, String),
    None,
}

/// Run the interactive chooser.
pub fn run(tmux: &Tmux) -> Result<Action> {
    let config = Config::load()?;
    let entries = build_entries(&config, tmux)?;

    if entries.is_empty() {
        anyhow::bail!("No sessions or campaigns to choose from");
    }

    let current = tmux.display_message("#{session_name}").unwrap_or_default();

    terminal::enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen).context("Failed to enter alt screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut table_state = TableState::default();
    let mut query = String::new();
    let mut filtering = false;
    // Default to active sessions only; `a` reveals dormant campaigns + create.
    let mut show_all = false;
    let mut filtered = filter_entries(&entries, &query, show_all);
    let mut confirm_kill: Option<usize> = None;
    // Rename buffer for entries[idx].
    let mut renaming: Option<(usize, String)> = None;
    let mut rename_error: Option<String> = None;
    // Create-campaign buffer: (repo, slug-buffer).
    let mut creating: Option<(String, String)> = None;
    let mut input_error: Option<String> = None;

    select_nearest_real(&entries, &filtered, &mut table_state, 0);

    let result = loop {
        terminal.draw(|f| {
            let area = f.area();
            let chunks = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(area);

            draw_table(
                f,
                chunks[0],
                &entries,
                &filtered,
                &current,
                &mut table_state,
                confirm_kill,
                renaming.as_ref().map(|(i, _)| *i),
            );
            draw_footer(
                f,
                chunks[1],
                &query,
                filtering,
                confirm_kill.is_some(),
                renaming.as_ref().map(|(_, buf)| buf.as_str()),
                creating.as_ref().map(|(repo, buf)| (repo.as_str(), buf.as_str())),
                input_error.as_deref().or(rename_error.as_deref()),
                selected_kind(&entries, &filtered, &table_state),
                show_all,
            );
        })?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            // Create-campaign mode -- swallows keys until Enter/Esc.
            if let Some((repo, buf)) = creating.as_mut() {
                match key.code {
                    KeyCode::Esc => {
                        creating = None;
                        input_error = None;
                    }
                    KeyCode::Enter => {
                        let slug = buf.trim().to_string();
                        match primitives::validate_topic(&slug) {
                            Ok(()) => {
                                let repo = repo.clone();
                                terminal::disable_raw_mode()?;
                                crossterm::execute!(
                                    terminal.backend_mut(),
                                    LeaveAlternateScreen
                                )?;
                                return Ok(Action::Open(repo, slug));
                            }
                            Err(e) => {
                                // First line of the validation message is enough
                                // for the footer.
                                input_error = Some(
                                    e.to_string().lines().next().unwrap_or("invalid").to_string(),
                                );
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        buf.pop();
                        input_error = None;
                    }
                    KeyCode::Char(c) => {
                        buf.push(c);
                        input_error = None;
                    }
                    _ => {}
                }
                continue;
            }

            // Rename mode -- swallows keys until Enter/Esc.
            if let Some((idx, buf)) = renaming.as_mut() {
                match key.code {
                    KeyCode::Esc => {
                        renaming = None;
                        rename_error = None;
                    }
                    KeyCode::Enter => {
                        let old = entries[*idx].name.clone();
                        let new = buf.trim().to_string();
                        if new.is_empty() {
                            rename_error = Some("name cannot be empty".to_string());
                        } else if new == old {
                            renaming = None;
                            rename_error = None;
                        } else if entries.iter().any(|e| e.selectable() && e.name == new) {
                            rename_error = Some(format!("'{new}' already exists"));
                        } else {
                            terminal::disable_raw_mode()?;
                            crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                            return Ok(Action::Rename(old, new));
                        }
                    }
                    KeyCode::Backspace => {
                        buf.pop();
                        rename_error = None;
                    }
                    KeyCode::Char(c) => {
                        buf.push(c);
                        rename_error = None;
                    }
                    _ => {}
                }
                continue;
            }

            // Kill confirmation mode.
            if let Some(kill_idx) = confirm_kill {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Enter => {
                        let name = entries[kill_idx].name.clone();
                        terminal::disable_raw_mode()?;
                        crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                        return Ok(Action::Kill(name));
                    }
                    _ => {
                        confirm_kill = None;
                        continue;
                    }
                }
            }

            match key.code {
                KeyCode::Esc if filtering => {
                    query.clear();
                    filtering = false;
                    filtered = filter_entries(&entries, &query, show_all);
                    select_nearest_real(&entries, &filtered, &mut table_state, 0);
                }
                KeyCode::Esc | KeyCode::Char('q') if !filtering => {
                    break Action::None;
                }
                KeyCode::Enter => {
                    if let Some(selected) = table_state.selected()
                        && let Some(&idx) = filtered.get(selected)
                    {
                        let e = &entries[idx];
                        match e.kind {
                            Kind::Control | Kind::Running => {
                                break Action::Switch(e.name.clone());
                            }
                            Kind::Dormant => {
                                break Action::Open(e.repo.clone(), e.campaign.clone());
                            }
                            Kind::NewStub => {
                                creating = Some((e.repo.clone(), String::new()));
                                input_error = None;
                            }
                            Kind::Separator => {}
                        }
                    }
                }
                KeyCode::Char('d') if !filtering => {
                    if let Some(selected) = table_state.selected()
                        && let Some(&idx) = filtered.get(selected)
                        && entries[idx].kind == Kind::Running
                        && entries[idx].name != current
                    {
                        confirm_kill = Some(idx);
                    }
                }
                KeyCode::Char('r') if !filtering => {
                    if let Some(selected) = table_state.selected()
                        && let Some(&idx) = filtered.get(selected)
                        && entries[idx].kind == Kind::Running
                    {
                        let prefill = entries[idx].name.clone();
                        renaming = Some((idx, prefill));
                        rename_error = None;
                    }
                }
                KeyCode::Char('n') if !filtering => {
                    // Jump to creating in the currently selected group.
                    if let Some(selected) = table_state.selected()
                        && let Some(&idx) = filtered.get(selected)
                        && !entries[idx].repo.is_empty()
                        && config.repos.contains_key(&entries[idx].repo)
                    {
                        creating = Some((entries[idx].repo.clone(), String::new()));
                        input_error = None;
                    }
                }
                KeyCode::Char('a') if !filtering => {
                    // Toggle between active-only (default) and the full
                    // launcher view (dormant campaigns + create rows).
                    show_all = !show_all;
                    filtered = filter_entries(&entries, &query, show_all);
                    select_nearest_real(&entries, &filtered, &mut table_state, 0);
                }
                KeyCode::Up => move_selection(&entries, &filtered, &mut table_state, -1),
                KeyCode::Down => move_selection(&entries, &filtered, &mut table_state, 1),
                KeyCode::Char('k') if !filtering => {
                    move_selection(&entries, &filtered, &mut table_state, -1)
                }
                KeyCode::Char('j') if !filtering => {
                    move_selection(&entries, &filtered, &mut table_state, 1)
                }
                KeyCode::Char('/') if !filtering => {
                    filtering = true;
                }
                KeyCode::Char(c) if filtering => {
                    query.push(c);
                    filtered = filter_entries(&entries, &query, show_all);
                    select_nearest_real(&entries, &filtered, &mut table_state, 0);
                }
                KeyCode::Backspace if filtering => {
                    query.pop();
                    if query.is_empty() {
                        filtering = false;
                    }
                    filtered = filter_entries(&entries, &query, show_all);
                    select_nearest_real(&entries, &filtered, &mut table_state, 0);
                }
                _ => {}
            }
        }
    };

    terminal::disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(result)
}

/// Kind of the currently selected row, for footer hinting.
fn selected_kind(entries: &[Entry], filtered: &[usize], state: &TableState) -> Option<Kind> {
    let sel = state.selected()?;
    let idx = *filtered.get(sel)?;
    Some(entries[idx].kind)
}

/// Move selection by delta, skipping separator rows.
fn move_selection(entries: &[Entry], filtered: &[usize], state: &mut TableState, delta: i32) {
    if filtered.is_empty() {
        return;
    }
    let current = state.selected().unwrap_or(0) as i32;
    let len = filtered.len() as i32;
    let mut next = (current + delta).rem_euclid(len);

    for _ in 0..len {
        if let Some(&idx) = filtered.get(next as usize)
            && !entries[idx].is_separator()
        {
            break;
        }
        next = (next + delta).rem_euclid(len);
    }

    state.select(Some(next as usize));
}

/// Select the nearest non-separator entry at or after `start`.
fn select_nearest_real(
    entries: &[Entry],
    filtered: &[usize],
    state: &mut TableState,
    start: usize,
) {
    for i in start..filtered.len() {
        if let Some(&idx) = filtered.get(i)
            && !entries[idx].is_separator()
        {
            state.select(Some(i));
            return;
        }
    }
    state.select(Some(0));
}

/// Build a context bar as ratatui Spans (8 chars wide).
fn health_bar(pct: u32) -> Vec<Span<'static>> {
    let width = 8usize;
    let filled = (pct as usize * width / 100).min(width);
    let empty = width - filled;

    let bar_color = if pct >= 80 {
        Color::Red
    } else if pct >= 50 {
        Color::Yellow
    } else {
        Color::Green
    };

    let mut spans = Vec::with_capacity(2);
    if filled > 0 {
        spans.push(Span::styled(
            "\u{2588}".repeat(filled),
            Style::default().fg(bar_color),
        ));
    }
    if empty > 0 {
        spans.push(Span::styled(
            "\u{2592}".repeat(empty),
            Style::default().fg(Color::Rgb(60, 60, 65)),
        ));
    }
    spans
}

#[allow(clippy::too_many_arguments)]
fn draw_table(
    f: &mut ratatui::Frame,
    area: Rect,
    entries: &[Entry],
    filtered: &[usize],
    current: &str,
    state: &mut TableState,
    confirm_kill: Option<usize>,
    rename_idx: Option<usize>,
) {
    let dim = Style::default().fg(Color::DarkGray);

    let header = Row::new(vec![
        Cell::from("  Repo").style(dim),
        Cell::from("Campaign").style(dim),
        Cell::from("        ").style(dim),
        Cell::from("     ").style(dim),
        Cell::from("Cache").style(dim),
        Cell::from("  Cost").style(dim),
        Cell::from("  Age").style(dim),
    ])
    .height(1);

    let rows: Vec<Row> = filtered
        .iter()
        .map(|&idx| {
            let e = &entries[idx];

            if e.is_separator() {
                let sep = Style::default().fg(Color::Rgb(50, 50, 55));
                return Row::new(vec![
                    Cell::from(Span::styled("────────────────", sep)),
                    Cell::from(Span::styled("──────────────────", sep)),
                    Cell::from(Span::styled("────────", sep)),
                    Cell::from(Span::styled("─────", sep)),
                    Cell::from(Span::styled("─────────", sep)),
                    Cell::from(Span::styled("───────", sep)),
                    Cell::from(Span::styled("──────", sep)),
                ])
                .height(1);
            }

            if e.kind == Kind::NewStub {
                let style = Style::default().fg(Color::Rgb(120, 150, 120));
                return Row::new(vec![
                    Cell::from(Span::styled("  ＋", style)),
                    Cell::from(Span::styled("new campaign…", style)),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                    Cell::from(""),
                ])
                .height(1);
            }

            let is_current = e.name == current;
            let is_kill_target = confirm_kill == Some(idx);
            let is_rename_target = rename_idx == Some(idx);
            let is_dormant = e.kind == Kind::Dormant;

            let marker = if is_rename_target {
                "✎ "
            } else if is_current {
                "● "
            } else {
                "  "
            };

            let kill_style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
            let is_switchboard = e.campaign == "switchboard";
            // Repo cell style.
            let vs = if is_kill_target {
                kill_style
            } else if is_dormant {
                Style::default().fg(Color::Rgb(110, 110, 120))
            } else if is_switchboard {
                Style::default().fg(e.color).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(e.color)
            };
            // Campaign cell style.
            let cs = if is_kill_target {
                kill_style
            } else if is_dormant {
                Style::default().fg(Color::Rgb(130, 130, 140))
            } else if is_switchboard {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else if is_current {
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let info_style = if is_kill_target {
                kill_style
            } else {
                Style::default().fg(Color::Rgb(90, 90, 100))
            };

            let (bar_cell, pct_cell, cache_cell, cost_cell) = if is_kill_target {
                (
                    Cell::from(Span::styled("kill?   ", kill_style)),
                    Cell::from(Span::styled("y/n  ", kill_style)),
                    Cell::from(Span::styled("         ", kill_style)),
                    Cell::from(Span::styled("       ", kill_style)),
                )
            } else if let Some(ref h) = e.health {
                let bar_spans = health_bar(h.context_pct);
                let pct_text = if h.exceeds_200k {
                    format!("{:>3}% 1M", h.context_pct)
                } else {
                    format!("{:>3}%   ", h.context_pct)
                };
                let pct_color = if h.context_pct >= 80 {
                    Color::Red
                } else if h.context_pct >= 50 {
                    Color::Yellow
                } else {
                    Color::White
                };
                let cache_text = match h.cache_pct {
                    Some(c) => format!("  {:>3}%   ", c),
                    None => "   --    ".to_string(),
                };
                let cost_text = if h.cost_usd > 0.0 {
                    format!(" ${:.2}", h.cost_usd)
                } else {
                    " $0.00".to_string()
                };
                (
                    Cell::from(Line::from(bar_spans)),
                    Cell::from(Span::styled(pct_text, Style::default().fg(pct_color))),
                    Cell::from(Span::styled(cache_text, info_style)),
                    Cell::from(Span::styled(cost_text, info_style)),
                )
            } else if is_dormant {
                (
                    Cell::from(Span::styled("        ", info_style)),
                    Cell::from(Span::styled("       ", info_style)),
                    Cell::from(Span::styled("         ", info_style)),
                    Cell::from(Span::styled("  open", Style::default().fg(Color::Rgb(90, 110, 90)))),
                )
            } else {
                (
                    Cell::from(Span::styled("        ", info_style)),
                    Cell::from(Span::styled("       ", info_style)),
                    Cell::from(Span::styled("   --    ", info_style)),
                    Cell::from(Span::styled("  idle", info_style)),
                )
            };

            let age_text = format!("  {}", format_age(e.activity));

            // Shards render indented under their hub.
            let campaign_label = if e.is_shard {
                format!("└ {}", e.campaign)
            } else {
                e.campaign.clone()
            };

            Row::new(vec![
                Cell::from(Line::from(vec![
                    Span::styled(marker, vs),
                    Span::styled(e.repo.clone(), vs),
                ])),
                Cell::from(Span::styled(campaign_label, cs)),
                bar_cell,
                pct_cell,
                cache_cell,
                cost_cell,
                Cell::from(Span::styled(age_text, info_style)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(16),
        Constraint::Min(14),
        Constraint::Length(8),
        Constraint::Length(7),
        Constraint::Length(9),
        Constraint::Length(7),
        Constraint::Length(6),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" muxr ")
                .title_style(
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
        )
        .row_highlight_style(
            Style::default()
                .bg(Color::Rgb(58, 58, 68))
                .add_modifier(Modifier::BOLD),
        );

    f.render_stateful_widget(table, area, state);
}

#[allow(clippy::too_many_arguments)]
fn draw_footer(
    f: &mut ratatui::Frame,
    area: Rect,
    query: &str,
    filtering: bool,
    killing: bool,
    rename_buffer: Option<&str>,
    create_buffer: Option<(&str, &str)>,
    input_error: Option<&str>,
    sel_kind: Option<Kind>,
    show_all: bool,
) {
    let dim = Style::default().fg(Color::DarkGray);
    let text = if let Some((repo, buf)) = create_buffer {
        let mut spans = vec![
            Span::styled(format!("  new {repo}/"), Style::default().fg(Color::Green)),
            Span::styled(buf.to_string(), Style::default().fg(Color::White)),
            Span::styled("_", Style::default().fg(Color::Green)),
        ];
        if let Some(err) = input_error {
            spans.push(Span::styled(format!("  {err}"), Style::default().fg(Color::Red)));
        } else {
            spans.push(Span::styled("  enter", dim));
            spans.push(Span::styled(" create  ", dim));
            spans.push(Span::styled("esc", dim));
            spans.push(Span::styled(" cancel", dim));
        }
        Line::from(spans)
    } else if let Some(buf) = rename_buffer {
        let mut spans = vec![
            Span::styled("  rename > ", Style::default().fg(Color::Cyan)),
            Span::styled(buf.to_string(), Style::default().fg(Color::White)),
            Span::styled("_", Style::default().fg(Color::Cyan)),
        ];
        if let Some(err) = input_error {
            spans.push(Span::styled(format!("  {err}"), Style::default().fg(Color::Red)));
        } else {
            spans.push(Span::styled("  enter", dim));
            spans.push(Span::styled(" commit  ", dim));
            spans.push(Span::styled("esc", dim));
            spans.push(Span::styled(" cancel", dim));
        }
        Line::from(spans)
    } else if killing {
        Line::from(vec![
            Span::styled("  y", Style::default().fg(Color::Red)),
            Span::styled(" confirm kill  ", dim),
            Span::styled("any", dim),
            Span::styled(" cancel", dim),
        ])
    } else if filtering || !query.is_empty() {
        Line::from(vec![
            Span::styled("  /", Style::default().fg(Color::Yellow)),
            Span::styled(query, Style::default().fg(Color::White)),
            Span::styled("_", Style::default().fg(Color::Yellow)),
            Span::styled("  esc", dim),
            Span::styled(" clear", dim),
        ])
    } else {
        // Context-sensitive enter hint.
        let enter_hint = match sel_kind {
            Some(Kind::Dormant) => " open  ",
            Some(Kind::NewStub) => " create  ",
            _ => " switch  ",
        };
        // Mode tag + what `a` toggles to.
        let (mode_tag, mode_color, a_hint) = if show_all {
            ("[all] ", Color::Cyan, " active  ")
        } else {
            ("[active] ", Color::Green, " all  ")
        };
        Line::from(vec![
            Span::styled(format!("  {mode_tag}"), Style::default().fg(mode_color)),
            Span::styled("a", dim),
            Span::styled(a_hint, dim),
            Span::styled("/", dim),
            Span::styled("filter  ", dim),
            Span::styled("j/k", dim),
            Span::styled(" move  ", dim),
            Span::styled("enter", dim),
            Span::styled(enter_hint, dim),
            Span::styled("n", dim),
            Span::styled(" new  ", dim),
            Span::styled("r", dim),
            Span::styled(" rename  ", dim),
            Span::styled("d", dim),
            Span::styled(" kill  ", dim),
            Span::styled("q", dim),
            Span::styled(" quit", dim),
        ])
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_color_valid() {
        assert_eq!(parse_hex_color("#7aa2f7"), Color::Rgb(122, 162, 247));
        assert_eq!(parse_hex_color("#000000"), Color::Rgb(0, 0, 0));
        assert_eq!(parse_hex_color("#FFFFFF"), Color::Rgb(255, 255, 255));
    }

    #[test]
    fn parse_hex_color_without_hash() {
        assert_eq!(parse_hex_color("7aa2f7"), Color::Rgb(122, 162, 247));
    }

    #[test]
    fn parse_hex_color_invalid_falls_back() {
        assert_eq!(parse_hex_color("#FFF"), Color::Gray);
        assert_eq!(parse_hex_color(""), Color::Gray);
    }

    #[test]
    fn format_age_seconds() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(format_age(now - 30), "30s");
    }

    #[test]
    fn format_age_minutes() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(format_age(now - 120), "2m");
    }

    #[test]
    fn format_age_hours() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(format_age(now - 7200), "2h");
    }

    #[test]
    fn format_age_days() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(format_age(now - 172800), "2d");
    }

    #[test]
    fn format_age_zero_returns_empty() {
        assert_eq!(format_age(0), "");
    }

    fn make_entry(repo: &str, campaign: &str, kind: Kind) -> Entry {
        Entry {
            repo: repo.to_string(),
            campaign: campaign.to_string(),
            name: if campaign.is_empty() {
                String::new()
            } else {
                format!("{repo}/{campaign}")
            },
            color: Color::Gray,
            activity: 0,
            health: None,
            kind,
            is_shard: false,
        }
    }

    #[test]
    fn filter_active_only_hides_dormant_and_trims_orphan_separator() {
        let entries = vec![
            make_entry("work", "api", Kind::Running),
            Entry::separator(),
            make_entry("personal", "blog", Kind::Dormant),
        ];
        // Default (active-only): dormant hidden, and the now-trailing
        // separator is trimmed.
        assert_eq!(filter_entries(&entries, "", false), vec![0]);
        // show_all reveals everything.
        assert_eq!(filter_entries(&entries, "", true), vec![0, 1, 2]);
    }

    #[test]
    fn filter_entries_matches_campaign() {
        let entries = vec![
            make_entry("work", "api", Kind::Running),
            make_entry("personal", "blog", Kind::Dormant),
        ];
        assert_eq!(filter_entries(&entries, "api", true), vec![0]);
    }

    #[test]
    fn filter_entries_matches_repo() {
        let entries = vec![
            make_entry("work", "api", Kind::Running),
            make_entry("work", "auth", Kind::Dormant),
            make_entry("personal", "blog", Kind::Running),
        ];
        // In all-mode, repo "work" matches the running + dormant entries.
        assert_eq!(filter_entries(&entries, "work", true), vec![0, 1]);
        // In active-only mode, only the running one.
        assert_eq!(filter_entries(&entries, "work", false), vec![0]);
    }

    #[test]
    fn filter_entries_skips_separators_and_stubs() {
        let entries = vec![
            make_entry("work", "api", Kind::Running),
            Entry::separator(),
            make_entry("work", "", Kind::NewStub),
            make_entry("personal", "blog", Kind::Dormant),
        ];
        // query that would textually match the stub's repo still drops it.
        assert_eq!(filter_entries(&entries, "blog", true), vec![3]);
        assert_eq!(filter_entries(&entries, "work", true), vec![0]);
    }

    #[test]
    fn filter_entries_case_insensitive() {
        let entries = vec![make_entry("Work", "API", Kind::Running)];
        assert_eq!(filter_entries(&entries, "api", false), vec![0]);
    }

    #[test]
    fn move_selection_skips_separators() {
        let entries = vec![
            make_entry("work", "api", Kind::Running),
            Entry::separator(),
            make_entry("personal", "blog", Kind::Dormant),
        ];
        let filtered = vec![0, 1, 2];
        let mut state = TableState::default();
        state.select(Some(0));
        move_selection(&entries, &filtered, &mut state, 1);
        // Should skip the separator at index 1 and land on 2.
        assert_eq!(state.selected(), Some(2));
    }
}
