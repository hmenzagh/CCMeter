use chrono::TimeZone;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use crate::data::oauth::{OAuthCredential, UsageReport, UsageStats, UsageWindow};
use crate::data::rate_limits::RateLimitHit;

use super::theme::theme;

fn source_display_name<'a>(
    source_root: &'a str,
    source_names: &'a [String],
    source_roots: &[Option<String>],
) -> &'a str {
    for (i, root) in source_roots.iter().enumerate() {
        if let Some(r) = root {
            if r == source_root {
                return &source_names[i];
            }
        }
    }
    source_root
        .rsplit('/')
        .find(|s| !s.is_empty() && *s != "projects")
        .unwrap_or(source_root)
}

fn source_color_index(source_root: &str, all_roots: &[String]) -> usize {
    all_roots
        .iter()
        .position(|r| r == source_root)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Gradient bar rendering with unicode blocks
// ---------------------------------------------------------------------------

/// Interpolate color from green → yellow → orange → red based on position
/// within the filled portion. The bar transitions smoothly across its length.
fn gradient_color(position_ratio: f64) -> Color {
    // 4-stop gradient: green(0.0) → yellow(0.33) → orange(0.66) → red(1.0)
    let (r, g, b) = if position_ratio < 0.33 {
        let t = position_ratio / 0.33;
        (
            80.0 + t * 140.0,  // 80 → 220
            200.0 + t * 0.0,   // 200 → 200
            80.0 - t * 40.0,   // 80 → 40
        )
    } else if position_ratio < 0.66 {
        let t = (position_ratio - 0.33) / 0.33;
        (
            220.0,              // 220
            200.0 - t * 60.0,  // 200 → 140
            40.0,               // 40
        )
    } else {
        let t = (position_ratio - 0.66) / 0.34;
        (
            220.0,              // 220
            140.0 - t * 90.0,  // 140 → 50
            40.0 + t * 10.0,   // 40 → 50
        )
    };
    Color::Rgb(r as u8, g as u8, b as u8)
}

/// Render a gradient bar into a single-line Rect.
/// Label on the left (fixed width), bar fills the rest with per-cell gradient color.
fn gradient_bar_line<'a>(
    total_width: u16,
    ratio: f64,
    label: &str,
    pct: f64,
    reset: &str,
) -> Line<'a> {
    let t = theme();

    let label_text = format!("{:<7}{:>3.0}% ~{} ", label, pct, reset);
    let label_len = label_text.len();

    let bar_width = (total_width as usize).saturating_sub(label_len);
    if bar_width == 0 {
        return Line::from(Span::styled(label_text, Style::default().fg(t.text_primary)));
    }

    let mut spans: Vec<Span<'a>> = Vec::with_capacity(bar_width + 2);
    spans.push(Span::styled(label_text, Style::default().fg(t.text_primary)));

    let filled = (ratio * bar_width as f64) as usize;

    // Filled cells with gradient
    for i in 0..filled.min(bar_width) {
        let pos = if bar_width > 1 { i as f64 / (bar_width - 1) as f64 } else { 0.0 };
        spans.push(Span::styled(
            " ",
            Style::default().bg(gradient_color(pos)),
        ));
    }

    // Empty remainder
    let empty_count = bar_width.saturating_sub(filled);
    if empty_count > 0 {
        spans.push(Span::styled(
            " ".repeat(empty_count),
            Style::default().bg(t.empty_bar),
        ));
    }

    Line::from(spans)
}

/// Color for a utilization percentage (used for extra usage text).
fn util_color(pct: f64) -> Color {
    if pct >= 75.0 {
        Color::Rgb(220, 50, 50)
    } else if pct >= 50.0 {
        Color::Rgb(220, 140, 40)
    } else if pct >= 25.0 {
        Color::Rgb(220, 200, 40)
    } else {
        Color::Rgb(80, 200, 80)
    }
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

pub(crate) fn render(
    frame: &mut Frame,
    area: Rect,
    hits: &[RateLimitHit],
    source_names: &[String],
    source_roots: &[Option<String>],
    credentials: &[OAuthCredential],
    selected: Option<usize>,
) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(outer[0]);

    // Filter rate limits by the selected credential's source_root
    let filtered_hits: Vec<&RateLimitHit> = match selected {
        Some(i) if i < credentials.len() => {
            let root = credentials[i].source_root.to_string_lossy().to_string();
            hits.iter().filter(|h| h.source_root == root).collect()
        }
        _ => hits.iter().collect(),
    };

    // Build credential roots once so rate-limits table and cards share the same color mapping
    let credential_roots: Vec<String> = credentials
        .iter()
        .map(|c| c.source_root.to_string_lossy().to_string())
        .collect();

    render_rate_limits(frame, columns[0], &filtered_hits, source_names, source_roots, &credential_roots);
    render_credential_cards(frame, columns[1], credentials, source_names, source_roots, selected, &credential_roots);

    // Footer with key hints
    render_footer(frame, outer[1]);
}

