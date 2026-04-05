use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Terminal;
use std::io;

use crate::claude_status::{self, SessionHealth};
use crate::config::Config;
use crate::tmux::Tmux;

struct Entry {
    vertical: String,
    context: String,
    name: String,
    color: Color,
    activity: u64,
    health: Option<SessionHealth>,
    is_separator: bool, // true = visual group separator, not selectable
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

/// Build entries sorted by activity (most recent first), with muxr control plane pinned to top.
/// Inserts separator rows between vertical groups.
fn build_entries(config: &Config, tmux: &Tmux) -> Result<Vec<Entry>> {
    let sessions = tmux.list_sessions_detailed()?;

    // Build raw entries
    let raw: Vec<Entry> = sessions
        .into_iter()
        .map(|s| {
            let (vertical, context) = match s.name.split_once('/') {
                Some((v, c)) => (v.to_string(), c.to_string()),
                None => (s.name.clone(), String::new()),
            };
            let color = parse_hex_color(config.color_for(&vertical));
            let health = claude_status::read_health(&s.name);

            Entry {
                vertical,
                context,
                name: s.name,
                color,
                activity: s.activity,
                health,
                is_separator: false,
            }
        })
        .collect();

    // Group by vertical, sort groups by most recent activity,
    // sort sessions within each group by most recent activity.
    // muxr control plane is always pinned to top (ungrouped).
    let mut muxr_entry: Option<Entry> = None;
    let mut groups: std::collections::HashMap<String, Vec<Entry>> =
        std::collections::HashMap::new();

    for entry in raw {
        if entry.name == "muxr" {
            muxr_entry = Some(entry);
        } else {
            groups
                .entry(entry.vertical.clone())
                .or_default()
                .push(entry);
        }
    }

    // Sort sessions within each group by most recent activity
    for group in groups.values_mut() {
        group.sort_by(|a, b| b.activity.cmp(&a.activity));
    }

    // Sort groups by their most recent session's activity
    let mut group_order: Vec<(String, u64)> = groups
        .iter()
        .map(|(name, entries)| {
            let max_activity = entries.iter().map(|e| e.activity).max().unwrap_or(0);
            (name.clone(), max_activity)
        })
        .collect();
    group_order.sort_by(|a, b| b.1.cmp(&a.1));

    // Build final list: muxr first, then groups with separators
    let mut entries: Vec<Entry> = Vec::with_capacity(groups.values().map(|g| g.len()).sum::<usize>() + group_order.len() + 1);

    if let Some(muxr) = muxr_entry {
        entries.push(muxr);
    }

    for (group_name, _) in &group_order {
        if !entries.is_empty() {
            entries.push(Entry {
                vertical: String::new(),
                context: String::new(),
                name: String::new(),
                color: Color::DarkGray,
                activity: 0,
                health: None,
                is_separator: true,
            });
        }
        if let Some(group_entries) = groups.remove(group_name) {
            entries.extend(group_entries);
        }
    }

    Ok(entries)
}

/// Filter entries, preserving separators between matched groups.
fn filter_entries(entries: &[Entry], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..entries.len()).collect();
    }
    let q = query.to_lowercase();

    // First pass: find matching real entries
    let matched: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            !e.is_separator
                && (e.name.to_lowercase().contains(&q)
                    || e.vertical.to_lowercase().contains(&q)
                    || e.context.to_lowercase().contains(&q))
        })
        .map(|(i, _)| i)
        .collect();

    // When filtering, skip separators -- just show matched entries flat
    matched
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

/// Action the switcher returns to main.
pub enum Action {
    Switch(String),
    Kill(String),
    None,
}

