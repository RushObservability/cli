use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
        draw_detail(frame, regions[1], app.selected());
    }
}

fn draw_table(frame: &mut Frame<'_>, area: Rect, app: &mut App) {
    let message_width = usize::from(area.width.saturating_sub(60)).max(1);
    let rows = app.records.iter().map(|record| {
        let level_style = level_style(&record.level);
        let messages = if app.stream_wrap {
            wrap_message(&record.summary, message_width, 3)
        } else {
            vec![record.summary.replace('\n', " ")]
        };
        let row_height = messages.len() as u16;
        Row::new(vec![
            Cell::from(record.timestamp()).style(Style::default().fg(MUTED)),
            Cell::from(record.level.to_uppercase()).style(level_style),
            Cell::from(record.service.clone()).style(Style::default().fg(BLUE)),
            Cell::from(messages.join("\n")),
            Cell::from(record.duration_ns.map(format_duration).unwrap_or_default())
                .style(Style::default().fg(MUTED)),
        ])
        .height(row_height)
    });
    let wrap_status = if app.stream_wrap { "on" } else { "off" };
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
            .title(format!(" stream · wrap {wrap_status} "))
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

fn draw_detail(frame: &mut Frame<'_>, area: Rect, selected: Option<&TailRecord>) {
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
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" context · message wraps ")
                    .border_style(Style::default().fg(AMBER)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.input_mode != InputMode::Normal {
        let label = if app.input_mode == InputMode::Search {
            "SEARCH — filters + text"
        } else {
            "FILTER — edit current"
        };
        let placeholder = if app.input_mode == InputMode::Filter {
            "e.g. service_name=gateway"
        } else {
            "e.g. service_name=gateway POST"
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {label} · Enter apply · Esc cancel "))
            .border_style(Style::default().fg(AMBER));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let cursor_width = UnicodeWidthStr::width(&app.input[..app.input_cursor]);
        let viewport_width = usize::from(inner.width);
        let scroll = cursor_width.saturating_sub(viewport_width.saturating_sub(1));
        let text = if app.input.is_empty() {
            Line::styled(placeholder, Style::default().fg(MUTED))
        } else {
            Line::raw(app.input.as_str())
        };
        frame.render_widget(
            Paragraph::new(text).scroll((0, scroll.min(usize::from(u16::MAX)) as u16)),
            inner,
        );
        let cursor_column = cursor_width
            .saturating_sub(scroll)
            .min(viewport_width.saturating_sub(1));
        frame.set_cursor_position((inner.x + cursor_column as u16, inner.y));
        return;
    }
    if let Some(error) = app.error.as_deref() {
        frame.render_widget(
            Paragraph::new(error).style(Style::default().fg(Color::Red)),
            area,
        );
        return;
    }

    let primary_key = Style::default().fg(AMBER).add_modifier(Modifier::BOLD);
    let primary_action = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    let secondary_key = Style::default().fg(BLUE);
    let secondary_action = Style::default().fg(MUTED);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("[/]", primary_key),
            Span::styled(" Edit search", primary_action),
            Span::raw("   "),
            Span::styled("[f]", primary_key),
            Span::styled(" Edit filter", primary_action),
            Span::raw("   "),
            Span::styled("[Space]", secondary_key),
            Span::styled(" Pause", secondary_action),
            Span::raw("   "),
            Span::styled("[w]", secondary_key),
            Span::styled(" Wrap", secondary_action),
            Span::raw("   "),
            Span::styled("[Enter]", secondary_key),
            Span::styled(" Details", secondary_action),
            Span::raw("   "),
            Span::styled("[?]", secondary_key),
            Span::styled(" All keys", secondary_action),
        ])),
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
        Line::raw("/        edit filters + free text (service_name=gateway POST)"),
        Line::raw("         ←/→ move  Home/End jump  Backspace/Delete edit"),
        Line::raw("f        edit last field filter (creates one when empty)"),
        Line::raw("x        remove the last field filter"),
        Line::raw("c        clear search and filters"),
        Line::raw("r        refresh immediately"),
        Line::raw("j/k      move selection"),
        Line::raw("g/G      newest/oldest row"),
        Line::raw("Enter    toggle selected-row details"),
        Line::raw("w        toggle main stream message wrapping (up to 3 lines)"),
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

fn wrap_message(message: &str, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut line = String::new();
    let mut line_width = 0;
    let mut truncated = false;

    for character in message.chars() {
        if character == '\n' {
            if !line.is_empty() {
                lines.push(line.trim_end().to_string());
                line.clear();
                line_width = 0;
            }
            if lines.len() == max_lines {
                truncated = true;
                break;
            }
            continue;
        }

        let character_width = character.width().unwrap_or(0);
        if line_width > 0 && line_width + character_width > width {
            lines.push(line.trim_end().to_string());
            line.clear();
            line_width = 0;
            if lines.len() == max_lines {
                truncated = true;
                break;
            }
        }
        if line.is_empty() && character.is_whitespace() {
            continue;
        }
        line.push(character);
        line_width += character_width;
    }

    if !line.is_empty() && lines.len() < max_lines {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    if truncated {
        let last = lines
            .last_mut()
            .expect("wrapped messages always have a line");
        while UnicodeWidthStr::width(last.as_str()) >= width && !last.is_empty() {
            last.pop();
        }
        last.push('…');
    }
    lines
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
        assert!(rendered.contains("[/] Edit search"));
        assert!(rendered.contains("[f] Edit filter"));
    }

    #[test]
    fn wraps_stream_messages_and_caps_the_row_height() {
        assert_eq!(
            wrap_message("request failed while calling upstream", 10, 3),
            vec!["request fa", "iled while", "calling u…"]
        );
        assert_eq!(wrap_message("short", 10, 3), vec!["short"]);
    }
}
