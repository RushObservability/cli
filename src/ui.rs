use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
};

use crate::{
    app::{App, InputMode},
    model::TailRecord,
};

const AMBER: Color = Color::Rgb(245, 158, 11);
const BLUE: Color = Color::Rgb(96, 165, 250);
const MUTED: Color = Color::Rgb(120, 128, 145);

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(8),
            Constraint::Length(if app.input_mode == InputMode::Normal {
                2
            } else {
                3
            }),
        ])
        .split(area);

    draw_header(frame, chunks[0], app);
    draw_query(frame, chunks[1], app);
    draw_content(frame, chunks[2], app);
    draw_footer(frame, chunks[3], app);

    if app.show_help {
        draw_help(frame, centered_rect(64, 70, area));
    }
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let state = if app.paused { "PAUSED" } else { "LIVE" };
    let state_color = if app.paused { AMBER } else { Color::Green };
    let pending = if app.paused && app.pending_count > 0 {
        format!("  +{} buffered", app.pending_count)
    } else if app.new_count > 0 {
        format!("  +{} new", app.new_count)
    } else {
        String::new()
    };
    let updated = app
        .last_updated
        .map(|value| value.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "connecting".into());
    let title = Line::from(vec![
        Span::styled(
            " RUSH ",
            Style::default()
                .fg(Color::Black)
                .bg(AMBER)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            app.spec.signal.to_string().to_uppercase(),
            Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            state,
            Style::default()
                .fg(state_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(pending, Style::default().fg(state_color)),
    ]);
    let right = format!("{} rows  •  {} ", app.records.len(), updated);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(title),
        Rect {
            width: inner.width.saturating_sub(right.len() as u16),
            ..inner
        },
    );
    frame.render_widget(
        Paragraph::new(right)
            .alignment(Alignment::Right)
            .style(Style::default().fg(MUTED)),
        inner,
    );
}

fn draw_query(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let filters = if app.spec.filters.is_empty() {
        "no field filters".to_string()
    } else {
        app.spec
            .filters
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("  ")
    };
    let search = if app.spec.search.is_empty() {
        "no text search"
    } else {
        app.spec.search.as_str()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" search ", Style::default().fg(MUTED)),
            Span::styled(search, Style::default().fg(Color::White)),
            Span::styled("   filters ", Style::default().fg(MUTED)),
            Span::styled(filters, Style::default().fg(AMBER)),
        ])),
        area,
    );
}

fn draw_content(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let regions = if app.show_detail {
        Layout::default()
            .direction(if area.width >= 120 {
                Direction::Horizontal
            } else {
                Direction::Vertical
            })
            .constraints(if area.width >= 120 {
                [Constraint::Percentage(66), Constraint::Percentage(34)]
            } else {
                [Constraint::Percentage(62), Constraint::Percentage(38)]
            })
            .split(area)
    } else {
        Layout::default()
            .constraints([Constraint::Percentage(100)])
            .split(area)
    };
    draw_table(frame, regions[0], app);
    if app.show_detail {
        draw_detail(frame, regions[1], app.selected(), app.wrap);
    }
}

