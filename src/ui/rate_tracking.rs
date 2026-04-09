use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate, TimeZone, Timelike};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use crate::data::index::EventIndex;
use crate::data::models::format_tokens;
use crate::data::oauth::{OAuthCredential, UsageReport, UsageStats, UsageWindow};
use crate::data::rate_history::RateHistory;
use crate::data::rate_limits::RateLimitHit;
use crate::data::tokens::MinuteTokens;

use super::theme::theme;

fn source_display_name<'a>(
    source_root: &'a str,
    source_names: &'a [String],
    source_roots: &[Option<String>],
) -> &'a str {
    for (i, root) in source_roots.iter().enumerate() {
        if let Some(r) = root
            && r == source_root
        {
            return &source_names[i];
        }
    }
    source_root
        .rsplit('/')
        .find(|s| !s.is_empty() && *s != "projects")
        .unwrap_or(source_root)
}

fn source_color_index(source_root: &str, all_roots: &[String]) -> usize {
    all_roots.iter().position(|r| r == source_root).unwrap_or(0)
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
            80.0 + t * 140.0, // 80 → 220
            200.0 + t * 0.0,  // 200 → 200
            80.0 - t * 40.0,  // 80 → 40
        )
    } else if position_ratio < 0.66 {
        let t = (position_ratio - 0.33) / 0.33;
        (
            220.0,            // 220
            200.0 - t * 60.0, // 200 → 140
            40.0,             // 40
        )
    } else {
        let t = (position_ratio - 0.66) / 0.34;
        (
            220.0,            // 220
            140.0 - t * 90.0, // 140 → 50
            40.0 + t * 10.0,  // 40 → 50
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
        return Line::from(Span::styled(
            label_text,
            Style::default().fg(t.text_primary),
        ));
    }

    let mut spans: Vec<Span<'a>> = Vec::with_capacity(bar_width + 2);
    spans.push(Span::styled(
        label_text,
        Style::default().fg(t.text_primary),
    ));

    let filled = (ratio * bar_width as f64) as usize;

    // Filled cells with gradient
    for i in 0..filled.min(bar_width) {
        let pos = if bar_width > 1 {
            i as f64 / (bar_width - 1) as f64
        } else {
            0.0
        };
        spans.push(Span::styled(" ", Style::default().bg(gradient_color(pos))));
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

#[allow(clippy::too_many_arguments)]
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
    reloading: bool,
    tick: usize,
) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    // Left: rate limits table (full height) | Right: cards (top) + chart (bottom)
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(34), Constraint::Min(40)])
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

    render_rate_limits(
        frame,
        columns[0],
        &filtered_hits,
        source_names,
        source_roots,
        &credential_roots,
    );

    // Right panel: cards + KPIs + forecast + usage timeline + max usage chart
    let right_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(count_card_height(credentials)),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(8),
            Constraint::Min(4),
        ])
        .split(columns[1]);

    render_credential_cards(
        frame,
        right_rows[0],
        credentials,
        source_names,
        source_roots,
        selected,
        &credential_roots,
    );

    let selected_cred = selected
        .filter(|&i| i < credentials.len())
        .map(|i| &credentials[i]);
    let bars = compute_session_bars(
        &filtered_hits,
        index,
        selected_root.as_deref(),
        selected_cred,
        rate_history,
    );
    render_kpi_bar(frame, right_rows[1], &bars, &filtered_hits);

    let selected_minute_tokens = index.build_minute_tokens(selected_root.as_deref(), None);
    render_session_forecast(
        frame,
        right_rows[2],
        selected_cred,
        &selected_minute_tokens,
        tick,
    );
    render_usage_timeline(
        frame,
        right_rows[3],
        &selected_minute_tokens,
        selected_cred,
        tick,
    );

    render_session_chart(frame, right_rows[4], &bars, tick);

    // Footer
    render_footer(frame, outer[1], reloading);
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

fn render_footer(frame: &mut Frame, area: Rect, reloading: bool) {
    let t = theme();
    if reloading {
        let footer = Paragraph::new(Span::styled("⟳ Reloading…", Style::default().fg(t.warning)))
            .alignment(Alignment::Center);
        frame.render_widget(footer, area);
    } else {
        let text = "←→ Select source   r Reload   ` Dashboard   q Quit";
        let footer = Paragraph::new(Span::styled(text, Style::default().fg(t.text_dim)))
            .alignment(Alignment::Center);
        frame.render_widget(footer, area);
    }
}

