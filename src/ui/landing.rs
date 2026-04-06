use ratatui::prelude::*;

pub(crate) fn render(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        ratatui::widgets::Block::default().style(Style::default()),
        area,
    );
}
