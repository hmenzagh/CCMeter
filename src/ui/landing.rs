use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate, TimeZone};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use crate::data::index::EventIndex;
use crate::data::models::format_tokens;
use crate::data::oauth::{OAuthCredential, UsageReport, UsageStats, UsageWindow};
use crate::data::rate_history::RateHistory;
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

    let reset_part = if reset.is_empty() {
        String::new()
    } else {
        format!("~{}", reset)
    };
    let label_text = format!("{:<7}{:>3.0}% {:<12} ", label, pct, reset_part);
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
    index: &EventIndex,
    rate_history: &RateHistory,
    tick: usize,
) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    // Left: rate limits table (full height) | Right: cards (top) + chart (bottom)
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(outer[0]);

    // Filter rate limits by the selected credential's source_root
    let selected_root: Option<String> = selected
        .filter(|&i| i < credentials.len())
        .map(|i| credentials[i].source_root.to_string_lossy().to_string());

    let filtered_hits: Vec<&RateLimitHit> = match &selected_root {
        Some(root) => hits.iter().filter(|h| h.source_root == *root).collect(),
        None => hits.iter().collect(),
    };

    // Build credential roots once so rate-limits table and cards share the same color mapping
    let credential_roots: Vec<String> = credentials
        .iter()
        .map(|c| c.source_root.to_string_lossy().to_string())
        .collect();

    render_rate_limits(frame, columns[0], &filtered_hits, source_names, source_roots, &credential_roots);

    // Right panel: cards (top) + chart (rest)
    let right_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(count_card_height(credentials)), Constraint::Min(4)])
        .split(columns[1]);

    render_credential_cards(frame, right_rows[0], credentials, source_names, source_roots, selected, &credential_roots);

    let selected_cred = selected.filter(|&i| i < credentials.len()).map(|i| &credentials[i]);
    let bars = compute_session_bars(&filtered_hits, index, selected_root.as_deref(), selected_cred, rate_history);
    render_session_chart(frame, right_rows[1], &bars, tick);

    // Footer
    render_footer(frame, outer[1]);
}