fn count_gauges(usage: Option<&UsageReport>) -> usize {
    let Some(u) = usage else { return 0 };
    [
        u.five_hour.is_some(),
        u.seven_day.is_some(),
        u.seven_day_opus.is_some(),
        u.seven_day_sonnet.is_some(),
        u.seven_day_cowork.is_some(),
    ]
    .iter()
    .filter(|&&v| v)
    .count()
}

// ---------------------------------------------------------------------------
// Credential cards (top-right, side by side)
// ---------------------------------------------------------------------------

fn render_footer(frame: &mut Frame, area: Rect) {
    let t = theme();
    let text = "←→ Select source   ` Dashboard   q Quit";
    let footer = Paragraph::new(Span::styled(text, Style::default().fg(t.text_dim)))
        .alignment(Alignment::Center);
    frame.render_widget(footer, area);
}

fn render_credential_cards(
    frame: &mut Frame,
    area: Rect,
    credentials: &[OAuthCredential],
    source_names: &[String],
    source_roots: &[Option<String>],
    selected: Option<usize>,
    credential_roots: &[String],
) {
    let t = theme();

    if credentials.is_empty() {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.border))
            .title(Span::styled(" No OAuth ", Style::default().fg(t.text_dim)));
        frame.render_widget(block, area);
        return;
    }

    let max_gauges = credentials
        .iter()
        .map(|c| count_gauges(c.usage.as_ref()))
        .max()
        .unwrap_or(0);
    // border(2) + gauges + status(1)
    let card_h = (3 + max_gauges).max(4) as u16;

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(card_h), Constraint::Min(0)])
        .split(area);

    let constraints: Vec<Constraint> = credentials
        .iter()
        .map(|_| Constraint::Ratio(1, credentials.len() as u32))
        .collect();
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(rows[0]);

    for (i, cred) in credentials.iter().enumerate() {
        let is_selected = selected == Some(i);
        render_card(frame, cols[i], cred, source_names, source_roots, credential_roots, is_selected);
    }
}

fn render_card(
    frame: &mut Frame,
    area: Rect,
    cred: &OAuthCredential,
    source_names: &[String],
    source_roots: &[Option<String>],
    credential_roots: &[String],
    is_selected: bool,
) {
    let t = theme();
    let root_str = cred.source_root.to_string_lossy().to_string();
    let name = source_display_name(&root_str, source_names, source_roots);
    let color_idx = source_color_index(&root_str, credential_roots);
    let color = t.rainbow[color_idx % t.rainbow.len()];

    let sub = cred.subscription_type.as_deref().unwrap_or("?");
    let title_left = format!(" {} ({}) ", name, sub);

    // Extra usage in title bar (right-aligned)
    let title_right = extra_usage_title(cred.usage.as_ref());

    let border_color = if is_selected { t.border_highlight } else { t.border };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .border_type(ratatui::widgets::BorderType::Rounded)
        .title(Span::styled(
            title_left,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));

    if !title_right.is_empty() {
        block = block.title_top(
            Line::from(title_right).alignment(Alignment::Right),
        );
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);

    match &cred.usage {
        Some(usage) => render_compact_usage(frame, inner, usage, &cred.stats),
        None => {
            let line = Line::from(vec![
                Span::styled("loading...", Style::default().fg(t.text_dim)),
                Span::styled(
                    format!(" ({}req, {}err)", cred.stats.call_count, cred.stats.rate_limit_count),
                    Style::default().fg(t.text_dim),
                ),
            ]);
            frame.render_widget(Paragraph::new(line), inner);
        }
    }
}