fn render_kpi_bar(frame: &mut Frame, area: Rect, bars: &[DayBar], hits: &[&RateLimitHit]) {
    let t = theme();

    // KPI 1: Avg tokens/session (from history bars, excluding current)
    let history_bars: Vec<&DayBar> = bars
        .iter()
        .filter(|b| b.kind != BarKind::Current && b.tokens > 0)
        .collect();
    let avg_tokens = if history_bars.is_empty() {
        0u64
    } else {
        let sum: u64 = history_bars.iter().map(|b| b.tokens).sum();
        sum / history_bars.len() as u64
    };
    let avg_str = format_tokens(avg_tokens);

    // KPI 2: Trend % (linear regression slope as % change over the period)
    let trend_str = if history_bars.len() >= 2 {
        let n = history_bars.len() as f64;
        let sum_x: f64 = (0..history_bars.len()).map(|i| i as f64).sum();
        let sum_y: f64 = history_bars.iter().map(|b| b.tokens as f64).sum();
        let sum_xy: f64 = history_bars
            .iter()
            .enumerate()
            .map(|(i, b)| i as f64 * b.tokens as f64)
            .sum();
        let sum_x2: f64 = (0..history_bars.len()).map(|i| (i * i) as f64).sum();
        let denom = n * sum_x2 - sum_x * sum_x;
        if denom.abs() > 1e-9 {
            let slope = (n * sum_xy - sum_x * sum_y) / denom;
            let mean_y = sum_y / n;
            if mean_y > 0.0 {
                let pct = (slope / mean_y) * 100.0;
                if pct >= 0.0 {
                    format!("↑{:.0}%", pct)
                } else {
                    format!("↓{:.0}%", pct.abs())
                }
            } else {
                "—".to_string()
            }
        } else {
            "—".to_string()
        }
    } else {
        "—".to_string()
    };
    let trend_color = if trend_str.starts_with('↑') {
        Color::Rgb(80, 200, 80) // green — positive trend
    } else if trend_str.starts_with('↓') {
        Color::Rgb(220, 60, 60) // red — negative trend
    } else {
        t.text_dim
    };

    // KPI 3: Rate-limit hits (total count)
    let hit_count = hits.len();
    let hits_str = format!("{}", hit_count);
    let hits_color = if hit_count == 0 {
        Color::Rgb(80, 200, 80)
    } else if hit_count <= 3 {
        Color::Rgb(220, 200, 40)
    } else {
        Color::Rgb(220, 60, 60)
    };

    let values: [(&str, &str, Color); 3] = [
        (&avg_str, " Avg tokens/session ", t.tokens_in),
        (&trend_str, " Trend ", trend_color),
        (&hits_str, " Rate-limit hits ", hits_color),
    ];

    let col_constraints: Vec<Constraint> = (0..3).map(|_| Constraint::Ratio(1, 3)).collect();
    let col_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(col_constraints)
        .split(area);

    for (i, col_area) in col_areas.iter().enumerate() {
        let (val, label, color) = &values[i];
        let block = Block::default()
            .title(Span::styled(*label, Style::default().fg(t.text_dim)))
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(t.border));
        let paragraph = Paragraph::new(Span::styled(
            *val,
            Style::default().fg(*color).add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center)
        .block(block);
        frame.render_widget(paragraph, *col_area);
    }
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
        render_card(
            frame,
            cols[i],
            cred,
            source_names,
            source_roots,
            credential_roots,
            is_selected,
        );
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

    let border_color = if is_selected {
        t.border_highlight
    } else {
        t.border
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .border_type(ratatui::widgets::BorderType::Rounded)
        .title(Span::styled(
            title_left,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));

    if !title_right.is_empty() {
        block = block.title_top(Line::from(title_right).alignment(Alignment::Right));
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);

    match &cred.usage {
        Some(usage) => render_compact_usage(frame, inner, usage, &cred.stats),
        None => {
            let line = Line::from(vec![
                Span::styled("loading...", Style::default().fg(t.text_dim)),
                Span::styled(
                    format!(
                        " ({}req, {}err)",
                        cred.stats.call_count, cred.stats.rate_limit_count
                    ),
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
    let Some(extra) = &u.extra_usage else {
        return vec![];
    };
    if !extra.is_enabled || extra.used_credits.is_none() {
        return vec![];
    }

    let used = extra.used_credits.unwrap_or(0.0) / 100.0;
    let limit = extra.monthly_limit.unwrap_or(0.0) / 100.0;
    let util = extra.utilization.unwrap_or(0.0);

    vec![Span::styled(
        format!("${:.2}/${:.2} ({:.0}%) ", used, limit, util),
        Style::default()
            .fg(util_color(util))
            .add_modifier(Modifier::BOLD),
    )]
}

fn render_compact_usage(frame: &mut Frame, area: Rect, usage: &UsageReport, stats: &UsageStats) {
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
        .filter_map(|(label, w)| {
            w.map(|w| {
                (
                    *label,
                    w.utilization,
                    w.resets_at.as_deref().map(format_reset).unwrap_or_default(),
                )
            })
        })
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
                    if h > 0 {
                        format!("{}h{:02}", h, m)
                    } else {
                        format!("{}m", m)
                    }
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
            Cell::from("Source")
                .style(Style::default().fg(t.text_dim).add_modifier(Modifier::BOLD)),
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
    input_tokens: u64,
    output_tokens: u64,
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

    let daily_io = index.daily_input_output_for_root(root);

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

    // Helper: split estimated total into (input, output) using the actual
    // ratio from the index for that day. Falls back to 50/50 if no data.
    let split_tokens = |date: NaiveDate, total: u64| -> (u64, u64) {
        if let Some(&(inp, out)) = daily_io.get(&date) {
            let actual_total = inp + out;
            if actual_total > 0 {
                let input_part = (total as f64 * inp as f64 / actual_total as f64).round() as u64;
                return (input_part, total.saturating_sub(input_part));
            }
        }
        let half = total / 2;
        (half, total - half)
    };

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
            if let Some(list) = tokens_list
                && !list.is_empty()
            {
                let avg = list.iter().sum::<u64>() / list.len() as u64;
                let (inp, out) = split_tokens(date, avg);
                bars.push(DayBar {
                    date,
                    tokens: avg,
                    input_tokens: inp,
                    output_tokens: out,
                    kind: BarKind::RateLimited,
                });
                continue;
            }
            // Fallback: if rate-limit hit but no token data from hits, use history
            if let Some(list) = day_entries.get(&date) {
                let avg = list.iter().sum::<u64>() / list.len() as u64;
                let (inp, out) = split_tokens(date, avg);
                bars.push(DayBar {
                    date,
                    tokens: avg,
                    input_tokens: inp,
                    output_tokens: out,
                    kind: BarKind::RateLimited,
                });
            }
        } else if let Some(list) = day_entries.get(&date) {
            // Violet bar: average of all history entries for that day
            let avg = list.iter().sum::<u64>() / list.len() as u64;
            let (inp, out) = split_tokens(date, avg);
            bars.push(DayBar {
                date,
                tokens: avg,
                input_tokens: inp,
                output_tokens: out,
                kind: BarKind::History,
            });
        }
    }

    // Fill gaps: ensure every day from first to last has an entry
    if !bars.is_empty() {
        let first = bars[0].date;
        let last = bars.last().unwrap().date;
        let existing: std::collections::HashSet<NaiveDate> = bars.iter().map(|b| b.date).collect();
        let mut d = first;
        while d <= last {
            if !existing.contains(&d) {
                bars.push(DayBar {
                    date: d,
                    tokens: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    kind: BarKind::History,
                });
            }
            d += chrono::Duration::days(1);
        }
        bars.sort_by_key(|b| b.date);
    }

    if let Some(cred) = selected_cred
        && let Some(usage) = &cred.usage
        && let Some(five_h) = &usage.five_hour
        && let Some(resets_at_str) = &five_h.resets_at
        && let Ok(resets_at) = chrono::DateTime::parse_from_rfc3339(resets_at_str)
    {
        let resets_utc = resets_at.with_timezone(&chrono::Utc);
        let session_start_utc = resets_utc - chrono::Duration::hours(5);
        let now_utc = chrono::Utc::now();

        let elapsed_min = (now_utc - session_start_utc).num_seconds().max(0) as f64 / 60.0;
        let utilization = five_h.utilization;

        if elapsed_min >= 30.0 || utilization >= 2.0 {
            let start_local = session_start_utc
                .with_timezone(&chrono::Local)
                .naive_local();
            let end_local = now_utc.with_timezone(&chrono::Local).naive_local();
            let (inp, out) = index.tokens_in_window_split(root, start_local, end_local);
            let tokens = inp + out;
            if tokens > 0 {
                let scale = 1.0 / (utilization / 100.0);
                let estimated_tokens = (tokens as f64 * scale).round() as u64;
                let estimated_in = (inp as f64 * scale).round() as u64;
                let estimated_out = estimated_tokens.saturating_sub(estimated_in);
                if estimated_tokens > 0 {
                    let today = chrono::Local::now().date_naive();
                    bars.push(DayBar {
                        date: today,
                        tokens: estimated_tokens,
                        input_tokens: estimated_in,
                        output_tokens: estimated_out,
                        kind: BarKind::Current,
                    });
                }
            }
        }
    }

    bars
}

fn render_session_forecast(
    frame: &mut Frame,
    area: Rect,
    selected_cred: Option<&OAuthCredential>,
    minute_tokens: &MinuteTokens,
    tick: usize,
) {
    let t = theme();
    let (star, star_style) = super::theme::star_span(tick);

    let title = Line::from(vec![
        Span::raw(" "),
        Span::styled(star, star_style),
        Span::styled(
            " Session forecast ",
            Style::default()
                .fg(t.heatmap_title)
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(t.border))
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 || inner.width < 20 {
        return;
    }

    let cred = match selected_cred {
        Some(c) => c,
        None => {
            let msg = Paragraph::new(Span::styled(
                "no source selected",
                Style::default().fg(t.text_dim),
            ));
            frame.render_widget(msg, inner);
            return;
        }
    };

    let usage = match &cred.usage {
        Some(u) => u,
        None => {
            let msg = Paragraph::new(Span::styled(
                "waiting for usage data…",
                Style::default().fg(t.text_dim),
            ));
            frame.render_widget(msg, inner);
            return;
        }
    };

    let five_h = match &usage.five_hour {
        Some(fh) => fh,
        None => return,
    };

    let utilization = five_h.utilization / 100.0; // 0.0 .. 1.0

    // Parse session timing
    let now_local = chrono::Local::now();
    let (session_start_local, session_end_local) = if let Some(resets_at_str) = &five_h.resets_at
        && let Ok(resets_at) = chrono::DateTime::parse_from_rfc3339(resets_at_str)
    {
        let end = resets_at.with_timezone(&chrono::Local);
        let start = end - chrono::Duration::hours(5);
        (start.naive_local(), end.naive_local())
    } else {
        // Fallback: assume 5h window ending 5h from now
        let end = now_local.naive_local() + chrono::Duration::hours(5);
        let start = now_local.naive_local();
        (start, end)
    };

    let total_window_min = 300.0f64; // 5h in minutes
    let elapsed_min = (now_local.naive_local() - session_start_local)
        .num_seconds()
        .max(0) as f64
        / 60.0;
    let remaining_min = (session_end_local - now_local.naive_local())
        .num_seconds()
        .max(0) as f64
        / 60.0;

    // Compute recent token rate (last 30 min or session duration, whichever is shorter)
    let today = now_local.date_naive();
    let now_minute = now_local.hour() as u16 * 60 + now_local.minute() as u16;
    let sample_window = (elapsed_min as u16).clamp(1, 30);
    let sample_start = now_minute.saturating_sub(sample_window);

    let mut recent_tokens: u64 = 0;
    for (&(date, minute), &val) in minute_tokens
        .input
        .iter()
        .chain(minute_tokens.output.iter())
    {
        if (date == today && minute >= sample_start && minute <= now_minute)
            || (date == today.pred_opt().unwrap_or(today)
                && sample_start > now_minute
                && minute >= sample_start)
        {
            recent_tokens += val;
        }
    }

    let rate_per_min = if sample_window > 0 {
        recent_tokens as f64 / sample_window as f64
    } else {
        0.0
    };

    // Estimate time to hit limit
    // If utilization is U and we've been going for E minutes,
    // the "safe" rate = (1.0 - U) / remaining_min (proportional)
    // But we compare actual rate vs what would deplete the remaining budget.
    // Remaining budget fraction = 1.0 - utilization
    // At current rate_per_min, total_tokens_per_min = rate_per_min
    // We need: rate over session → how much util per minute = utilization / elapsed_min
    // Better: use rate ratio. If current rate continues, minutes until 100%:
    let util_per_min = if elapsed_min > 1.0 {
        utilization / elapsed_min
    } else {
        utilization
    };

    let minutes_to_limit = if util_per_min > 0.0 {
        (1.0 - utilization) / util_per_min
    } else {
        f64::INFINITY
    };

    let limit_time = if minutes_to_limit.is_finite() {
        now_local + chrono::Duration::seconds((minutes_to_limit * 60.0) as i64)
    } else {
        now_local
    };

    // Determine status based on ratio of current rate vs safe rate
    // Safe rate = rate that would exactly finish at the end of window
    let safe_util_per_min = if remaining_min > 0.0 {
        (1.0 - utilization) / remaining_min
    } else {
        0.0
    };

    let rate_ratio = if safe_util_per_min > 0.0 {
        util_per_min / safe_util_per_min
    } else if util_per_min > 0.0 {
        f64::INFINITY
    } else {
        0.0
    };

    let (status_label, status_color) = if rate_ratio < 0.70 {
        ("Steady", t.lines_positive)
    } else if rate_ratio < 0.85 {
        ("Watch", t.warning)
    } else if rate_ratio < 0.95 {
        ("Slow down", t.efficiency_accent)
    } else {
        ("Critical", t.error)
    };

    // Format rate
    let rate_label = if rate_per_min >= 1000.0 {
        format!("{:.1}K/m", rate_per_min / 1000.0)
    } else {
        format!("{:.0}/m", rate_per_min)
    };

    // Format time remaining
    let time_remaining_label = if minutes_to_limit.is_infinite() || minutes_to_limit > 600.0 {
        "—".to_string()
    } else {
        let h = minutes_to_limit as u64 / 60;
        let m = minutes_to_limit as u64 % 60;
        if h > 0 {
            format!("{}h{:02}m", h, m)
        } else {
            format!("{}m", m)
        }
    };

    let limit_time_label = if minutes_to_limit.is_infinite() || minutes_to_limit > 600.0 {
        "—".to_string()
    } else {
        format!("{}", limit_time.format("%H:%M"))
    };

    // Row 0: progress bar
    // Session start and end times on left/right, bar in between
    let start_str = format!("{}", session_start_local.format("%H:%M"));
    let end_str = format!("{}", session_end_local.format("%H:%M"));
    let bar_width = inner
        .width
        .saturating_sub(start_str.len() as u16 + end_str.len() as u16 + 2)
        as usize;

    let now_pos = (elapsed_min / total_window_min).clamp(0.0, 1.0);
    let filled = (now_pos * bar_width as f64) as usize;

    let mut bar_spans: Vec<Span> = Vec::new();
    bar_spans.push(Span::styled(
        format!("{} ", start_str),
        Style::default().fg(t.text_dim),
    ));
    for i in 0..bar_width {
        if i < filled {
            bar_spans.push(Span::styled(
                "█",
                Style::default().fg(gradient_color(utilization)),
            ));
        } else {
            bar_spans.push(Span::styled("░", Style::default().fg(t.empty_bar)));
        }
    }
    bar_spans.push(Span::styled(
        format!(" {}", end_str),
        Style::default().fg(t.text_dim),
    ));

    frame.render_widget(
        Paragraph::new(Line::from(bar_spans)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    // Row 1: ↑now marker positioned under the bar
    let marker_col = (start_str.len() + 1) + filled;
    let marker_col = marker_col.min(inner.width as usize - 4);
    let mut marker_spans: Vec<Span> = Vec::new();
    if marker_col > 0 {
        marker_spans.push(Span::raw(" ".repeat(marker_col)));
    }
    marker_spans.push(Span::styled("↑now", Style::default().fg(t.text_secondary)));
    frame.render_widget(
        Paragraph::new(Line::from(marker_spans)),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    // Row 2: status indicator + time remaining + rate
    let mut status_spans: Vec<Span> = Vec::new();
    // Blink the dot for critical status
    let dot_visible = if rate_ratio >= 0.95 {
        (tick / 3).is_multiple_of(2)
    } else {
        true
    };
    if dot_visible {
        status_spans.push(Span::styled("● ", Style::default().fg(status_color)));
    } else {
        status_spans.push(Span::raw("  "));
    }
    status_spans.push(Span::styled(
        status_label,
        Style::default()
            .fg(status_color)
            .add_modifier(Modifier::BOLD),
    ));
    status_spans.push(Span::styled(
        format!(" — {}", time_remaining_label),
        Style::default().fg(t.text_primary),
    ));

    // Right-align: rate + limit time
    let left_part = format!("● {} — {}", status_label, time_remaining_label);
    let right_part = format!("{} → {}", rate_label, limit_time_label);
    let padding = (inner.width as usize).saturating_sub(left_part.len() + right_part.len());
    if padding > 0 {
        status_spans.push(Span::raw(" ".repeat(padding)));
    }
    status_spans.push(Span::styled(
        rate_label.clone(),
        Style::default().fg(t.text_dim),
    ));
    status_spans.push(Span::styled(" → ", Style::default().fg(t.text_dim)));
    status_spans.push(Span::styled(
        limit_time_label,
        Style::default().fg(status_color),
    ));

    frame.render_widget(
        Paragraph::new(Line::from(status_spans)),
        Rect::new(inner.x, inner.y + 2, inner.width, 1),
    );
}

fn render_usage_timeline(
    frame: &mut Frame,
    area: Rect,
    minute_tokens: &MinuteTokens,
    selected_cred: Option<&OAuthCredential>,
    _tick: usize,
) {
    let t = theme();
    let dot_color = t.tokens_in;

    let title = Line::from(vec![Span::styled(
        " Session tokens ",
        Style::default()
            .fg(t.heatmap_title)
            .add_modifier(Modifier::BOLD),
    )]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(t.border))
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 2 || inner.width < 8 {
        return;
    }

    // Determine session window from the selected credential's five_hour.resets_at
    let now_local = chrono::Local::now();
    let (session_date, start_minute, now_minute) = if let Some(cred) = selected_cred
        && let Some(usage) = &cred.usage
        && let Some(five_h) = &usage.five_hour
        && let Some(resets_at_str) = &five_h.resets_at
        && let Ok(resets_at) = chrono::DateTime::parse_from_rfc3339(resets_at_str)
    {
        let session_start =
            (resets_at.with_timezone(&chrono::Local) - chrono::Duration::hours(5)).naive_local();
        let date = session_start.date();
        let sm = session_start.hour() as u16 * 60 + session_start.minute() as u16;
        let nm = if date == now_local.date_naive() {
            now_local.hour() as u16 * 60 + now_local.minute() as u16
        } else {
            // Session started yesterday — clamp to today
            now_local.hour() as u16 * 60 + now_local.minute() as u16
        };
        (date, sm, nm)
    } else {
        // Fallback: last 4 hours
        let nm = now_local.hour() as u16 * 60 + now_local.minute() as u16;
        let sm = nm.saturating_sub(nm.min(240));
        (now_local.date_naive(), sm, nm)
    };

    // Handle sessions that may span midnight
    let today = now_local.date_naive();
    let spans_midnight = session_date < today;
    let total_minutes = if spans_midnight {
        (1440 - start_minute) + now_minute
    } else {
        now_minute.saturating_sub(start_minute)
    };

    if total_minutes == 0 {
        return;
    }

    let bucket_min: u16 = 2;
    let n_buckets = total_minutes.div_ceil(bucket_min).max(1) as usize;

    let mut buckets = vec![0u64; n_buckets];
    for (&(date, minute), &val) in minute_tokens
        .input
        .iter()
        .chain(minute_tokens.output.iter())
    {
        let offset = if spans_midnight {
            if date == session_date && minute >= start_minute {
                Some((minute - start_minute) as u32)
            } else if date == today && minute <= now_minute {
                Some((1440 - start_minute) as u32 + minute as u32)
            } else {
                None
            }
        } else {
            if date == today && minute >= start_minute && minute <= now_minute {
                Some((minute - start_minute) as u32)
            } else {
                None
            }
        };
        if let Some(off) = offset {
            let idx = (off / bucket_min as u32) as usize;
            if idx < n_buckets {
                buckets[idx] += val;
            }
        }
    }

    let buckets = &buckets[..n_buckets];

    // Scale label (top-right)
    let max_val = buckets.iter().cloned().max().unwrap_or(0).max(1);
    let scale_label = format_tokens(max_val);
    let scale_span = Span::styled(&scale_label, Style::default().fg(t.tokens_in));
    let scale_area = Rect::new(
        inner.x + inner.width.saturating_sub(scale_label.len() as u16),
        inner.y,
        scale_label.len() as u16,
        1,
    );
    frame.render_widget(Paragraph::new(scale_span), scale_area);

    let chart_h = inner.height.saturating_sub(1) as usize; // 1 row for x-axis
    let chart_w = inner.width as usize;
    if chart_w < 2 || chart_h < 1 {
        return;
    }

    // Always resample to exactly fill the available width (dot-columns = chart_w * 2)
    let dot_cols = chart_w * 2;
    let dot_rows = chart_h * 4;

    let data: Vec<f64> = {
        let ratio = n_buckets as f64 / dot_cols as f64;
        (0..dot_cols)
            .map(|i| {
                let center = (i as f64 + 0.5) * ratio;
                let lo = (center - ratio / 2.0).max(0.0).floor() as usize;
                let hi = (center + ratio / 2.0).ceil() as usize;
                let hi = hi.min(n_buckets);
                if lo >= hi {
                    let idx = center.round() as usize;
                    if idx < n_buckets {
                        buckets[idx] as f64
                    } else {
                        0.0
                    }
                } else {
                    let sum: u64 = buckets[lo..hi].iter().sum();
                    sum as f64 / (hi - lo) as f64
                }
            })
            .collect()
    };
    let n_points = data.len();
    let max_f = max_val as f64;

    // Braille line rendering — interpolate between consecutive points so the
    // curve is continuous, just like the dashboard sparkline.
    const DOT_BITS: [u8; 8] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80];
    let mut braille_map: std::collections::HashMap<(u16, u16), u8> =
        std::collections::HashMap::new();

    let set_dot = |map: &mut std::collections::HashMap<(u16, u16), u8>, dx: usize, dy: usize| {
        let cell_x = dx / 2;
        let cell_y = dy / 4;
        let sub_x = dx % 2;
        let sub_y = dy % 4;
        let bit_idx = match (sub_x, sub_y) {
            (0, r @ 0..=2) => r,
            (0, 3) => 6,
            (1, r @ 0..=2) => r + 3,
            (1, 3) => 7,
            _ => unreachable!(),
        };
        let cx = inner.x + cell_x as u16;
        let cy = inner.y + cell_y as u16;
        if cx < inner.x + inner.width && cy < inner.y + chart_h as u16 {
            *map.entry((cx, cy)).or_default() |= DOT_BITS[bit_idx];
        }
    };

    // Convert data values to dot-row positions
    let y_positions: Vec<usize> = data
        .iter()
        .map(|&v| {
            let ratio = v / max_f;
            ((1.0 - ratio) * (dot_rows - 1) as f64).round() as usize
        })
        .collect();

    for dx in 0..n_points {
        let dy = y_positions[dx];
        set_dot(&mut braille_map, dx, dy);

        // Interpolate vertically between this point and the next
        if dx + 1 < n_points {
            let dy_next = y_positions[dx + 1];
            let (lo, hi) = if dy < dy_next {
                (dy, dy_next)
            } else {
                (dy_next, dy)
            };
            for y in lo..=hi {
                set_dot(&mut braille_map, dx, y);
            }
        }
    }

    let buf = frame.buffer_mut();
    for ((cx, cy), bits) in &braille_map {
        let ch = char::from_u32(0x2800 + *bits as u32).unwrap_or('·');
        let cell = &mut buf[(*cx, *cy)];
        cell.set_char(ch);
        cell.set_fg(dot_color);
    }

    // X-axis: time labels
    let x_row = inner.y + inner.height - 1;
    let label_count = (chart_w / 12).max(2);
    for li in 0..label_count {
        let col = if label_count > 1 {
            li * (chart_w - 1) / (label_count - 1)
        } else {
            0
        };
        let offset_min = (col as f64 / chart_w as f64 * total_minutes as f64) as u16;
        let abs_minute = (start_minute + offset_min) % 1440;
        let label = format!("{:02}:{:02}", abs_minute / 60, abs_minute % 60);
        // Shift last label left so it doesn't get clipped
        let label_len = label.len() as u16;
        let lx = if inner.x + col as u16 + label_len > inner.x + inner.width {
            (inner.x + inner.width).saturating_sub(label_len)
        } else {
            inner.x + col as u16
        };
        for (ci, ch) in label.chars().enumerate() {
            let x = lx + ci as u16;
            if x < inner.x + inner.width && x_row < inner.y + inner.height {
                let cell = &mut buf[(x, x_row)];
                cell.set_char(ch);
                cell.set_fg(t.text_dim);
            }
        }
    }
}

fn render_session_chart(frame: &mut Frame, area: Rect, bars: &[DayBar], _tick: usize) {
    let t = theme();
    // Base colors per bar kind
    let red = Color::Rgb(220, 60, 60);
    let blue = Color::Rgb(80, 170, 240);
    let violet = Color::Rgb(160, 100, 220);
    // Lighter variants for input portion (high contrast)
    let red_light = Color::Rgb(255, 150, 130);
    let blue_light = Color::Rgb(170, 220, 255);
    let violet_light = Color::Rgb(220, 180, 255);

    let title = Line::from(vec![
        Span::styled(
            " Max usage graph ",
            Style::default()
                .fg(t.heatmap_title)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("██", Style::default().fg(violet)),
        Span::styled(" history ", Style::default().fg(t.text_dim)),
        Span::styled("██", Style::default().fg(red)),
        Span::styled(" hit ", Style::default().fg(t.text_dim)),
        Span::styled("██", Style::default().fg(blue)),
        Span::styled(" live ", Style::default().fg(t.text_dim)),
        Span::styled("│ ", Style::default().fg(t.border)),
        Span::styled("█", Style::default().fg(violet_light)),
        Span::styled("█", Style::default().fg(violet)),
        Span::styled(" in/out ", Style::default().fg(t.text_dim)),
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
        let bar_h = (ratio * chart_height as f64)
            .round()
            .max(if bar.tokens > 0 { 1.0 } else { 0.0 }) as usize;

        // Colors: darker shade = output (bottom), lighter shade = input (top)
        let (color_output, color_input) = match bar.kind {
            BarKind::Current => (blue, blue_light),
            BarKind::RateLimited => (red, red_light),
            BarKind::History => (violet, violet_light),
        };

        // Stacked bar: output on bottom, input on top
        // Guarantee at least 1 row for each non-zero portion
        let out_h = if bar.tokens > 0 && bar_h >= 2 && bar.output_tokens > 0 && bar.input_tokens > 0
        {
            let out_ratio = bar.output_tokens as f64 / bar.tokens as f64;
            let raw = (out_ratio * bar_h as f64).round() as usize;
            raw.clamp(1, bar_h - 1)
        } else if bar.tokens > 0 && bar.input_tokens == 0 {
            bar_h
        } else {
            0
        };

        for dy in 0..bar_h.min(chart_height) {
            let y = inner.y + chart_height as u16 - 1 - dy as u16;
            let c = if dy < out_h {
                color_output
            } else {
                color_input
            };
            for dx in 0..bar_w {
                let cell = &mut buf[(x + dx, y)];
                cell.set_char(' ');
                cell.set_bg(c);
            }
        }

        // Date label (day number) — skip labels when bars are too narrow
        // A day label is 2 chars wide; show every Nth label so they don't overlap
        let label_step = if slot_w >= 3 {
            1
        } else {
            3_usize.div_ceil(slot_w)
        };
        let label_y = inner.y + chart_height as u16;
        if i % label_step == 0 {
            let label = format!("{:>2}", bar.date.day());
            for (ci, ch) in label.chars().enumerate() {
                let lx = x + ci as u16;
                if lx < inner.x + inner.width && label_y < inner.y + inner.height {
                    let cell = &mut buf[(lx, label_y)];
                    cell.set_char(ch);
                    cell.set_fg(t.text_dim);
                }
            }
        }

        // Month label on first bar and when month changes
        let show_month =
            i == 0 || (i > 0 && visible[i].date.month() != visible[i - 1].date.month());
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
        let mut braille_map: std::collections::HashMap<(u16, u16), u8> =
            std::collections::HashMap::new();

        for dx in 0..dot_cols {
            let data_x = if dot_cols > 1 {
                dx as f64 / (dot_cols - 1) as f64 * (vn - 1) as f64
            } else {
                0.0
            };
            let trend_val = slope * data_x + intercept;
            let ratio = trend_val / max_val;
            if !(0.0..=1.0).contains(&ratio) {
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