fn count_card_height(credentials: &[OAuthCredential]) -> u16 {
    if credentials.is_empty() {
        return 4;
    }
    let max_gauges = credentials
        .iter()
        .map(|c| count_gauges(c.usage.as_ref()))
        .max()
        .unwrap_or(0);
    (3 + max_gauges).max(4) as u16
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
        .filter_map(|(label, w)| w.map(|w| (*label, w.utilization, w.resets_at.as_deref().map(format_reset).unwrap_or_default())))
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

// ---------------------------------------------------------------------------
// Session token bar chart
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum BarKind {
    /// Violet: historical average from rate-history (no rate-limit hit that day).
    History,
    /// Red: historical average from rate-history, only sessions that hit the limit.
    RateLimited,
    /// Blue: current live session (always last bar).
    Current,
}

struct DayBar {
    date: NaiveDate,
    tokens: u64,
    kind: BarKind,
}

/// Compute token bars:
/// - Violet/Red bars from rate-history (one per day, averaged).
///   Red if at least one rate-limit hit that day, violet otherwise.
/// - Blue bar for the current live session (always appended last).
fn compute_session_bars(
    hits: &[&RateLimitHit],
    index: &EventIndex,
    source_root: Option<&str>,
    selected_cred: Option<&OAuthCredential>,
    rate_history: &RateHistory,
) -> Vec<DayBar> {
    let Some(root) = source_root else {
        return Vec::new();
    };

    let mut rate_limited_days: std::collections::HashSet<NaiveDate> =
        std::collections::HashSet::new();
    let mut rate_limited_tokens: BTreeMap<NaiveDate, Vec<u64>> = BTreeMap::new();
    for hit in hits {
        let day = hit
            .timestamp
            .with_timezone(&chrono::Local)
            .naive_local()
            .date();
        rate_limited_days.insert(day);
        if hit.tokens > 0 {
            rate_limited_tokens.entry(day).or_default().push(hit.tokens);
        }
    }

    let entries = rate_history.entries_for_root(root);

    let mut day_entries: BTreeMap<NaiveDate, Vec<u64>> = BTreeMap::new();
    for entry in &entries {
        day_entries
            .entry(entry.date)
            .or_default()
            .push(entry.estimated_tokens);
    }

    let mut bars: Vec<DayBar> = Vec::new();

    let all_dates: std::collections::BTreeSet<NaiveDate> = day_entries
        .keys()
        .chain(rate_limited_days.iter())
        .copied()
        .collect();

    for date in all_dates {
        if rate_limited_days.contains(&date) {
            // Red bar: average of rate-limited session tokens
            let tokens_list = rate_limited_tokens.get(&date);
            if let Some(list) = tokens_list {
                if !list.is_empty() {
                    let avg = list.iter().sum::<u64>() / list.len() as u64;
                    bars.push(DayBar {
                        date,
                        tokens: avg,
                        kind: BarKind::RateLimited,
                    });
                    continue;
                }
            }
            // Fallback: if rate-limit hit but no token data from hits, use history
            if let Some(list) = day_entries.get(&date) {
                let avg = list.iter().sum::<u64>() / list.len() as u64;
                bars.push(DayBar {
                    date,
                    tokens: avg,
                    kind: BarKind::RateLimited,
                });
            }
        } else if let Some(list) = day_entries.get(&date) {
            // Violet bar: average of all history entries for that day
            let avg = list.iter().sum::<u64>() / list.len() as u64;
            bars.push(DayBar {
                date,
                tokens: avg,
                kind: BarKind::History,
            });
        }
    }

    // Fill gaps: ensure every day from first to last has an entry
    if !bars.is_empty() {
        let first = bars[0].date;
        let last = bars.last().unwrap().date;
        let existing: std::collections::HashSet<NaiveDate> =
            bars.iter().map(|b| b.date).collect();
        let mut d = first;
        while d <= last {
            if !existing.contains(&d) {
                bars.push(DayBar {
                    date: d,
                    tokens: 0,
                    kind: BarKind::History,
                });
            }
            d += chrono::Duration::days(1);
        }
        bars.sort_by_key(|b| b.date);
    }

    if let Some(cred) = selected_cred {
        if let Some(usage) = &cred.usage {
            if let Some(five_h) = &usage.five_hour {
                if let Some(resets_at_str) = &five_h.resets_at
                    && let Ok(resets_at) = chrono::DateTime::parse_from_rfc3339(resets_at_str)
                {
                    let resets_utc = resets_at.with_timezone(&chrono::Utc);
                    let session_start_utc = resets_utc - chrono::Duration::hours(5);
                    let now_utc = chrono::Utc::now();
                    let start_local = session_start_utc
                        .with_timezone(&chrono::Local)
                        .naive_local();
                    let end_local = now_utc
                        .with_timezone(&chrono::Local)
                        .naive_local();
                    let tokens = index.tokens_in_window(root, start_local, end_local);
                    let utilization = five_h.utilization;
                    if tokens > 0 && utilization > 0.0 {
                        let estimated_tokens =
                            (tokens as f64 / (utilization / 100.0)).round() as u64;
                        if estimated_tokens > 0 {
                            let today = chrono::Local::now().date_naive();
                            bars.push(DayBar {
                                date: today,
                                tokens: estimated_tokens,
                                kind: BarKind::Current,
                            });
                        }
                    }
                }
            }
        }
    }

    bars
}

fn render_session_chart(frame: &mut Frame, area: Rect, bars: &[DayBar], tick: usize) {
    let t = theme();
    let (star, star_style) = super::theme::star_span(tick);

    let red = Color::Rgb(220, 60, 60);
    let blue = Color::Rgb(80, 170, 240);
    let violet = Color::Rgb(160, 100, 220);

    let title = Line::from(vec![
        Span::raw(" "),
        Span::styled(star, star_style),
        Span::styled(" Max usage graph ", Style::default().fg(t.heatmap_title).add_modifier(Modifier::BOLD)),
        Span::styled("██", Style::default().fg(violet)),
        Span::styled(" history ", Style::default().fg(t.text_dim)),
        Span::styled("██", Style::default().fg(red)),
        Span::styled(" hit ", Style::default().fg(t.text_dim)),
        Span::styled("██", Style::default().fg(blue)),
        Span::styled(" live ", Style::default().fg(t.text_dim)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(t.border))
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if bars.is_empty() || inner.height < 4 || inner.width < 4 {
        if inner.height >= 1 && inner.width >= 4 {
            let msg = Paragraph::new(Span::styled("no data", Style::default().fg(t.text_dim)));
            frame.render_widget(msg, inner);
        }
        return;
    }

    // Empty row between title and chart
    let inner = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..inner
    };

    // Reserve 1 row for date labels, 1 for scale label
    let chart_height = inner.height.saturating_sub(2) as usize;
    if chart_height == 0 {
        return;
    }

    let max_tokens = bars.iter().map(|b| b.tokens).max().unwrap_or(1).max(1);

    let n = bars.len();
    // Distribute full width evenly: each slot = total_width / n
    // Bar fills slot minus 1 char gap (minimum bar width = 1)
    let slot_w = (inner.width as usize / n).max(2);
    let bar_w = (slot_w - 1).max(1) as u16;
    let max_bars = inner.width as usize / slot_w;

    let visible = &bars[bars.len().saturating_sub(max_bars)..];
    let total_chart_w = visible.len() * slot_w;
    let x_offset = (inner.width as usize).saturating_sub(total_chart_w) / 2;
    let buf = frame.buffer_mut();

    for (i, bar) in visible.iter().enumerate() {
        let x = inner.x + x_offset as u16 + (i * slot_w) as u16;
        if x + bar_w > inner.x + inner.width {
            break;
        }

        let ratio = bar.tokens as f64 / max_tokens as f64;
        let bar_h = (ratio * chart_height as f64).round().max(if bar.tokens > 0 { 1.0 } else { 0.0 }) as usize;
        let color = match bar.kind {
            BarKind::Current => blue,
            BarKind::RateLimited => red,
            BarKind::History => violet,
        };

        // Draw bar from bottom to top
        for dy in 0..bar_h.min(chart_height) {
            let y = inner.y + chart_height as u16 - 1 - dy as u16;
            for dx in 0..bar_w {
                let cell = &mut buf[(x + dx, y)];
                cell.set_char(' ');
                cell.set_bg(color);
            }
        }

        // Date label (day number)
        let label_y = inner.y + chart_height as u16;
        let label = format!("{:>2}", bar.date.day());
        for (ci, ch) in label.chars().enumerate() {
            let lx = x + ci as u16;
            if lx < inner.x + inner.width && label_y < inner.y + inner.height {
                let cell = &mut buf[(lx, label_y)];
                cell.set_char(ch);
                cell.set_fg(t.text_dim);
            }
        }

        // Month label on first bar and when month changes
        let show_month = i == 0
            || (i > 0
                && visible[i].date.month() != visible[i - 1].date.month());
        if show_month {
            let month_y = inner.y + chart_height as u16 + 1;
            if month_y < inner.y + inner.height {
                let month_label = bar.date.format("%b").to_string();
                for (ci, ch) in month_label.chars().enumerate() {
                    let lx = x + ci as u16;
                    if lx < inner.x + inner.width {
                        let cell = &mut buf[(lx, month_y)];
                        cell.set_char(ch);
                        cell.set_fg(t.text_secondary);
                    }
                }
            }
        }
    }

    // Scale label (top-right): show max token value
    let scale_label = format_tokens(max_tokens);
    let scale_y = inner.y;
    let scale_x = inner.x + inner.width.saturating_sub(scale_label.len() as u16);
    for (ci, ch) in scale_label.chars().enumerate() {
        let lx = scale_x + ci as u16;
        if lx < inner.x + inner.width && scale_y < inner.y + inner.height {
            let cell = &mut buf[(lx, scale_y)];
            cell.set_char(ch);
            cell.set_fg(t.text_dim);
        }
    }

    let nonzero: Vec<(f64, f64)> = visible
        .iter()
        .enumerate()
        .filter(|(_, b)| b.tokens > 0 && b.kind != BarKind::Current)
        .map(|(i, b)| (i as f64, b.tokens as f64))
        .collect();

    if nonzero.len() >= 2 && chart_height >= 2 {
        let max_val = max_tokens as f64;
        let nn = nonzero.len() as f64;
        let sum_x: f64 = nonzero.iter().map(|p| p.0).sum();
        let sum_y: f64 = nonzero.iter().map(|p| p.1).sum();
        let sum_xy: f64 = nonzero.iter().map(|p| p.0 * p.1).sum();
        let sum_x2: f64 = nonzero.iter().map(|p| p.0 * p.0).sum();
        let denom = nn * sum_x2 - sum_x * sum_x;
        let (slope, intercept) = if denom.abs() > 1e-9 {
            let s = (nn * sum_xy - sum_x * sum_y) / denom;
            let i = (sum_y - s * sum_x) / nn;
            (s, i)
        } else {
            (0.0, sum_y / nn)
        };

        let (sr, sg, sb) = t.star_base;
        let trend_color = Color::Rgb(sr, sg, sb);
        let buf = frame.buffer_mut();
        let chart_w = total_chart_w;
        let vn = visible.len();

        // Braille: each cell is 2x4 dots, giving sub-character resolution
        // Row of dots: 4 vertical per cell, so effective Y resolution = chart_height * 4
        // Col of dots: 2 horizontal per cell, so effective X resolution = chart_w * 2
        let dot_rows = chart_height * 4;
        let dot_cols = chart_w * 2;

        // Braille base: U+2800, dots indexed as:
        //  0 3
        //  1 4
        //  2 5
        //  6 7
        const DOT_BITS: [u8; 8] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80];

        // Collect which braille dots to set per cell
        let mut braille_map: std::collections::HashMap<(u16, u16), u8> = std::collections::HashMap::new();

        for dx in 0..dot_cols {
            let data_x = if dot_cols > 1 {
                dx as f64 / (dot_cols - 1) as f64 * (vn - 1) as f64
            } else {
                0.0
            };
            let trend_val = slope * data_x + intercept;
            let ratio = trend_val / max_val;
            if ratio < 0.0 || ratio > 1.0 {
                continue;
            }
            let dy = ((1.0 - ratio) * (dot_rows - 1) as f64).round() as usize;

            let cell_x = dx / 2;
            let cell_y = dy / 4;
            let sub_x = dx % 2;
            let sub_y = dy % 4;
            // Braille dot order: col0=[0,1,2,6] col1=[3,4,5,7]
            let bit_idx = match (sub_x, sub_y) {
                (0, r @ 0..=2) => r,
                (0, 3) => 6,
                (1, r @ 0..=2) => r + 3,
                (1, 3) => 7,
                _ => unreachable!(),
            };

            let cx = inner.x + x_offset as u16 + cell_x as u16;
            let cy = inner.y + cell_y as u16;
            if cx < inner.x + inner.width && cy < inner.y + chart_height as u16 {
                *braille_map.entry((cx, cy)).or_default() |= DOT_BITS[bit_idx];
            }
        }

        for ((cx, cy), bits) in &braille_map {
            let ch = char::from_u32(0x2800 + *bits as u32).unwrap_or('·');
            let cell = &mut buf[(*cx, *cy)];
            cell.set_char(ch);
            cell.set_fg(trend_color);
        }
    }
}

