use super::data::ProjectCard;

use std::collections::HashMap;

use chrono::{NaiveDate, Timelike};
use ratatui::{
    prelude::*,
    widgets::{Block, BorderType, Borders, Paragraph},
};

use crate::ui::theme::theme;

const CARD_HEIGHT: u16 = 6;

/// Cost per (project, model) bucketed by (date, minute of day).
pub type MinuteModelCosts = HashMap<(String, String), HashMap<(NaiveDate, u16), f64>>;

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Quartile boundaries for efficiency gauges, computed from visible cards.
struct EffQuartiles {
    p25: f64,
    p50: f64,
    p75: f64,
}

impl EffQuartiles {
    fn from_cards(cards: &[ProjectCard]) -> Self {
        let mut vals: Vec<f64> = cards
            .iter()
            .map(|c| c.efficiency)
            .filter(|&e| e > 0.0)
            .collect();
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        if vals.is_empty() {
            return Self {
                p25: 500.0,
                p50: 1000.0,
                p75: 1500.0,
            };
        }
        let percentile = |p: f64| -> f64 {
            let idx = p * (vals.len() - 1) as f64;
            let lo = idx.floor() as usize;
            let hi = (lo + 1).min(vals.len() - 1);
            let frac = idx - lo as f64;
            vals[lo] * (1.0 - frac) + vals[hi] * frac
        };
        Self {
            p25: percentile(0.25),
            p50: percentile(0.50),
            p75: percentile(0.75),
        }
    }
}

/// Mini gauge for efficiency: 4 chars wide, colored green→yellow→red.
/// Filled bars and color are based on quartile position among visible cards.
fn efficiency_gauge(eff: f64, q: &EffQuartiles) -> (Span<'static>, Span<'static>) {
    const WIDTH: usize = 4;
    let (filled, t) = if eff <= q.p25 {
        (4, 0.0)
    } else if eff <= q.p50 {
        (3, 0.33)
    } else if eff <= q.p75 {
        (2, 0.66)
    } else {
        (1, 1.0)
    };

    let color = if t < 0.5 {
        let p = t * 2.0;
        let r = (60.0 + 195.0 * p) as u8;
        let g = (210.0 - 10.0 * p) as u8;
        Color::Rgb(r, g, 50)
    } else {
        let p = (t - 0.5) * 2.0;
        Color::Rgb(255, (200.0 - 140.0 * p) as u8, (50.0 - 30.0 * p) as u8)
    };

    let filled_str: String = "█".repeat(filled);
    let empty_str: String = "░".repeat(WIDTH - filled);
    (
        Span::styled(filled_str, Style::default().fg(color)),
        Span::styled(empty_str, Style::default().fg(theme().empty_bar)),
    )
}

fn format_tokens(n: u64) -> String {
    crate::data::models::format_tokens(n)
}

fn format_duration(minutes: u64) -> String {
    if minutes >= 60 {
        format!("{}h{:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{}m", minutes)
    }
}

fn format_cost(cost: f64) -> String {
    if cost >= 100.0 {
        format!("${:.0}", cost)
    } else if cost >= 10.0 {
        format!("${:.1}", cost)
    } else {
        format!("${:.2}", cost)
    }
}

/// Render sparkline with model colors overlaid.
fn render_sparkline_with_models(
    daily_costs: &[(NaiveDate, f64)],
    model_shares: &[(String, f64)],
    width: usize,
    range_start: NaiveDate,
    range_end: NaiveDate,
) -> Line<'static> {
    if width == 0 {
        return Line::from("");
    }

    static BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    let total_days = (range_end - range_start).num_days().max(0) as usize + 1;
    let full_values: Vec<f64> = (0..total_days)
        .map(|i| {
            let date = range_start + chrono::Duration::days(i as i64);
            daily_costs
                .binary_search_by_key(&date, |(d, _)| *d)
                .ok()
                .map(|idx| daily_costs[idx].1)
                .unwrap_or(0.0)
        })
        .collect();

    let values: Vec<f64> = if full_values.len() <= width {
        full_values
    } else {
        let bucket_size = full_values.len() as f64 / width as f64;
        (0..width)
            .map(|i| {
                let start = (i as f64 * bucket_size) as usize;
                let end = (((i + 1) as f64 * bucket_size) as usize).min(full_values.len());
                full_values[start..end].iter().sum::<f64>()
            })
            .collect()
    };

    let scaled_values: Vec<f64> = values.iter().map(|&v| v.cbrt()).collect();
    let max = scaled_values.iter().cloned().fold(0.0f64, f64::max);

    let model_colors: Vec<Color> = build_model_color_map(model_shares, values.len());

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_color = Color::Reset;
    let mut current_run = String::new();

    for (i, (&v, &sv)) in values.iter().zip(scaled_values.iter()).enumerate() {
        let ch = if max <= 0.0 || v <= 0.0 {
            ' '
        } else {
            let idx = ((sv / max) * 7.0).round() as usize;
            BLOCKS[idx.clamp(1, 7)]
        };

        let color = if i < model_colors.len() {
            model_colors[i]
        } else {
            theme().model_other
        };

        if color == current_color {
            current_run.push(ch);
        } else {
            if !current_run.is_empty() {
                spans.push(Span::styled(
                    current_run.clone(),
                    Style::default().fg(current_color),
                ));
            }
            current_run = String::from(ch);
            current_color = color;
        }
    }
    if !current_run.is_empty() {
        spans.push(Span::styled(
            current_run,
            Style::default().fg(current_color),
        ));
    }

    Line::from(spans)
}