fn draw_table(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let rows = app.records.iter().map(|record| {
        let level_style = level_style(&record.level);
        Row::new(vec![
            Cell::from(record.timestamp()).style(Style::default().fg(MUTED)),
            Cell::from(record.level.to_uppercase()).style(level_style),
            Cell::from(record.service.clone()).style(Style::default().fg(BLUE)),
            Cell::from(record.summary.replace('\n', " ")),
            Cell::from(record.duration_ns.map(format_duration).unwrap_or_default())
                .style(Style::default().fg(MUTED)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(13),
            Constraint::Length(9),
            Constraint::Length(20),
            Constraint::Min(24),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(["TIME", "LEVEL", "SERVICE", "EVENT", "DURATION"])
            .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD))
            .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" stream ")
            .border_style(Style::default().fg(Color::DarkGray)),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::Rgb(32, 39, 52))
            .add_modifier(Modifier::BOLD),
    )
    .highlight_symbol("▸ ");
    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn draw_detail(frame: &mut Frame<'_>, area: Rect, selected: Option<&TailRecord>, wrap: bool) {
    let text = match selected {
        Some(record) => Text::from(vec![
            detail_line("signal", record.signal.to_string()),
            detail_line("time", record.timestamp()),
            detail_line("service", &record.service),
            detail_line("level", &record.level),
            detail_line("trace", empty_as_dash(&record.trace_id)),
            detail_line("span", empty_as_dash(&record.span_id)),
            detail_line(
                "status",
                record
                    .http_status_code
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "—".into()),
            ),
            detail_line(
                "duration",
                record
                    .duration_ns
                    .map(format_duration)
                    .unwrap_or_else(|| "—".into()),
            ),
            Line::raw(""),
            Line::styled(
                "message",
                Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
            ),
            Line::raw(record.summary.clone()),
            Line::raw(""),
            Line::styled(
                "o  open this context in Rush web",
                Style::default().fg(AMBER),
            ),
        ]),
        None => Text::from("No row selected"),
    };
    let mut paragraph = Paragraph::new(text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" context ")
            .border_style(Style::default().fg(AMBER)),
    );
    if wrap {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
    frame.render_widget(paragraph, area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.input_mode != InputMode::Normal {
        let label = if app.input_mode == InputMode::Search {
            "search"
        } else {
            "filter"
        };
        let hint = if app.input_mode == InputMode::Filter {
            "  e.g. service_name=gateway"
        } else {
            ""
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {label} "))
            .border_style(Style::default().fg(AMBER));
        frame.render_widget(
            Paragraph::new(format!("{}{}", app.input, hint)).block(block),
            area,
        );
        frame.set_cursor_position((area.x + 1 + app.input.len() as u16, area.y + 1));
        return;
    }
    let message = app.error.as_deref().unwrap_or(
        "space pause  / search  f filter  Tab logs/APM  Enter details  o web  ? help  q quit",
    );
    let color = if app.error.is_some() {
        Color::Red
    } else {
        MUTED
    };
    frame.render_widget(
        Paragraph::new(message).style(Style::default().fg(color)),
        area,
    );
}

fn draw_help(frame: &mut Frame<'_>, area: Rect) {
    frame.render_widget(Clear, area);
    let help = Text::from(vec![
        Line::styled(
            "Rush live tail",
            Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw("space    pause/resume (polling continues into a buffer)"),
        Line::raw("Tab      switch logs/APM"),
        Line::raw("/        edit server-side free-text search"),
        Line::raw("f        add field filter (field=value, !=, >=, <=, >, <, ~)"),
        Line::raw("x        remove the last field filter"),
        Line::raw("c        clear search and filters"),
        Line::raw("r        refresh immediately"),
        Line::raw("j/k      move selection"),
        Line::raw("g/G      newest/oldest row"),
        Line::raw("Enter    toggle selected-row details"),
        Line::raw("w        toggle detail word wrapping"),
        Line::raw("o        open selected trace/log context in Rush web"),
        Line::raw("q        quit"),
        Line::raw(""),
        Line::styled("Press ? or Esc to close", Style::default().fg(MUTED)),
    ]);
    frame.render_widget(
        Paragraph::new(help).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" help ")
                .border_style(Style::default().fg(AMBER)),
        ),
        area,
    );
}

fn detail_line(label: &str, value: impl Into<String>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<9}"), Style::default().fg(MUTED)),
        Span::raw(value.into()),
    ])
}

fn empty_as_dash(value: &str) -> &str {
    if value.is_empty() { "—" } else { value }
}

fn format_duration(duration_ns: u64) -> String {
    if duration_ns >= 1_000_000_000 {
        format!("{:.2}s", duration_ns as f64 / 1_000_000_000.0)
    } else if duration_ns >= 1_000_000 {
        format!("{:.1}ms", duration_ns as f64 / 1_000_000.0)
    } else if duration_ns >= 1_000 {
        format!("{:.1}µs", duration_ns as f64 / 1_000.0)
    } else {
        format!("{duration_ns}ns")
    }
}

fn level_style(level: &str) -> Style {
    let lower = level.to_ascii_lowercase();
    if lower.contains("error") || lower.contains("fatal") {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else if lower.contains("warn") {
        Style::default().fg(AMBER)
    } else if lower.contains("debug") || lower.contains("trace") {
        Style::default().fg(MUTED)
    } else {
        Style::default().fg(Color::Green)
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ratatui::{Terminal, backend::TestBackend};
    use tokio::sync::watch;

    use crate::model::{QuerySpec, Signal};

    use super::*;

    #[test]
    fn formats_durations_at_readable_units() {
        assert_eq!(format_duration(850), "850ns");
        assert_eq!(format_duration(2_500_000), "2.5ms");
        assert_eq!(format_duration(1_500_000_000), "1.50s");
    }

    #[test]
    fn renders_live_stream_shell() {
        let spec = QuerySpec {
            signal: Signal::Logs,
            search: "timeout".into(),
            filters: vec!["service_name=gateway".parse().unwrap()],
            window: Duration::from_secs(300),
            limit: 500,
        };
        let (tx, _) = watch::channel(spec.clone());
        let mut app = App::new(spec, "http://localhost:5173".into(), 5000, tx);
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let buffer = terminal.backend().buffer();
        let mut rendered = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                rendered.push_str(buffer[(x, y)].symbol());
            }
        }
        assert!(rendered.contains("RUSH"));
        assert!(rendered.contains("LIVE"));
        assert!(rendered.contains("service_name=gateway"));
    }
}
