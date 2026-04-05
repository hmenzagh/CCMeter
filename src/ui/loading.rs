use std::time::Duration;

use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};

use super::theme::theme;

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(crate) fn draw_loading(frame: &mut Frame, elapsed: Duration) {
    let t = theme();
    let area = frame.area();

    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.border)),
        area,
    );

    if area.height < 5 || area.width < 20 {
        return;
    }

    let spinner = SPINNER_FRAMES[(elapsed.as_millis() / 80) as usize % SPINNER_FRAMES.len()];

    let rows = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .split(area);

    frame.render_widget(
        Paragraph::new(Span::styled(
            "CCMeter",
            Style::default().fg(t.title).add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Span::styled(
            format!("{spinner}  Parsing session files…"),
            Style::default().fg(t.tokens_out),
        ))
        .alignment(Alignment::Center),
        rows[3],
    );

    frame.render_widget(
        Paragraph::new(Span::styled(
            "Press q to quit",
            Style::default().fg(t.text_dim),
        ))
        .alignment(Alignment::Center),
        rows[5],
    );
}
