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

use crate::config::Config;
use crate::tmux;

/// A session entry for the switcher UI.
struct Entry {
    vertical: String,
    context: String,
    session_type: &'static str, // "local" or "remote"
    name: String,               // full session name for tmux attach
    color: Color,
}

/// Parse a hex color string (#RRGGBB) into a ratatui Color.
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

/// Build the entry list from live tmux sessions + config.
fn build_entries(config: &Config) -> Result<Vec<Entry>> {
    let sessions = tmux::list_sessions()?;
    let mut entries = Vec::new();

    for (name, _path) in sessions {
        let (vertical, context) = match name.split_once('/') {
            Some((v, c)) => (v.to_string(), c.to_string()),
            None => (name.clone(), String::new()),
        };

        let is_remote = config.is_remote(&vertical);
        let color_hex = config.color_for(&vertical);
        let color = parse_hex_color(color_hex);

        entries.push(Entry {
            vertical,
            context,
            session_type: if is_remote { "remote" } else { "local" },
            name,
            color,
        });
    }

    Ok(entries)
}

/// Filter entries by a query string (fuzzy: matches anywhere in name).
fn filter_entries(entries: &[Entry], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..entries.len()).collect();
    }
    let q = query.to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.name.to_lowercase().contains(&q)
                || e.vertical.to_lowercase().contains(&q)
                || e.context.to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Run the interactive switcher. Returns the session name to switch to, or None.
pub fn run() -> Result<Option<String>> {
    let config = Config::load()?;
    let entries = build_entries(&config)?;

    if entries.is_empty() {
        anyhow::bail!("No active tmux sessions");
    }

    // Get current session so we can highlight it
    let current = std::process::Command::new("tmux")
        .args(["display-message", "-p", "#{session_name}"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
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
    table_state.select(Some(0));

    let result = loop {
        terminal.draw(|f| {
            let area = f.area();
            let chunks = Layout::vertical([
                Constraint::Min(3),
                Constraint::Length(3),
            ])
            .split(area);

            draw_table(f, chunks[0], &entries, &filtered, &current, &mut table_state);
            draw_filter(f, chunks[1], &query, filtering);
        })?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            // Navigation keys always work regardless of mode
            match key.code {
                KeyCode::Esc if filtering => {
                    query.clear();
                    filtering = false;
                    filtered = filter_entries(&entries, &query);
                    table_state.select(Some(0));
                    continue;
                }
                KeyCode::Esc | KeyCode::Char('q') if !filtering => {
                    break None;
                }
                KeyCode::Enter => {
                    if let Some(selected) = table_state.selected()
                        && let Some(&idx) = filtered.get(selected)
                    {
                        break Some(entries[idx].name.clone());
                    }
                    break None;
                }
                KeyCode::Up => {
                    let i = table_state.selected().unwrap_or(0);
                    let next = if i == 0 {
                        filtered.len().saturating_sub(1)
                    } else {
                        i - 1
                    };
                    table_state.select(Some(next));
                }
                KeyCode::Down => {
                    let i = table_state.selected().unwrap_or(0);
                    let next = if i >= filtered.len().saturating_sub(1) {
                        0
                    } else {
                        i + 1
                    };
                    table_state.select(Some(next));
                }
                KeyCode::Char('k') if !filtering => {
                    let i = table_state.selected().unwrap_or(0);
                    let next = if i == 0 {
                        filtered.len().saturating_sub(1)
                    } else {
                        i - 1
                    };
                    table_state.select(Some(next));
                }
                KeyCode::Char('j') if !filtering => {
                    let i = table_state.selected().unwrap_or(0);
                    let next = if i >= filtered.len().saturating_sub(1) {
                        0
                    } else {
                        i + 1
                    };
                    table_state.select(Some(next));
                }
                KeyCode::Char('/') if !filtering => {
                    filtering = true;
                }
                KeyCode::Char(c) if filtering => {
                    query.push(c);
                    filtered = filter_entries(&entries, &query);
                    table_state.select(Some(0));
                }
                KeyCode::Backspace if filtering => {
                    query.pop();
                    if query.is_empty() {
                        filtering = false;
                    }
                    filtered = filter_entries(&entries, &query);
                    table_state.select(Some(0));
                }
                _ => {}
            }
        }
    };

    // Restore terminal
    terminal::disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(result)
}

fn draw_table(
    f: &mut ratatui::Frame,
    area: Rect,
    entries: &[Entry],
    filtered: &[usize],
    current: &str,
    state: &mut TableState,
) {
    let header = Row::new(vec![
        Cell::from("  Vertical").style(Style::default().fg(Color::DarkGray)),
        Cell::from("Context").style(Style::default().fg(Color::DarkGray)),
        Cell::from("Type").style(Style::default().fg(Color::DarkGray)),
    ])
    .height(1);

    let rows: Vec<Row> = filtered
        .iter()
        .map(|&idx| {
            let e = &entries[idx];
            let is_current = e.name == current;

            let marker = if is_current { "● " } else { "  " };
            let vertical_style = Style::default().fg(e.color);
            let context_style = if is_current {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let type_style = Style::default().fg(if e.session_type == "remote" {
                Color::Cyan
            } else {
                Color::DarkGray
            });

            Row::new(vec![
                Cell::from(Line::from(vec![
                    Span::styled(marker, vertical_style),
                    Span::styled(e.vertical.clone(), vertical_style),
                ])),
                Cell::from(Span::styled(e.context.clone(), context_style)),
                Cell::from(Span::styled(e.session_type, type_style)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(16),
        Constraint::Min(20),
        Constraint::Length(8),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" muxr ")
                .title_style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        )
        .row_highlight_style(
            Style::default()
                .bg(Color::Rgb(58, 58, 68))
                .add_modifier(Modifier::BOLD),
        );

    f.render_stateful_widget(table, area, state);
}

fn draw_filter(f: &mut ratatui::Frame, area: Rect, query: &str, filtering: bool) {
    let dim = Style::default().fg(Color::DarkGray);
    let text = if filtering || !query.is_empty() {
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
            Span::styled(" navigate  ", dim),
            Span::styled("enter", dim),
            Span::styled(" select  ", dim),
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
