use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::ports::PortRow;

pub struct View<'a> {
    pub rows: &'a [PortRow],
    pub selected: usize,
    pub scan_error: Option<&'a str>,
    pub status: ViewStatus<'a>,
}

#[derive(Clone, Copy)]
pub enum ViewStatus<'a> {
    Ready { message: &'a str },
    Reconnecting { message: &'a str },
    ConfirmKill { row: &'a PortRow },
}

pub fn draw(frame: &mut Frame<'_>, view: &View<'_>) {
    let area = frame.area();
    frame.render_widget(Clear, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(frame, chunks[0], view.rows.len());
    match view.status {
        ViewStatus::Reconnecting { message } => draw_reconnect(frame, chunks[1], message),
        _ => draw_rows(frame, chunks[1], view),
    }
    draw_status(frame, chunks[2], view.status);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, count: usize) {
    let text = format!("Listening ports ({count})");
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            middle_truncate(&text, area.width as usize),
            Style::new().add_modifier(Modifier::BOLD),
        ))),
        area,
    );
}

fn draw_rows(frame: &mut Frame<'_>, area: Rect, view: &View<'_>) {
    if area.height == 0 {
        return;
    }

    if let Some(error) = view.scan_error {
        draw_message_rows(frame, area, &["Port scan error", error]);
        return;
    }
    if view.rows.is_empty() {
        frame.render_widget(Paragraph::new("No listening TCP ports"), area);
        return;
    }

    let visible_height = area.height as usize;
    let offset = scroll_offset(view.selected, visible_height, view.rows.len());
    for (line_index, row) in view
        .rows
        .iter()
        .skip(offset)
        .take(visible_height)
        .enumerate()
    {
        let selected = offset + line_index == view.selected;
        let row_area = Rect::new(area.x, area.y + line_index as u16, area.width, 1);
        frame.render_widget(
            Paragraph::new(row_line(row, area.width as usize, selected)),
            row_area,
        );
    }
}

fn draw_reconnect(frame: &mut Frame<'_>, area: Rect, message: &str) {
    draw_message_rows(
        frame,
        area,
        &[
            "Reconnecting to cmux",
            message,
            "Set CMUX_TUI_SOCKET to the cmux-tui JSON-lines socket path.",
        ],
    );
}

fn draw_message_rows(frame: &mut Frame<'_>, area: Rect, lines: &[&str]) {
    for (index, line) in lines.iter().enumerate() {
        if index >= area.height as usize {
            break;
        }
        frame.render_widget(
            Paragraph::new(middle_truncate(line, area.width as usize)),
            Rect::new(area.x, area.y + index as u16, area.width, 1),
        );
    }
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, status: ViewStatus<'_>) {
    let (message, style) = match status {
        ViewStatus::Ready { message } => (message, Style::new().add_modifier(Modifier::DIM)),
        ViewStatus::Reconnecting { .. } => (
            "Ctrl-C quit • Esc stays",
            Style::new().add_modifier(Modifier::DIM),
        ),
        ViewStatus::ConfirmKill { row } => {
            let message = format!("SIGTERM {} ({})? y/n", row.process, row.pid);
            let message = middle_truncate(&message, area.width as usize);
            frame.render_widget(
                Paragraph::new(message).style(Style::new().fg(Color::Yellow)),
                area,
            );
            return;
        }
    };
    frame.render_widget(
        Paragraph::new(middle_truncate(message, area.width as usize)).style(style),
        area,
    );
}

fn row_line(row: &PortRow, width: usize, selected: bool) -> Line<'static> {
    let label = middle_truncate(&row.label(), width);
    let mut style = Style::new();
    if row.is_common_dev_port() {
        style = style.fg(Color::Cyan);
    }
    if row.is_new {
        style = style.fg(Color::Green).add_modifier(Modifier::BOLD);
    }
    if selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    Line::from(Span::styled(label, style))
}

fn scroll_offset(selected: usize, visible_height: usize, total: usize) -> usize {
    if visible_height == 0 || total <= visible_height || selected < visible_height {
        return 0;
    }
    (selected + 1)
        .saturating_sub(visible_height)
        .min(total - visible_height)
}

pub fn middle_truncate(input: &str, max_chars: usize) -> String {
    let chars = input.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return input.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }

    let keep = max_chars - 3;
    let front = keep.div_ceil(2);
    let back = keep / 2;
    format!(
        "{}...{}",
        chars[..front].iter().collect::<String>(),
        chars[chars.len() - back..].iter().collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn middle_truncates_for_narrow_sidebars() {
        assert_eq!(
            middle_truncate("abcdefghijklmnopqrstuvwxyz", 9),
            "abc...xyz"
        );
        assert_eq!(middle_truncate("abcdef", 3), "...");
        assert_eq!(middle_truncate("abcdef", 0), "");
    }
}