fn build_model_color_map(model_shares: &[(String, f64)], width: usize) -> Vec<Color> {
    let t = theme();
    if model_shares.is_empty() || width == 0 {
        return vec![t.model_other; width];
    }

    let mut colors = Vec::with_capacity(width);
    let mut used = 0usize;

    for (i, (model, share)) in model_shares.iter().enumerate() {
        let segment_width = if i == model_shares.len() - 1 {
            width - used
        } else {
            ((share * width as f64).round() as usize)
                .max(1)
                .min(width - used)
        };

        let color = t.model_color(model);
        for _ in 0..segment_width {
            colors.push(color);
        }
        used += segment_width;
    }

    colors
}

/// Render project cards into the given area.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    cards: &[ProjectCard],
    tick: usize,
    range_start: NaiveDate,
    range_end: NaiveDate,
    scroll_offset: usize,
) {
    if cards.is_empty() || area.height < CARD_HEIGHT + 1 || area.width < 20 {
        return;
    }

    let t = theme();

    // Model legend at top (1 line)
    let legend_spans: Vec<Span> = vec![
        Span::styled("■", Style::default().fg(t.model_color("opus"))),
        Span::styled(" opus ", Style::default().fg(t.text_dim)),
        Span::styled("■", Style::default().fg(t.model_color("sonnet"))),
        Span::styled(" sonnet ", Style::default().fg(t.text_dim)),
        Span::styled("■", Style::default().fg(t.model_color("haiku"))),
        Span::styled(" haiku", Style::default().fg(t.text_dim)),
    ];
    let legend = Paragraph::new(Line::from(legend_spans)).alignment(Alignment::Right);
    frame.render_widget(legend, Rect::new(area.x, area.y, area.width, 1));

    let cards_area = Rect::new(area.x, area.y + 1, area.width, area.height - 1);

    let cols = if cards_area.width >= 120 {
        3
    } else if cards_area.width >= 80 {
        2
    } else {
        1
    };

    let card_width = cards_area.width / cols;
    let visible_rows = cards_area.height / CARD_HEIGHT;
    let total_rows = (cards.len() as u16).div_ceil(cols);
    let max_scroll = total_rows.saturating_sub(visible_rows) as usize;
    let scroll = scroll_offset.min(max_scroll);

    let start = scroll as u16 * cols;
    let max_visible = (cols * visible_rows) as usize;
    let end = cards.len().min((start as usize) + max_visible);

    let quartiles = EffQuartiles::from_cards(cards);

    for (i, card) in cards[start as usize..end].iter().enumerate() {
        let col = (i as u16) % cols;
        let row = (i as u16) / cols;
        let x = cards_area.x + col * card_width;
        let y = cards_area.y + row * CARD_HEIGHT;

        if y + CARD_HEIGHT > cards_area.y + cards_area.height {
            break;
        }

        let w = if col == cols - 1 {
            cards_area.width - col * card_width
        } else {
            card_width
        };

        let card_area = Rect::new(x, y, w, CARD_HEIGHT);
        render_card(
            frame,
            card_area,
            card,
            tick,
            range_start,
            range_end,
            &quartiles,
        );
    }

    // Scroll indicator
    if total_rows > visible_rows {
        let indicator = format!(" {}/{} ", scroll + 1, max_scroll + 1);
        let indicator_widget =
            Paragraph::new(Span::styled(indicator, Style::default().fg(t.text_dim)))
                .alignment(Alignment::Left);
        frame.render_widget(indicator_widget, Rect::new(area.x, area.y, area.width, 1));
    }
}