/// Run the interactive switcher.
pub fn run(tmux: &Tmux) -> Result<Action> {
    let config = Config::load()?;
    let entries = build_entries(&config, tmux)?;

    if entries.is_empty() {
        anyhow::bail!("No active tmux sessions");
    }

    let current = tmux
        .display_message("#{session_name}")
        .unwrap_or_default();

    terminal::enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen).context("Failed to enter alt screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut table_state = TableState::default();
    let mut query = String::new();
    let mut filtering = false;
    let mut filtered = filter_entries(&entries, &query);
    let mut confirm_kill: Option<usize> = None; // index into entries if confirming

    // Select first non-separator
    select_nearest_real(&entries, &filtered, &mut table_state, 0);

    let result = loop {
        terminal.draw(|f| {
            let area = f.area();
            let chunks =
                Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(area);

            draw_table(
                f,
                chunks[0],
                &entries,
                &filtered,
                &current,
                &mut table_state,
                confirm_kill,
            );
            draw_footer(f, chunks[1], &query, filtering, confirm_kill.is_some());
        })?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            // Kill confirmation mode
            if let Some(kill_idx) = confirm_kill {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Enter => {
                        let name = entries[kill_idx].name.clone();
                        // Restore terminal before killing
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
                    filtered = filter_entries(&entries, &query);
                    select_nearest_real(&entries, &filtered, &mut table_state, 0);
                }
                KeyCode::Esc | KeyCode::Char('q') if !filtering => {
                    break Action::None;
                }
                KeyCode::Enter => {
                    if let Some(selected) = table_state.selected()
                        && let Some(&idx) = filtered.get(selected)
                        && !entries[idx].is_separator
                    {
                        break Action::Switch(entries[idx].name.clone());
                    }
                }
                KeyCode::Char('d') if !filtering => {
                    if let Some(selected) = table_state.selected()
                        && let Some(&idx) = filtered.get(selected)
                        && !entries[idx].is_separator
                        && entries[idx].name != current
                        && entries[idx].name != "muxr"
                    {
                        confirm_kill = Some(idx);
                    }
                }
                KeyCode::Up => {
                    move_selection(&entries, &filtered, &mut table_state, -1);
                }
                KeyCode::Down => {
                    move_selection(&entries, &filtered, &mut table_state, 1);
                }
                KeyCode::Char('k') if !filtering => {
                    move_selection(&entries, &filtered, &mut table_state, -1);
                }
                KeyCode::Char('j') if !filtering => {
                    move_selection(&entries, &filtered, &mut table_state, 1);
                }
                KeyCode::Char('/') if !filtering => {
                    filtering = true;
                }
                KeyCode::Char(c) if filtering => {
                    query.push(c);
                    filtered = filter_entries(&entries, &query);
                    select_nearest_real(&entries, &filtered, &mut table_state, 0);
                }
                KeyCode::Backspace if filtering => {
                    query.pop();
                    if query.is_empty() {
                        filtering = false;
                    }
                    filtered = filter_entries(&entries, &query);
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

/// Move selection by delta, skipping separator rows.
fn move_selection(
    entries: &[Entry],
    filtered: &[usize],
    state: &mut TableState,
    delta: i32,
) {
    if filtered.is_empty() {
        return;
    }
    let current = state.selected().unwrap_or(0) as i32;
    let len = filtered.len() as i32;
    let mut next = (current + delta).rem_euclid(len);

    // Skip separators
    for _ in 0..len {
        if let Some(&idx) = filtered.get(next as usize)
            && !entries[idx].is_separator
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
            && !entries[idx].is_separator
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

fn draw_table(
    f: &mut ratatui::Frame,
    area: Rect,
    entries: &[Entry],
    filtered: &[usize],
    current: &str,
    state: &mut TableState,
    confirm_kill: Option<usize>,
) {
    let dim = Style::default().fg(Color::DarkGray);

    let header = Row::new(vec![
        Cell::from("  Session").style(dim),
        Cell::from("Context").style(dim),
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

            if e.is_separator {
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

            let is_current = e.name == current;
            let is_kill_target = confirm_kill == Some(idx);

            let marker = if is_current { "● " } else { "  " };

            let kill_style = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
            let vs = if is_kill_target {
                kill_style
            } else {
                Style::default().fg(e.color)
            };
            let cs = if is_kill_target {
                kill_style
            } else if is_current {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let info_style = if is_kill_target {
                kill_style
            } else {
                Style::default().fg(Color::Rgb(90, 90, 100))
            };

            // Health columns
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
            } else {
                (
                    Cell::from(Span::styled("        ", info_style)),
                    Cell::from(Span::styled("       ", info_style)),
                    Cell::from(Span::styled("   --    ", info_style)),
                    Cell::from(Span::styled("  idle", info_style)),
                )
            };

            let age_text = format!("  {}", format_age(e.activity));

            Row::new(vec![
                Cell::from(Line::from(vec![
                    Span::styled(marker, vs),
                    Span::styled(e.vertical.clone(), vs),
                ])),
                Cell::from(Span::styled(e.context.clone(), cs)),
                bar_cell,
                pct_cell,
                cache_cell,
                cost_cell,
                Cell::from(Span::styled(age_text, info_style)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(16),  // session (marker + vertical)
        Constraint::Min(12),    // context
        Constraint::Length(8),  // bar
        Constraint::Length(7),  // pct (+ 1M badge)
        Constraint::Length(9),  // cache
        Constraint::Length(7),  // cost
        Constraint::Length(6),  // age
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

fn draw_footer(f: &mut ratatui::Frame, area: Rect, query: &str, filtering: bool, killing: bool) {
    let dim = Style::default().fg(Color::DarkGray);
    let text = if killing {
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
        Line::from(vec![
            Span::styled("  /", dim),
            Span::styled("filter  ", dim),
            Span::styled("j/k", dim),
            Span::styled(" move  ", dim),
            Span::styled("enter", dim),
            Span::styled(" select  ", dim),
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
