use ratatui::{Terminal, backend::TestBackend, prelude::*};

/// Quick visual test: renders just the update banner line to verify
/// the rainbow text appears correctly.
#[test]
fn update_banner_renders() {
    let backend = TestBackend::new(60, 1);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal
        .draw(|frame| {
            let area = frame.area();
            let rainbow = [
                Color::Rgb(255, 80, 80),
                Color::Rgb(255, 160, 50),
                Color::Rgb(255, 230, 50),
                Color::Rgb(80, 220, 80),
                Color::Rgb(80, 170, 240),
                Color::Rgb(190, 120, 240),
            ];
            let text = " \u{2b06} v99.0.0 available — brew upgrade ccmeter ";
            let tick = 0usize;
            let spans: Vec<Span> = text
                .chars()
                .enumerate()
                .map(|(i, ch)| {
                    let color = rainbow[(i + tick) % rainbow.len()];
                    Span::styled(
                        String::from(ch),
                        Style::default()
                            .fg(color)
                            .add_modifier(Modifier::BOLD),
                    )
                })
                .collect();
            let line = Line::from(spans);
            frame.render_widget(
                ratatui::widgets::Paragraph::new(line).alignment(Alignment::Center),
                area,
            );
        })
        .unwrap();

    let buf = terminal.backend().buffer().clone();
    let rendered: String = (0..buf.area.width)
        .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
        .collect();
    let rendered = rendered.trim();
    println!("Rendered banner: [{rendered}]");
    assert!(
        rendered.contains("v99.0.0"),
        "Banner should contain version: got '{rendered}'"
    );
    assert!(
        rendered.contains("brew upgrade"),
        "Banner should contain install hint: got '{rendered}'"
    );
}