/// Build extra usage spans for the block title (right side).
fn extra_usage_title(usage: Option<&UsageReport>) -> Vec<Span<'static>> {
    let Some(u) = usage else { return vec![] };
    let Some(extra) = &u.extra_usage else { return vec![] };
    if !extra.is_enabled || extra.used_credits.is_none() {
        return vec![];
    }

    let used = extra.used_credits.unwrap_or(0.0) / 100.0;
    let limit = extra.monthly_limit.unwrap_or(0.0) / 100.0;
    let util = extra.utilization.unwrap_or(0.0);

    vec![
        Span::styled(
            format!("${:.2}/${:.2} ({:.0}%) ", used, limit, util),
            Style::default()
                .fg(util_color(util))
                .add_modifier(Modifier::BOLD),
        ),
    ]
}

fn render_compact_usage(
    frame: &mut Frame,
    area: Rect,
    usage: &UsageReport,
    stats: &UsageStats,
) {
    let t = theme();

    let windows: &[(&str, Option<&UsageWindow>)] = &[
        ("5h", usage.five_hour.as_ref()),
        ("7d", usage.seven_day.as_ref()),
        ("opus", usage.seven_day_opus.as_ref()),
        ("sonnet", usage.seven_day_sonnet.as_ref()),
        ("cowork", usage.seven_day_cowork.as_ref()),
    ];
    let items: Vec<(&str, f64, String)> = windows
        .iter()
        .filter_map(|(label, w)| w.map(|w| (*label, w.utilization, format_reset(&w.resets_at))))
        .collect();

    let mut constraints: Vec<Constraint> = Vec::new();
    for _ in &items {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // status line
    constraints.push(Constraint::Min(0));

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    for (i, (label, pct, reset)) in items.iter().enumerate() {
        let ratio = (*pct / 100.0).clamp(0.0, 1.0);
        let line = gradient_bar_line(area.width, ratio, label, *pct, reset);
        frame.render_widget(Paragraph::new(line), rows[i]);
    }

    // Status line
    let status = Line::from(vec![Span::styled(
        format!(
            "polled {} ({}req, {}err)",
            stats.last_fetch_ago(),
            stats.call_count,
            stats.rate_limit_count,
        ),
        Style::default().fg(t.text_dim),
    )]);
    frame.render_widget(Paragraph::new(status), rows[items.len()]);
}

// ---------------------------------------------------------------------------
// Rate limits table (left panel)
// ---------------------------------------------------------------------------

fn render_rate_limits(
    frame: &mut Frame,
    area: Rect,
    hits: &[&RateLimitHit],
    source_names: &[String],
    source_roots: &[Option<String>],
    credential_roots: &[String],
) {
    let t = theme();

    let rows: Vec<Row> = hits
        .iter()
        .map(|hit| {
            let local_ts = chrono::Local.from_utc_datetime(&hit.timestamp.naive_utc());
            let date_str = local_ts.format("%m-%d %H:%M").to_string();
            let name = source_display_name(&hit.source_root, source_names, source_roots);
            let duration_str = match hit.session_duration_min {
                Some(min) if min >= 1.0 => {
                    let h = min as u64 / 60;
                    let m = min as u64 % 60;
                    if h > 0 { format!("{}h{:02}", h, m) } else { format!("{}m", m) }
                }
                Some(_) => "<1m".to_string(),
                None => "—".to_string(),
            };
            let color_idx = source_color_index(&hit.source_root, credential_roots);
            let color = t.rainbow[color_idx % t.rainbow.len()];

            Row::new(vec![
                Cell::from(date_str).style(Style::default().fg(t.text_primary)),
                Cell::from(name.to_string()).style(Style::default().fg(color)),
                Cell::from(duration_str).style(Style::default().fg(t.text_secondary)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Min(10),
            Constraint::Length(6),
        ],
    )
    .header(
        Row::new(vec![
            Cell::from("Date").style(Style::default().fg(t.text_dim).add_modifier(Modifier::BOLD)),
            Cell::from("Source").style(Style::default().fg(t.text_dim).add_modifier(Modifier::BOLD)),
            Cell::from("Dur").style(Style::default().fg(t.text_dim).add_modifier(Modifier::BOLD)),
        ])
        .bottom_margin(1),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(t.border))
            .title(Span::styled(
                format!(" Rate Limits ({}) ", hits.len()),
                Style::default()
                    .fg(t.heatmap_title)
                    .add_modifier(Modifier::BOLD),
            )),
    )
    .column_spacing(1);

    frame.render_widget(table, area);
}

fn format_reset(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%a %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| iso.to_string())
}