fn rainbow_title(name: &str, tick: usize) -> Line<'static> {
    let t = theme();
    let mut spans = vec![
        Span::raw(" "),
        Span::styled("★ ", Style::default().fg(t.cost)),
    ];
    for (i, ch) in name.chars().enumerate() {
        let color_idx = (i + tick) % t.rainbow.len();
        spans.push(Span::styled(
            ch.to_string(),
            Style::default()
                .fg(t.rainbow[color_idx])
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn render_card(
    frame: &mut Frame,
    area: Rect,
    card: &ProjectCard,
    tick: usize,
    range_start: NaiveDate,
    range_end: NaiveDate,
    quartiles: &EffQuartiles,
) {
    let t = theme();

    let border_color = if card.is_starred {
        t.border_highlight
    } else {
        t.border
    };

    let title_line = if card.is_starred {
        rainbow_title(&card.name, tick)
    } else {
        Line::from(Span::styled(
            format!(" {} ", card.name),
            Style::default().fg(t.text_secondary),
        ))
    };

    let block = Block::default()
        .title(title_line)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 4 || inner.width < 10 {
        return;
    }

    let content_width = inner.width as usize;

    // Line 1: cost + time + efficiency with mini gauge
    let cost_str = format_cost(card.total_cost);
    let time_str = if card.time_minutes > 0 {
        format!("⏱ {}", format_duration(card.time_minutes))
    } else {
        String::new()
    };
    let eff_spans: Vec<Span> = if card.efficiency > 0.0 {
        let gauge = efficiency_gauge(card.efficiency, quartiles);
        let num_str = format!(" {:.0} tok/ln", card.efficiency);
        let label = "⚡ ";
        let time_extra = if time_str.is_empty() {
            0
        } else {
            time_str.len() + 2
        };
        let total_len = label.len() + 4 + num_str.len();
        let padding = content_width.saturating_sub(cost_str.len() + time_extra + total_len);
        vec![
            Span::raw(" ".repeat(padding)),
            Span::styled(label, Style::default().fg(t.text_secondary)),
            gauge.0,
            gauge.1,
            Span::styled(num_str, Style::default().fg(t.text_secondary)),
        ]
    } else {
        let time_extra = if time_str.is_empty() {
            0
        } else {
            time_str.len() + 2
        };
        let padding = content_width.saturating_sub(cost_str.len() + time_extra);
        vec![Span::raw(" ".repeat(padding))]
    };
    let mut line1_spans = vec![Span::styled(
        &cost_str,
        Style::default().fg(t.cost).add_modifier(Modifier::BOLD),
    )];
    if !time_str.is_empty() {
        line1_spans.push(Span::raw("  "));
        line1_spans.push(Span::styled(&time_str, Style::default().fg(t.duration)));
    }
    line1_spans.extend(eff_spans);
    let line1 = Line::from(line1_spans);

    // Line 2: tokens breakdown
    let in_str = format!("in: {}", format_tokens(card.tokens_in));
    let out_str = format!("out: {}", format_tokens(card.tokens_out));
    let cache_str = format!("cache: {}", format_tokens(card.tokens_cache));
    let line2 = Line::from(vec![
        Span::styled(&in_str, Style::default().fg(t.tokens_in)),
        Span::raw(" "),
        Span::styled(&out_str, Style::default().fg(t.tokens_out)),
        Span::raw(" "),
        Span::styled(&cache_str, Style::default().fg(t.cache)),
    ]);

    // Line 3: lines added / deleted
    let added_str = format!("+{}", card.lines_added);
    let deleted_str = format!("-{}", card.lines_deleted);
    let line3 = Line::from(vec![
        Span::styled(added_str, Style::default().fg(t.lines_positive)),
        Span::raw(" "),
        Span::styled(deleted_str, Style::default().fg(t.lines_negative)),
    ]);

    // Line 4: sparkline colored by model segments
    let line4 = render_sparkline_with_models(
        &card.daily_costs,
        &card.model_shares,
        content_width,
        range_start,
        range_end,
    );

    let text = vec![line1, line2, line3, line4];
    let paragraph = Paragraph::new(text);
    frame.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Detail view helpers
// ---------------------------------------------------------------------------

fn format_y_cost(v: f64) -> String {
    if v >= 100.0 {
        format!("${:.0}", v)
    } else if v >= 10.0 {
        format!("${:.1}", v)
    } else {
        format!("${:.2}", v)
    }
}

fn format_y_tokens(v: f64) -> String {
    use crate::data::models::TOKENS_PER_MILLION;
    if v >= TOKENS_PER_MILLION {
        format!("{:.1}M", v / TOKENS_PER_MILLION)
    } else if v >= 1_000.0 {
        format!("{:.0}K", v / 1_000.0)
    } else {
        format!("{:.0}", v)
    }
}

fn bucket_f64_series(
    data: &[(NaiveDate, f64)],
    range_start: NaiveDate,
    range_end: NaiveDate,
    width: usize,
) -> Vec<f64> {
    let total_days = (range_end - range_start).num_days().max(0) as usize + 1;
    let full: Vec<f64> = (0..total_days)
        .map(|i| {
            let d = range_start + chrono::Duration::days(i as i64);
            data.binary_search_by_key(&d, |(date, _)| *date)
                .ok()
                .map(|idx| data[idx].1)
                .unwrap_or(0.0)
        })
        .collect();
    bucket_vec_f64(&full, width)
}

fn bucket_u64_series(
    data: &[(NaiveDate, u64)],
    range_start: NaiveDate,
    range_end: NaiveDate,
    width: usize,
) -> Vec<f64> {
    let total_days = (range_end - range_start).num_days().max(0) as usize + 1;
    let full: Vec<f64> = (0..total_days)
        .map(|i| {
            let d = range_start + chrono::Duration::days(i as i64);
            data.binary_search_by_key(&d, |(date, _)| *date)
                .ok()
                .map(|idx| data[idx].1 as f64)
                .unwrap_or(0.0)
        })
        .collect();
    bucket_vec_f64(&full, width)
}

fn bucket_vec_f64(full: &[f64], width: usize) -> Vec<f64> {
    if width == 0 || full.is_empty() {
        return Vec::new();
    }
    if full.len() == width {
        return full.to_vec();
    }
    if full.len() < width {
        (0..width)
            .map(|i| {
                let src = i * full.len() / width;
                full[src.min(full.len() - 1)]
            })
            .collect()
    } else {
        let bs = full.len() as f64 / width as f64;
        (0..width)
            .map(|i| {
                let s = (i as f64 * bs) as usize;
                let e = (((i + 1) as f64 * bs) as usize).min(full.len());
                full[s..e].iter().sum::<f64>()
            })
            .collect()
    }
}

fn x_axis_labels(
    range_start: NaiveDate,
    range_end: NaiveDate,
    width: usize,
) -> Vec<(usize, String)> {
    let total_days = (range_end - range_start).num_days().max(0) as usize + 1;
    if width < 6 || total_days == 0 {
        return Vec::new();
    }

    let label_slot = 8usize;
    let max_labels = (width / label_slot).max(1);
    let n_labels = max_labels.min(total_days);

    let mut labels = Vec::new();
    let cols_per_day = width as f64 / total_days as f64;

    for i in 0..n_labels {
        let day_idx = if n_labels <= 1 {
            0
        } else {
            i * (total_days - 1) / (n_labels - 1)
        };
        let col = ((day_idx as f64 + 0.5) * cols_per_day) as usize;
        let col = col.min(width.saturating_sub(6));
        let date = range_start + chrono::Duration::days(day_idx as i64);
        let label = date.format("%b %d").to_string();
        labels.push((col, label));
    }
    labels
}

// ---------------------------------------------------------------------------
// Intraday granularity for detail charts
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub enum DetailGranularity {
    Daily,
    Intraday { bucket_min: u16, total_min: u16 },
}

impl DetailGranularity {
    pub fn from_time_filter(is_today: bool, is_12h: bool, is_1h: bool) -> Self {
        if is_1h {
            DetailGranularity::Intraday {
                bucket_min: 6,
                total_min: 60,
            }
        } else if is_12h {
            DetailGranularity::Intraday {
                bucket_min: 30,
                total_min: 720,
            }
        } else if is_today {
            DetailGranularity::Intraday {
                bucket_min: 60,
                total_min: 1440,
            }
        } else {
            DetailGranularity::Daily
        }
    }

    pub fn period_suffix(&self) -> &'static str {
        match self {
            DetailGranularity::Daily => "/day  ",
            DetailGranularity::Intraday { bucket_min: 60, .. } => "/hour ",
            DetailGranularity::Intraday { bucket_min: 30, .. } => "/30m  ",
            DetailGranularity::Intraday { .. } => "/6m   ",
        }
    }
}

fn bucket_minute(
    entries: impl Iterator<Item = ((NaiveDate, u16), f64)>,
    date: NaiveDate,
    start_minute: u16,
    bucket_min: u16,
    n_buckets: usize,
    chart_width: usize,
) -> Vec<f64> {
    let mut buckets = vec![0.0f64; n_buckets];
    for ((d, m), v) in entries {
        if d != date || m < start_minute {
            continue;
        }
        let offset = m - start_minute;
        let idx = (offset / bucket_min) as usize;
        if idx < n_buckets {
            buckets[idx] += v;
        }
    }
    bucket_vec_f64(&buckets, chart_width)
}

fn x_axis_time_labels(
    start_minute: u16,
    bucket_min: u16,
    n_buckets: usize,
    width: usize,
) -> Vec<(usize, String)> {
    if n_buckets == 0 {
        return Vec::new();
    }

    let hourly = bucket_min >= 30;
    let cols_per_bucket = width as f64 / n_buckets as f64;
    let mut labels = Vec::new();

    if hourly {
        let min_gap = 3usize;
        let mut last_col: Option<usize> = None;
        for bucket_idx in 0..n_buckets {
            let minute = start_minute + bucket_idx as u16 * bucket_min;
            let m = minute % 60;
            if m >= 30 {
                continue;
            }
            let h = minute / 60;
            let label = format!("{}", h);
            let col = ((bucket_idx as f64 + 0.5) * cols_per_bucket) as usize;
            let col = col.min(width.saturating_sub(label.len()));
            if let Some(lc) = last_col
                && col < lc + min_gap
            {
                continue;
            }
            last_col = Some(col + label.len());
            labels.push((col, label));
        }
    } else {
        let label_slot = 6usize;
        if width < label_slot {
            return Vec::new();
        }
        let max_labels = (width / label_slot).max(1).min(n_buckets);
        for i in 0..max_labels {
            let bucket_idx = if max_labels <= 1 {
                0
            } else {
                i * (n_buckets - 1) / (max_labels - 1)
            };
            let minute = start_minute + bucket_idx as u16 * bucket_min;
            let h = minute / 60;
            let m = minute % 60;
            let label = format!("{:02}:{:02}", h, m);
            let col = ((bucket_idx as f64 + 0.5) * cols_per_bucket) as usize;
            let col = col.min(width.saturating_sub(label.len()));
            labels.push((col, label));
        }
    }
    labels
}

/// Render a stacked bar chart (two series overlaid: series_a on top of series_b).
#[allow(clippy::too_many_arguments)]
fn render_stacked_bar_chart(
    frame: &mut Frame,
    area: Rect,
    series_a: &[f64],
    series_b: &[f64],
    color_a: Color,
    color_b: Color,
    y_formatter: fn(f64) -> String,
    x_labels: &[(usize, String)],
) {
    let t = theme();
    let len = series_a.len().max(series_b.len());
    let stacked: Vec<f64> = (0..len)
        .map(|i| {
            let a = if i < series_a.len() { series_a[i] } else { 0.0 };
            let b = if i < series_b.len() { series_b[i] } else { 0.0 };
            a + b
        })
        .collect();
    let max_val = stacked.iter().cloned().fold(0.0f64, f64::max);

    if area.height < 3 || area.width < 10 {
        return;
    }

    let y_label_width: u16 = 7;
    let chart_x = area.x + y_label_width;
    let chart_w = area.width.saturating_sub(y_label_width) as usize;
    let chart_h = area.height.saturating_sub(1) as usize;

    if chart_w < 2 || chart_h < 1 {
        return;
    }

    let max_v = if max_val > 0.0 { max_val } else { 1.0 };

    // Y-axis labels
    let y_positions = if chart_h >= 5 {
        vec![(0, max_v), (chart_h / 2, max_v / 2.0), (chart_h - 1, 0.0)]
    } else {
        vec![(0, max_v), (chart_h - 1, 0.0)]
    };
    for (row, val) in &y_positions {
        let label = y_formatter(*val);
        let padded = format!("{:>6} ", label);
        let y = area.y + *row as u16;
        if y < area.y + area.height - 1 {
            frame.render_widget(
                Paragraph::new(Span::styled(padded, Style::default().fg(t.text_dim))),
                Rect::new(area.x, y, y_label_width, 1),
            );
        }
    }

    let sub_blocks = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    for row in 0..chart_h {
        let row_bottom = (chart_h - 1 - row) as f64 / chart_h as f64;
        let row_top = (chart_h - row) as f64 / chart_h as f64;

        let mut spans: Vec<Span> = Vec::new();
        let mut current_color = Color::Reset;
        let mut current_run = String::new();

        for col in 0..chart_w {
            let total = if col < stacked.len() {
                stacked[col]
            } else {
                0.0
            };
            let a_val = if col < series_a.len() {
                series_a[col]
            } else {
                0.0
            };
            let norm_total = total / max_v;
            let norm_a = a_val / max_v;

            let ch;
            let color;

            if norm_total <= row_bottom {
                ch = ' ';
                color = t.chart_bg;
            } else if norm_total >= row_top {
                ch = '█';
                let mid = (row_bottom + row_top) / 2.0;
                color = if mid < norm_a { color_a } else { color_b };
            } else {
                let frac = (norm_total - row_bottom) / (row_top - row_bottom);
                let idx = (frac * 7.0).round() as usize;
                ch = sub_blocks[idx.min(7)];
                let mid = row_bottom + (norm_total - row_bottom) / 2.0;
                color = if mid < norm_a { color_a } else { color_b };
            };

            if color == current_color {
                current_run.push(ch);
            } else {
                if !current_run.is_empty() {
                    spans.push(Span::styled(
                        current_run.clone(),
                        Style::default().fg(current_color),
                    ));
                }
                current_run = String::from(ch);
                current_color = color;
            }
        }
        if !current_run.is_empty() {
            spans.push(Span::styled(
                current_run,
                Style::default().fg(current_color),
            ));
        }

        let y = area.y + row as u16;
        if y < area.y + area.height - 1 {
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(chart_x, y, chart_w as u16, 1),
            );
        }
    }

    // X-axis
    let x_row = area.y + area.height - 1;
    render_x_axis_labels(frame, chart_x, x_row, chart_w, x_labels);
}

fn render_x_axis_labels(
    frame: &mut Frame,
    chart_x: u16,
    x_row: u16,
    chart_w: usize,
    labels: &[(usize, String)],
) {
    let t = theme();
    let mut x_spans: Vec<Span> = Vec::new();
    let mut cursor = 0usize;
    for (col, label) in labels {
        let col = *col;
        if col > cursor {
            x_spans.push(Span::raw(" ".repeat(col - cursor)));
            cursor = col;
        }
        let lbl = if cursor + label.len() <= chart_w {
            label.as_str()
        } else {
            ""
        };
        x_spans.push(Span::styled(lbl, Style::default().fg(t.text_dim)));
        cursor += lbl.len();
    }
    frame.render_widget(
        Paragraph::new(Line::from(x_spans)),
        Rect::new(chart_x, x_row, chart_w as u16, 1),
    );
}

// ---------------------------------------------------------------------------
// Detail view
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn render_detail(
    frame: &mut Frame,
    area: Rect,
    cards: &[ProjectCard],
    tick: usize,
    range_start: NaiveDate,
    range_end: NaiveDate,
    granularity: DetailGranularity,
    minute_tokens: &crate::data::tokens::MinuteTokens,
    minute_model_costs: &MinuteModelCosts,
) {
    let card = match cards.first() {
        Some(c) => c,
        None => return,
    };

    if area.height < 6 || area.width < 30 {
        return;
    }

    let t = theme();

    let border_color = if card.is_starred {
        t.border_highlight
    } else {
        t.text_dim
    };

    let title_line = if card.is_starred {
        rainbow_title(&card.name, tick)
    } else {
        Line::from(Span::styled(
            format!(" {} ", card.name),
            Style::default()
                .fg(t.text_primary)
                .add_modifier(Modifier::BOLD),
        ))
    };

    let block = Block::default()
        .title(title_line)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 4 || inner.width < 20 {
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(4),
        ])
        .split(inner);

    render_detail_metrics(frame, rows[0], card);

    render_detail_charts(
        frame,
        rows[2],
        card,
        granularity,
        range_start,
        range_end,
        minute_tokens,
        minute_model_costs,
    );
}

fn render_detail_metrics(frame: &mut Frame, area: Rect, card: &ProjectCard) {
    let t = theme();
    let dim = Style::default().fg(t.text_dim);
    let bright = Style::default().fg(t.text_secondary);

    let cost_str = format_cost(card.total_cost);
    let eff_str = if card.efficiency > 0.0 {
        format!("{:.0} tok/ln", card.efficiency)
    } else {
        String::new()
    };
    let sessions_str = format!("{}", card.sessions);
    let active_str = format!("{}", card.active_days);
    let span_str = format!(
        "{}",
        (card.last_activity - card.first_activity).num_days() + 1
    );
    let first_str = card.first_activity.format("%Y-%m-%d").to_string();
    let last_str = card.last_activity.format("%Y-%m-%d").to_string();

    let time_str = if card.time_minutes > 0 {
        format_duration(card.time_minutes)
    } else {
        String::new()
    };

    let mut line1_spans = vec![
        Span::styled(
            &cost_str,
            Style::default().fg(t.cost).add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled("⚡ ", Style::default().fg(t.efficiency_accent)),
        Span::styled(&eff_str, Style::default().fg(t.efficiency_accent)),
        Span::raw("   "),
        Span::styled("sessions ", dim),
        Span::styled(
            &sessions_str,
            Style::default()
                .fg(t.tokens_in)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled("active ", dim),
        Span::styled(
            &active_str,
            Style::default()
                .fg(t.tokens_out)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("/", dim),
        Span::styled(&span_str, bright),
        Span::styled("d", dim),
    ];
    if !time_str.is_empty() {
        line1_spans.push(Span::raw("   "));
        line1_spans.push(Span::styled("⏱ ", Style::default().fg(t.duration)));
        line1_spans.push(Span::styled(
            &time_str,
            Style::default().fg(t.duration).add_modifier(Modifier::BOLD),
        ));
    }
    line1_spans.push(Span::raw("   "));
    line1_spans.push(Span::styled(&first_str, Style::default().fg(t.text_dim)));
    line1_spans.push(Span::styled(" → ", dim));
    line1_spans.push(Span::styled(&last_str, Style::default().fg(t.text_dim)));
    let line1 = Line::from(line1_spans);

    let line2 = Line::from(vec![
        Span::styled("in ", dim),
        Span::styled(
            format_tokens(card.tokens_in),
            Style::default().fg(t.tokens_in),
        ),
        Span::raw("   "),
        Span::styled("out ", dim),
        Span::styled(
            format_tokens(card.tokens_out),
            Style::default().fg(t.tokens_out),
        ),
        Span::raw("   "),
        Span::styled("cache ", dim),
        Span::styled(
            format_tokens(card.tokens_cache),
            Style::default().fg(t.cache),
        ),
    ]);

    let total_lines = card.lines_added + card.lines_deleted;
    let net = card.lines_added as i64 - card.lines_deleted as i64;
    let acc_rate = if card.lines_suggested > 0 {
        format!(
            "{:.0}%",
            card.lines_accepted as f64 / card.lines_suggested as f64 * 100.0
        )
    } else {
        "-".to_string()
    };
    let line3 = Line::from(vec![
        Span::styled(
            format!("+{}", card.lines_added),
            Style::default().fg(t.lines_positive),
        ),
        Span::raw("  "),
        Span::styled(
            format!("-{}", card.lines_deleted),
            Style::default().fg(t.lines_negative),
        ),
        Span::raw("   "),
        Span::styled("net ", dim),
        Span::styled(
            format!("{}{}", if net >= 0 { "+" } else { "" }, net),
            Style::default().fg(if net >= 0 {
                t.lines_positive
            } else {
                t.lines_negative
            }),
        ),
        Span::raw("   "),
        Span::styled("total ", dim),
        Span::styled(format!("{}", total_lines), bright),
        Span::raw("   "),
        Span::styled("accepted ", dim),
        Span::styled(
            format!("{}", card.lines_accepted),
            Style::default().fg(t.lines_positive),
        ),
        Span::styled("/", dim),
        Span::styled(format!("{}", card.lines_suggested), bright),
        Span::styled(" (", dim),
        Span::styled(&acc_rate, Style::default().fg(t.cache)),
        Span::styled(")", dim),
    ]);

    let metrics = Paragraph::new(vec![line1, line2, line3]);
    frame.render_widget(metrics, area);
}

#[allow(clippy::too_many_arguments)]
fn render_detail_charts(
    frame: &mut Frame,
    charts_area: Rect,
    card: &ProjectCard,
    granularity: DetailGranularity,
    range_start: NaiveDate,
    range_end: NaiveDate,
    minute_tokens: &crate::data::tokens::MinuteTokens,
    minute_model_costs: &MinuteModelCosts,
) {
    let t = theme();

    let vert_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(4),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(charts_area);

    let chart_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(vert_split[0]);

    let left_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3)])
        .split(chart_cols[0]);

    let right_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3)])
        .split(chart_cols[1]);

    // Left chart: Cost stacked by model
    let cost_period_label = format!(" Cost{}", granularity.period_suffix());
    let left_legend = Line::from(vec![
        Span::styled(
            cost_period_label,
            Style::default()
                .fg(t.text_secondary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("■", Style::default().fg(t.model_color("opus"))),
        Span::styled(" opus ", Style::default().fg(t.text_dim)),
        Span::styled("■", Style::default().fg(t.model_color("sonnet"))),
        Span::styled(" sonnet ", Style::default().fg(t.text_dim)),
        Span::styled("■", Style::default().fg(t.model_color("haiku"))),
        Span::styled(" haiku", Style::default().fg(t.text_dim)),
    ]);
    frame.render_widget(Paragraph::new(left_legend), left_split[0]);

    let chart_w_left = left_split[1].width.saturating_sub(7) as usize;
    let model_order = ["opus", "sonnet", "haiku", "other"];

    let intraday_params = match granularity {
        DetailGranularity::Intraday {
            bucket_min,
            total_min,
        } => {
            let now = chrono::Local::now();
            let now_minute = now.hour() as u16 * 60 + now.minute() as u16;
            Some((
                now.date_naive(),
                now_minute.saturating_sub(total_min),
                bucket_min,
                (total_min / bucket_min) as usize,
            ))
        }
        _ => None,
    };

    let (model_series, left_x_labels) = match granularity {
        DetailGranularity::Daily => {
            let series: Vec<(String, Vec<f64>)> = model_order
                .iter()
                .filter_map(|&name| {
                    card.model_daily_costs
                        .iter()
                        .find(|(m, _)| m == name)
                        .map(|(m, data)| {
                            (
                                m.clone(),
                                bucket_f64_series(data, range_start, range_end, chart_w_left),
                            )
                        })
                })
                .collect();
            let labels = x_axis_labels(range_start, range_end, chart_w_left);
            (series, labels)
        }
        DetailGranularity::Intraday { bucket_min, .. } => {
            let Some((today, start_minute, _, n_buckets)) = intraday_params else {
                return;
            };
            let series: Vec<(String, Vec<f64>)> = model_order
                .iter()
                .filter_map(|&name| {
                    let key = (card.root_key.clone(), name.to_string());
                    minute_model_costs.get(&key).map(|data| {
                        (
                            name.to_string(),
                            bucket_minute(
                                data.iter().map(|(&k, &v)| (k, v)),
                                today,
                                start_minute,
                                bucket_min,
                                n_buckets,
                                chart_w_left,
                            ),
                        )
                    })
                })
                .filter(|(_, v)| v.iter().any(|x| *x > 0.0))
                .collect();
            let labels = x_axis_time_labels(start_minute, bucket_min, n_buckets, chart_w_left);
            (series, labels)
        }
    };

    render_model_stacked_chart(
        frame,
        left_split[1],
        &model_series,
        format_y_cost,
        &left_x_labels,
    );

    // Right chart: Tokens in/out stacked
    let tok_period_label = format!(" Tokens{}", granularity.period_suffix());
    let right_legend = Line::from(vec![
        Span::styled(
            tok_period_label,
            Style::default()
                .fg(t.text_secondary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("■", Style::default().fg(t.tokens_in)),
        Span::styled(" input ", Style::default().fg(t.text_dim)),
        Span::styled("■", Style::default().fg(t.tokens_out)),
        Span::styled(" output", Style::default().fg(t.text_dim)),
    ]);
    frame.render_widget(Paragraph::new(right_legend), right_split[0]);

    let chart_w_right = right_split[1].width.saturating_sub(7) as usize;

    let (tok_in, tok_out, right_x_labels) = match granularity {
        DetailGranularity::Daily => {
            let ti =
                bucket_u64_series(&card.daily_tokens_in, range_start, range_end, chart_w_right);
            let to = bucket_u64_series(
                &card.daily_tokens_out,
                range_start,
                range_end,
                chart_w_right,
            );
            let labels = x_axis_labels(range_start, range_end, chart_w_right);
            (ti, to, labels)
        }
        DetailGranularity::Intraday { bucket_min, .. } => {
            let Some((today, start_minute, _, n_buckets)) = intraday_params else {
                return;
            };
            let ti = bucket_minute(
                minute_tokens.input.iter().map(|(&k, &v)| (k, v as f64)),
                today,
                start_minute,
                bucket_min,
                n_buckets,
                chart_w_right,
            );
            let to = bucket_minute(
                minute_tokens.output.iter().map(|(&k, &v)| (k, v as f64)),
                today,
                start_minute,
                bucket_min,
                n_buckets,
                chart_w_right,
            );
            let labels = x_axis_time_labels(start_minute, bucket_min, n_buckets, chart_w_right);
            (ti, to, labels)
        }
    };

    render_stacked_bar_chart(
        frame,
        right_split[1],
        &tok_in,
        &tok_out,
        t.tokens_in,
        t.tokens_out,
        format_y_tokens,
        &right_x_labels,
    );

    // Model distribution bar
    render_model_bar(frame, vert_split[2], &card.model_shares);
}

fn render_model_bar(frame: &mut Frame, area: Rect, model_shares: &[(String, f64)]) {
    if area.width < 10 || area.height < 1 || model_shares.is_empty() {
        return;
    }

    let t = theme();
    let w = area.width as usize;
    let mut spans: Vec<Span> = Vec::new();
    let mut used = 0usize;

    for (i, (model, share)) in model_shares.iter().enumerate() {
        let segment_w = if i == model_shares.len() - 1 {
            w - used
        } else {
            ((share * w as f64).round() as usize).max(1).min(w - used)
        };
        if segment_w == 0 {
            continue;
        }

        let color = t.model_color(model);
        let pct = format!(" {}:{:.0}% ", model, share * 100.0);

        if segment_w >= pct.len() + 2 {
            let pad_left = (segment_w - pct.len()) / 2;
            let pad_right = segment_w - pct.len() - pad_left;
            spans.push(Span::styled(
                "█".repeat(pad_left),
                Style::default().fg(color),
            ));
            spans.push(Span::styled(
                pct,
                Style::default().fg(t.model_bar_text).bg(color),
            ));
            spans.push(Span::styled(
                "█".repeat(pad_right),
                Style::default().fg(color),
            ));
        } else {
            spans.push(Span::styled(
                "█".repeat(segment_w),
                Style::default().fg(color),
            ));
        }

        used += segment_w;
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn render_model_stacked_chart(
    frame: &mut Frame,
    area: Rect,
    series: &[(String, Vec<f64>)],
    y_formatter: fn(f64) -> String,
    x_labels: &[(usize, String)],
) {
    if area.height < 3 || area.width < 10 || series.is_empty() {
        return;
    }

    let t = theme();

    let y_label_width: u16 = 7;
    let chart_x = area.x + y_label_width;
    let chart_w = area.width.saturating_sub(y_label_width) as usize;
    let chart_h = area.height.saturating_sub(1) as usize;

    if chart_w < 2 || chart_h < 1 {
        return;
    }

    let max_cols = series.iter().map(|(_, v)| v.len()).max().unwrap_or(0);
    let stacked_totals: Vec<f64> = (0..max_cols)
        .map(|col| {
            series
                .iter()
                .map(|(_, v)| if col < v.len() { v[col] } else { 0.0 })
                .sum()
        })
        .collect();
    let max_v = stacked_totals.iter().cloned().fold(0.0f64, f64::max);
    let max_v = if max_v > 0.0 { max_v } else { 1.0 };

    let col_layers: Vec<Vec<(f64, Color)>> = (0..max_cols)
        .map(|col| {
            let mut layers = Vec::new();
            let mut cumulative = 0.0;
            for (model, vals) in series {
                let v = if col < vals.len() { vals[col] } else { 0.0 };
                cumulative += v;
                layers.push((cumulative / max_v, t.model_color(model)));
            }
            layers
        })
        .collect();

    // Y-axis labels
    let y_positions = if chart_h >= 5 {
        vec![(0, max_v), (chart_h / 2, max_v / 2.0), (chart_h - 1, 0.0)]
    } else {
        vec![(0, max_v), (chart_h - 1, 0.0)]
    };
    for (row, val) in &y_positions {
        let label = y_formatter(*val);
        let padded = format!("{:>6} ", label);
        let y = area.y + *row as u16;
        if y < area.y + area.height - 1 {
            frame.render_widget(
                Paragraph::new(Span::styled(padded, Style::default().fg(t.text_dim))),
                Rect::new(area.x, y, y_label_width, 1),
            );
        }
    }

    let sub_blocks = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    for row in 0..chart_h {
        let row_bottom = (chart_h - 1 - row) as f64 / chart_h as f64;
        let row_top = (chart_h - row) as f64 / chart_h as f64;

        let mut spans: Vec<Span> = Vec::new();
        let mut current_color = Color::Reset;
        let mut current_run = String::new();

        for col in 0..chart_w {
            let layers = if col < col_layers.len() {
                &col_layers[col]
            } else {
                &vec![]
            };

            let total_norm = layers.last().map(|(n, _)| *n).unwrap_or(0.0);

            let (ch, color) = if total_norm <= row_bottom {
                (' ', t.chart_bg)
            } else {
                let pixel_mid = (row_bottom + row_top.min(total_norm)) / 2.0;
                let layer_color = layers
                    .iter()
                    .find(|(cum, _)| pixel_mid < *cum)
                    .map(|(_, c)| *c)
                    .unwrap_or(t.model_other);

                if total_norm >= row_top {
                    ('█', layer_color)
                } else {
                    let frac = (total_norm - row_bottom) / (row_top - row_bottom);
                    let idx = (frac * 7.0).round() as usize;
                    (sub_blocks[idx.min(7)], layer_color)
                }
            };

            if color == current_color {
                current_run.push(ch);
            } else {
                if !current_run.is_empty() {
                    spans.push(Span::styled(
                        current_run.clone(),
                        Style::default().fg(current_color),
                    ));
                }
                current_run = String::from(ch);
                current_color = color;
            }
        }
        if !current_run.is_empty() {
            spans.push(Span::styled(
                current_run,
                Style::default().fg(current_color),
            ));
        }

        let y = area.y + row as u16;
        if y < area.y + area.height - 1 {
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(chart_x, y, chart_w as u16, 1),
            );
        }
    }

    // X-axis
    let x_row = area.y + area.height - 1;
    render_x_axis_labels(frame, chart_x, x_row, chart_w, x_labels);
}
