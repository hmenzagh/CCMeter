use std::collections::HashMap;

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, Timelike};
use ratatui::{
    prelude::*,
    symbols::Marker,
    widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph},
};

use super::theme::theme;
use crate::data::tokens::{DailyTokens, MinuteTokens};

/// Height of the heatmap grid alone: 2 borders + 1 month labels + 7 day rows.
const GRID_HEIGHT: u16 = 10;
/// Height of the sparkline trend area below each heatmap.
const TREND_HEIGHT: u16 = 4;
/// Total height: heatmap grid + trend sparklines.
pub const HEATMAP_HEIGHT: u16 = GRID_HEIGHT + TREND_HEIGHT;

/// Compute best grid layout (cols, rows) for 4 heatmap panels given available space.
pub fn compute_grid(width: u16, height: u16) -> (u16, u16) {
    let max_cols = (width / 20).min(4);
    let max_rows = height / HEATMAP_HEIGHT;

    if max_cols == 0 || max_rows == 0 {
        return (0, 0);
    }

    // Prefer wider layouts (fewer rows)
    if max_cols >= 4 {
        return (4, 1);
    }
    if max_cols >= 2 && max_rows >= 2 {
        return (2, 2);
    }
    if max_cols >= 2 {
        return (2, 1);
    }
    // 1 column
    (1, max_rows.min(4))
}

/// Height needed for heatmap grid, capped by available space.
pub fn grid_height(width: u16, max_height: u16) -> u16 {
    let (_, rows) = compute_grid(width, max_height);
    rows * HEATMAP_HEIGHT
}

const RATE_THRESHOLDS: [u64; 4] = [25, 50, 75, 100];

fn quartile_thresholds(values: &mut Vec<u64>) -> [u64; 4] {
    values.retain(|&v| v > 0);
    if values.is_empty() {
        return [1, 2, 3, 4];
    }
    values.sort();
    let n = values.len();
    [
        values[n / 4],
        values[n / 2],
        values[3 * n / 4],
        values[n - 1],
    ]
}

pub fn compute_thresholds(daily: &HashMap<NaiveDate, u64>) -> [u64; 4] {
    let mut values: Vec<u64> = daily.values().copied().collect();
    quartile_thresholds(&mut values)
}

/// Fill `width` consecutive buffer cells at (x, y) with the same symbol and color.
pub(super) fn fill_cell(
    buf: &mut ratatui::buffer::Buffer,
    x: u16,
    y: u16,
    width: u16,
    symbol: &str,
    fg: Color,
) {
    let right = buf.area().right();
    for dx in 0..width {
        let cx = x + dx;
        if cx >= right {
            break;
        }
        let cell = &mut buf[(cx, y)];
        cell.set_symbol(symbol);
        cell.set_fg(fg);
    }
}

fn token_level(value: u64, thresholds: &[u64; 4]) -> usize {
    if value == 0 {
        return 0;
    }
    for (i, &t) in thresholds.iter().enumerate() {
        if value <= t {
            return i + 1;
        }
    }
    4
}

fn month_abbrev(month: u32) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "???",
    }
}

fn format_tokens(tokens: u64) -> String {
    crate::data::models::format_tokens(tokens)
}

/// Render three side-by-side GitHub-style contribution heatmaps.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    daily: &DailyTokens,
    thresholds: &Thresholds,
    tick: usize,
    range: (NaiveDate, NaiveDate),
    expanded: bool,
) {
    let (grid_cols, grid_rows) = compute_grid(area.width, area.height);
    if grid_cols == 0 || grid_rows == 0 {
        return;
    }

    let t = theme();

    let input_total: u64 = daily.input.values().sum();
    let output_total: u64 = daily.output.values().sum();
    let lines_total: u64 = daily.lines_accepted.values().sum();
    let rate = daily.overall_accept_rate();
    let rate_map = daily.accept_rate_map();

    let input_title = format!(" Input {} ", format_tokens(input_total));
    let output_title = format!(" Output {} ", format_tokens(output_total));
    let lines_title = format!(" Lines {} ", format_tokens(lines_total));
    let rate_title = format!(" Accept {:.0}% ", rate);

    #[allow(clippy::type_complexity)]
    let panels: [(&str, &HashMap<NaiveDate, u64>, &[u64; 4], &[Color; 5]); 4] = [
        (
            &input_title,
            &daily.input,
            &thresholds.input,
            &t.input_colors,
        ),
        (
            &output_title,
            &daily.output,
            &thresholds.output,
            &t.output_colors,
        ),
        (
            &lines_title,
            &daily.lines_accepted,
            &thresholds.lines_changed,
            &t.lines_colors,
        ),
        (&rate_title, &rate_map, &RATE_THRESHOLDS, &t.rate_colors),
    ];

    let num_panels = (grid_cols * grid_rows).min(4) as usize;

    let row_constraints: Vec<Constraint> = (0..grid_rows)
        .map(|_| Constraint::Length(HEATMAP_HEIGHT))
        .collect();
    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(area);

    let mut idx = 0usize;
    for row_area in row_areas.iter() {
        let col_constraints: Vec<Constraint> = (0..grid_cols)
            .map(|_| Constraint::Ratio(1, grid_cols as u32))
            .collect();
        let col_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(col_constraints)
            .split(*row_area);

        for col_area in col_areas.iter() {
            if idx < num_panels {
                let (title, data, thresh, colors) = &panels[idx];
                render_one(
                    frame, *col_area, title, data, thresh, colors, tick, range, expanded,
                );
                idx += 1;
            }
        }
    }
}

pub struct Thresholds {
    pub input: [u64; 4],
    pub output: [u64; 4],
    pub lines_changed: [u64; 4],
}

fn render_scanner_separator(
    frame: &mut Frame,
    x: u16,
    y: u16,
    width: u16,
    max_y: u16,
    accent: Color,
    tick: usize,
) {
    if y >= max_y || width == 0 {
        return;
    }
    let t = theme();
    let (br, bg, bb) = t.scanner_base;
    let w = width as usize;
    let scan_pos = tick % (w * 2);
    let pos = if scan_pos < w {
        scan_pos
    } else {
        w * 2 - scan_pos - 1
    };
    let Color::Rgb(cr, cg, cb) = accent else {
        unreachable!()
    };

    let buf = frame.buffer_mut();
    for i in 0..w {
        let cell_x = x + i as u16;
        if cell_x >= buf.area().right() || y >= buf.area().bottom() {
            break;
        }
        let dist = (i as isize - pos as isize).unsigned_abs();
        let t = if dist < 6 {
            (6 - dist) as f32 / 6.0
        } else {
            0.0
        };
        let r = br + (cr.saturating_sub(br) as f32 * t) as u8;
        let g = bg + (cg.saturating_sub(bg) as f32 * t) as u8;
        let b = bb + (cb.saturating_sub(bb) as f32 * t) as u8;
        let cell = &mut buf[(cell_x, y)];
        cell.set_symbol("─");
        cell.set_fg(Color::Rgb(r, g, b));
    }
}

fn render_trend_line(frame: &mut Frame, area: Rect, raw_values: &[f64], accent: Color) {
    let n = raw_values.len();
    if area.height < 2 || n == 0 {
        return;
    }

    let window = 3usize;
    let smoothed: Vec<f64> = (0..n)
        .map(|i| {
            let s = i.saturating_sub(window / 2);
            let e = (i + window / 2 + 1).min(n);
            let sum: f64 = raw_values[s..e].iter().sum();
            sum / (e - s) as f64
        })
        .collect();

    let points: Vec<(f64, f64)> = smoothed
        .iter()
        .enumerate()
        .map(|(i, &val)| (i as f64, val.cbrt()))
        .collect();

    let max_val = points.iter().map(|p| p.1).fold(0.0_f64, f64::max).max(1.0);

    let datasets = vec![
        Dataset::default()
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(accent))
            .data(&points),
    ];

    let chart = Chart::new(datasets)
        .x_axis(Axis::default().bounds([0.0, (n - 1).max(1) as f64]))
        .y_axis(Axis::default().bounds([0.0, max_val]));

    frame.render_widget(chart, area);
}

#[allow(clippy::too_many_arguments)]
fn render_one(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    daily_tokens: &HashMap<NaiveDate, u64>,
    thresholds: &[u64; 4],
    colors: &[Color; 5],
    tick: usize,
    range: (NaiveDate, NaiveDate),
    expanded: bool,
) {
    if area.height < HEATMAP_HEIGHT || area.width < 20 {
        return;
    }

    let t = theme();
    let today = Local::now().date_naive();
    let dow = today.weekday().num_days_from_sunday();
    let this_sunday = today - chrono::Duration::days(dow as i64);

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(t.heatmap_title)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(t.border));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let label_cols: u16 = 5;
    let grid_width = inner.width.saturating_sub(label_cols);

    // Cap weeks to the time-filter range (+ partial week on each side)
    let range_days = (range.1 - range.0).num_days().max(0) as usize + 1;
    let max_range_weeks = range_days.div_ceil(7) + 1; // +1 for partial week at boundary
    let max_screen_weeks = grid_width as usize; // min cell_w is 1
    let num_weeks = max_screen_weeks.min(max_range_weeks);
    if num_weeks == 0 {
        return;
    }

    let cell_w: u16 = if expanded {
        (grid_width / num_weeks as u16).max(1)
    } else if (grid_width / 2) as usize >= num_weeks {
        2
    } else {
        1
    };
    let fill_w: u16 = if expanded { cell_w } else { 1 };
    let total_grid_w = label_cols + num_weeks as u16 * cell_w;
    let left_pad = (inner.width.saturating_sub(total_grid_w)) / 2;
    let gx = inner.x + left_pad;

    let start_sunday = this_sunday - chrono::Duration::days(7 * (num_weeks as i64 - 1));

    render_month_labels(
        frame,
        gx,
        inner.y,
        total_grid_w,
        label_cols,
        cell_w,
        num_weeks,
        start_sunday,
    );

    static DAY_LABELS: [&str; 7] = ["", "Mon", "", "Wed", "", "Fri", ""];
    let buf = frame.buffer_mut();
    for day in 0u32..7 {
        let y = inner.y + 1 + day as u16;
        if y >= inner.y + inner.height || y >= buf.area().bottom() {
            break;
        }

        // Write day label
        let label = DAY_LABELS[day as usize];
        for (ci, ch) in label.chars().enumerate() {
            let cx = gx + ci as u16;
            if cx < buf.area().right() {
                let cell = &mut buf[(cx, y)];
                cell.set_char(ch);
                cell.set_fg(t.heatmap_label);
            }
        }

        for week in 0..num_weeks {
            let date = start_sunday + chrono::Duration::days(7 * week as i64 + day as i64);
            let cx = gx + label_cols + (week as u16) * cell_w;
            if cx >= buf.area().right() {
                break;
            }
            if date > today {
                // leave blank (buffer is already cleared)
            } else {
                match daily_tokens.get(&date) {
                    None => {
                        fill_cell(buf, cx, y, fill_w, "\u{00b7}", t.dot_empty);
                    }
                    Some(&val) => {
                        let level = token_level(val, thresholds);
                        fill_cell(buf, cx, y, fill_w, "\u{2580}", colors[level]);
                    }
                }
            }
        }
    }

    let sep_y = inner.y + 8;
    render_scanner_separator(
        frame,
        inner.x,
        sep_y,
        inner.width,
        inner.y + inner.height,
        colors[4],
        tick,
    );

    let trend_y = sep_y + 1;
    let trend_h = (inner.y + inner.height).saturating_sub(trend_y);
    if trend_h >= 2 {
        let num_days = num_weeks * 7;
        let raw: Vec<f64> = (0..num_days)
            .map(|i| {
                let date = today - chrono::Duration::days((num_days - 1 - i) as i64);
                daily_tokens.get(&date).copied().unwrap_or(0) as f64
            })
            .collect();
        render_trend_line(
            frame,
            Rect::new(inner.x, trend_y, inner.width, trend_h),
            &raw,
            colors[4],
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_month_labels(
    frame: &mut Frame,
    x: u16,
    y: u16,
    width: u16,
    label_cols: u16,
    cell_w: u16,
    num_weeks: usize,
    start_sunday: NaiveDate,
) {
    let mut buf = vec![' '; width as usize];
    let mut last_label_end: usize = 0;

    for week in 0..num_weeks {
        let week_date = start_sunday + chrono::Duration::days(7 * week as i64);
        let prev_week = week_date - chrono::Duration::days(7);
        if week == 0 || week_date.month() != prev_week.month() {
            let col = label_cols as usize + week * cell_w as usize;
            if col >= last_label_end {
                let label = month_abbrev(week_date.month());
                for (i, ch) in label.chars().enumerate() {
                    let pos = col + i;
                    if pos < buf.len() {
                        buf[pos] = ch;
                    }
                }
                last_label_end = col + label.len() + 1;
            }
        }
    }

    let text: String = buf.into_iter().collect();
    frame.render_widget(
        Paragraph::new(Span::styled(
            text,
            Style::default().fg(theme().heatmap_label),
        )),
        Rect::new(x, y, width, 1),
    );
}

// === Intraday support ===

#[derive(Clone, Copy, PartialEq)]
pub enum IntradayMode {
    Today,
    Hours12,
    Hour1,
}

const INTRADAY_ROWS: usize = 6;
const INTRADAY_ROW_LABELS: [&str; 6] = [":00", ":10", ":20", ":30", ":40", ":50"];

impl IntradayMode {
    fn bucket_minutes(self) -> u16 {
        match self {
            Self::Today | Self::Hours12 => 10,
            Self::Hour1 => 1,
        }
    }

    fn total_buckets(self) -> usize {
        match self {
            Self::Today => 144,
            Self::Hours12 => 72,
            Self::Hour1 => 60,
        }
    }

    fn max_cols(self) -> usize {
        match self {
            Self::Today => 24,
            Self::Hours12 => 12,
            Self::Hour1 => 10,
        }
    }

    fn bucket_index(self, row: usize, col: usize) -> usize {
        match self {
            Self::Today | Self::Hours12 => col * INTRADAY_ROWS + row,
            Self::Hour1 => row * 10 + col,
        }
    }
}

fn intraday_start(now: NaiveDateTime, mode: IntradayMode) -> NaiveDateTime {
    match mode {
        IntradayMode::Today => now.date().and_hms_opt(0, 0, 0).unwrap_or(now),
        IntradayMode::Hours12 => {
            let start = now - chrono::Duration::hours(11);
            start
                .date()
                .and_hms_opt(start.hour(), 0, 0)
                .unwrap_or(start)
        }
        IntradayMode::Hour1 => {
            let s = now - chrono::Duration::minutes(59);
            s.date().and_hms_opt(s.hour(), s.minute(), 0).unwrap_or(s)
        }
    }
}

fn extract_intraday_buckets(
    data: &HashMap<(NaiveDate, u16), u64>,
    start: NaiveDateTime,
    total: usize,
    bucket_min: u16,
) -> Vec<u64> {
    let mut buckets = vec![0u64; total];
    for (i, bucket) in buckets.iter_mut().enumerate() {
        let base = start + chrono::Duration::minutes(i as i64 * bucket_min as i64);
        let date = base.date();
        let minute_base = base.hour() as u16 * 60 + base.minute() as u16;
        for m in 0..bucket_min {
            if let Some(&val) = data.get(&(date, minute_base + m)) {
                *bucket += val;
            }
        }
    }
    buckets
}

fn compute_thresholds_from_slice(values: &[u64]) -> [u64; 4] {
    let mut v: Vec<u64> = values.to_vec();
    quartile_thresholds(&mut v)
}

fn build_col_labels(start: NaiveDateTime, mode: IntradayMode) -> Vec<String> {
    match mode {
        IntradayMode::Today => (0..24).map(|h| format!("{}", h)).collect(),
        IntradayMode::Hours12 => (0i64..12)
            .map(|i| {
                let dt = start + chrono::Duration::hours(i);
                format!("{}", dt.hour())
            })
            .collect(),
        IntradayMode::Hour1 => (0..10).map(|m| format!("{}", m)).collect(),
    }
}

pub fn render_intraday(
    frame: &mut Frame,
    area: Rect,
    minute_data: &MinuteTokens,
    mode: IntradayMode,
    tick: usize,
    expanded: bool,
) {
    let (grid_cols, grid_rows) = compute_grid(area.width, area.height);
    if grid_cols == 0 || grid_rows == 0 {
        return;
    }

    let t = theme();

    let now = Local::now().naive_local();
    let start = intraday_start(now, mode);
    let total = mode.total_buckets();
    let bucket_min = mode.bucket_minutes();

    let input_b = extract_intraday_buckets(&minute_data.input, start, total, bucket_min);
    let output_b = extract_intraday_buckets(&minute_data.output, start, total, bucket_min);
    let lines_b = extract_intraday_buckets(&minute_data.lines_accepted, start, total, bucket_min);
    let sug_b = extract_intraday_buckets(&minute_data.lines_suggested, start, total, bucket_min);

    let elapsed = (now - start).num_minutes().max(0) as usize;
    let active = (elapsed / bucket_min as usize + 1).min(total);

    let rate_b: Vec<u64> = lines_b
        .iter()
        .zip(sug_b.iter())
        .map(|(&a, &s)| {
            if s > 0 {
                ((a as f64 / s as f64) * 100.0).min(100.0) as u64
            } else {
                0
            }
        })
        .collect();

    let active_end = active.min(input_b.len());
    let input_t = compute_thresholds_from_slice(&input_b[..active_end]);
    let output_t = compute_thresholds_from_slice(&output_b[..active_end]);
    let lines_t = compute_thresholds_from_slice(&lines_b[..active_end]);

    let input_total: u64 = input_b[..active_end].iter().sum();
    let output_total: u64 = output_b[..active_end].iter().sum();
    let lines_total: u64 = lines_b[..active_end].iter().sum();
    let sug_total: u64 = sug_b[..active_end].iter().sum();
    let rate = if sug_total > 0 {
        (lines_total as f64 / sug_total as f64 * 100.0).min(100.0)
    } else {
        0.0
    };

    let input_title = format!(" Input {} ", format_tokens(input_total));
    let output_title = format!(" Output {} ", format_tokens(output_total));
    let lines_title = format!(" Lines {} ", format_tokens(lines_total));
    let rate_title = format!(" Accept {:.0}% ", rate);

    let col_labels = build_col_labels(start, mode);

    let row_labels: Vec<String> = if mode == IntradayMode::Hour1 {
        (0i64..6)
            .map(|r| {
                let dt = start + chrono::Duration::minutes(r * 10);
                format!(":{:02}", dt.minute())
            })
            .collect()
    } else {
        INTRADAY_ROW_LABELS.iter().map(|s| s.to_string()).collect()
    };

    let num_panels = (grid_cols * grid_rows).min(4) as usize;

    let row_constraints: Vec<Constraint> = (0..grid_rows)
        .map(|_| Constraint::Length(HEATMAP_HEIGHT))
        .collect();
    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(area);

    #[allow(clippy::type_complexity)]
    let panel_data: [(&str, &[u64], &[u64; 4], &[Color; 5]); 4] = [
        (&input_title, &input_b, &input_t, &t.input_colors),
        (&output_title, &output_b, &output_t, &t.output_colors),
        (&lines_title, &lines_b, &lines_t, &t.lines_colors),
        (&rate_title, &rate_b, &RATE_THRESHOLDS, &t.rate_colors),
    ];

    let mut idx = 0usize;
    for row_area in row_areas.iter() {
        let col_constraints: Vec<Constraint> = (0..grid_cols)
            .map(|_| Constraint::Ratio(1, grid_cols as u32))
            .collect();
        let col_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(col_constraints)
            .split(*row_area);

        for col_area in col_areas.iter() {
            if idx < num_panels {
                let (title, data, thresh, colors) = &panel_data[idx];
                render_intraday_one(
                    frame,
                    *col_area,
                    title,
                    data,
                    active,
                    mode,
                    &col_labels,
                    &row_labels,
                    thresh,
                    colors,
                    tick,
                    expanded,
                );
                idx += 1;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_intraday_one(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    buckets: &[u64],
    active: usize,
    mode: IntradayMode,
    col_labels: &[String],
    row_labels: &[String],
    thresholds: &[u64; 4],
    colors: &[Color; 5],
    tick: usize,
    expanded: bool,
) {
    if area.height < HEATMAP_HEIGHT || area.width < 12 {
        return;
    }

    let t = theme();

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(t.heatmap_title)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(t.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let label_cols: u16 = 4;
    let grid_width = inner.width.saturating_sub(label_cols);
    let max_cols = mode.max_cols() as u16;
    if max_cols == 0 || grid_width == 0 {
        return;
    }

    // Expanded: at least 1 char wider than compact so the difference is always visible.
    // If not all columns fit, the existing scroll logic shows the most recent subset.
    let compact_w: u16 = if grid_width / 2 >= max_cols { 2 } else { 1 };
    let cell_w: u16 = if expanded {
        (grid_width / max_cols).max(1)
    } else {
        compact_w
    };
    let num_cols = (grid_width / cell_w).min(max_cols) as usize;
    let fill_w = if expanded { cell_w } else { 1 };

    let col_offset = match mode {
        IntradayMode::Hour1 => 0,
        _ => {
            let last_col = if active == 0 {
                0
            } else {
                (active - 1) / INTRADAY_ROWS
            };
            if num_cols > last_col + 1 {
                0
            } else {
                (last_col + 1).saturating_sub(num_cols)
            }
        }
    };

    let total_grid_w = label_cols + num_cols as u16 * cell_w;
    let left_pad = (inner.width.saturating_sub(total_grid_w)) / 2;
    let gx = inner.x + left_pad;

    let end = (col_offset + num_cols).min(col_labels.len());
    let visible_labels: Vec<&str> = col_labels[col_offset..end]
        .iter()
        .map(|s| s.as_str())
        .collect();
    render_intraday_col_labels(
        frame,
        gx,
        inner.y,
        total_grid_w,
        label_cols,
        cell_w,
        &visible_labels,
    );

    let buf = frame.buffer_mut();
    for (row, label) in row_labels.iter().enumerate().take(INTRADAY_ROWS) {
        let y = inner.y + 1 + row as u16;
        if y >= inner.y + inner.height || y >= buf.area().bottom() {
            break;
        }

        // Write row label
        for (ci, ch) in label.chars().enumerate() {
            let cx = gx + ci as u16;
            if cx < buf.area().right() {
                let cell = &mut buf[(cx, y)];
                cell.set_char(ch);
                cell.set_fg(t.heatmap_label);
            }
        }

        for col in 0..num_cols {
            let actual_col = col_offset + col;
            let bi = mode.bucket_index(row, actual_col);
            let cx = gx + label_cols + (col as u16) * cell_w;
            if cx >= buf.area().right() {
                break;
            }
            if bi >= buckets.len() || bi >= active {
                // leave blank
            } else {
                let val = buckets[bi];
                if val == 0 {
                    fill_cell(buf, cx, y, fill_w, "\u{00b7}", t.dot_empty);
                } else {
                    let level = token_level(val, thresholds);
                    fill_cell(buf, cx, y, fill_w, "\u{2580}", colors[level]);
                }
            }
        }
    }

    let sep_y = inner.y + 1 + INTRADAY_ROWS as u16;
    render_scanner_separator(
        frame,
        inner.x,
        sep_y,
        inner.width,
        inner.y + inner.height,
        colors[4],
        tick,
    );

    let trend_y = sep_y + 1;
    let trend_h = (inner.y + inner.height).saturating_sub(trend_y);
    if trend_h >= 2 && active > 0 {
        let raw: Vec<f64> = buckets[..active].iter().map(|&v| v as f64).collect();
        render_trend_line(
            frame,
            Rect::new(inner.x, trend_y, inner.width, trend_h),
            &raw,
            colors[4],
        );
    }
}

// === Weekly (days × hours) heatmap ===
// "Last Week" uses a 7-column (days) × 6-row (4-hour blocks) grid built from
// minute-level data, giving finer granularity than the daily calendar view.

const WEEKLY_ROWS: usize = 6;
const WEEKLY_BUCKET_HOURS: usize = 4;
const WEEKLY_ROW_LABELS: [&str; 6] = ["0h", "4h", "8h", "12h", "16h", "20h"];

/// Aggregate minute-level data into 4-hour buckets for the 7 days starting at `start_date`.
fn extract_weekly_buckets(
    data: &HashMap<(NaiveDate, u16), u64>,
    start_date: NaiveDate,
) -> Vec<u64> {
    // 7 days × 6 four-hour blocks = 42 buckets
    // Layout: bucket index = day * WEEKLY_ROWS + row
    let mut buckets = vec![0u64; 7 * WEEKLY_ROWS];
    for (&(date, minute), &val) in data {
        let day = (date - start_date).num_days();
        if !(0..7).contains(&day) {
            continue;
        }
        let row = (minute as usize / 60) / WEEKLY_BUCKET_HOURS;
        if row < WEEKLY_ROWS {
            buckets[day as usize * WEEKLY_ROWS + row] += val;
        }
    }
    buckets
}

fn build_weekly_col_labels(start_date: NaiveDate) -> Vec<String> {
    (0..7)
        .map(|i| {
            let d = start_date + chrono::Duration::days(i as i64);
            match d.weekday() {
                chrono::Weekday::Mon => "Mo",
                chrono::Weekday::Tue => "Tu",
                chrono::Weekday::Wed => "We",
                chrono::Weekday::Thu => "Th",
                chrono::Weekday::Fri => "Fr",
                chrono::Weekday::Sat => "Sa",
                chrono::Weekday::Sun => "Su",
            }
            .to_string()
        })
        .collect()
}

pub fn render_weekly(
    frame: &mut Frame,
    area: Rect,
    minute_data: &MinuteTokens,
    tick: usize,
    expanded: bool,
) {
    let (grid_cols, grid_rows) = compute_grid(area.width, area.height);
    if grid_cols == 0 || grid_rows == 0 {
        return;
    }

    let t = theme();
    let today = Local::now().date_naive();
    let start_date = today - chrono::Duration::days(6);

    let input_b = extract_weekly_buckets(&minute_data.input, start_date);
    let output_b = extract_weekly_buckets(&minute_data.output, start_date);
    let lines_b = extract_weekly_buckets(&minute_data.lines_accepted, start_date);
    let sug_b = extract_weekly_buckets(&minute_data.lines_suggested, start_date);

    let now = Local::now().naive_local();
    let today_col = 6usize; // last column is today
    let current_hour = now.hour() as usize;
    let current_row = current_hour / WEEKLY_BUCKET_HOURS;
    let active = today_col * WEEKLY_ROWS + current_row + 1;

    let rate_b: Vec<u64> = lines_b
        .iter()
        .zip(sug_b.iter())
        .map(|(&a, &s)| {
            if s > 0 {
                ((a as f64 / s as f64) * 100.0).min(100.0) as u64
            } else {
                0
            }
        })
        .collect();

    let active_end = active.min(input_b.len());
    let input_t = compute_thresholds_from_slice(&input_b[..active_end]);
    let output_t = compute_thresholds_from_slice(&output_b[..active_end]);
    let lines_t = compute_thresholds_from_slice(&lines_b[..active_end]);

    let input_total: u64 = input_b[..active_end].iter().sum();
    let output_total: u64 = output_b[..active_end].iter().sum();
    let lines_total: u64 = lines_b[..active_end].iter().sum();
    let sug_total: u64 = sug_b[..active_end].iter().sum();
    let rate = if sug_total > 0 {
        (lines_total as f64 / sug_total as f64 * 100.0).min(100.0)
    } else {
        0.0
    };

    let input_title = format!(" Input {} ", format_tokens(input_total));
    let output_title = format!(" Output {} ", format_tokens(output_total));
    let lines_title = format!(" Lines {} ", format_tokens(lines_total));
    let rate_title = format!(" Accept {:.0}% ", rate);

    let col_labels = build_weekly_col_labels(start_date);

    let num_panels = (grid_cols * grid_rows).min(4) as usize;

    let row_constraints: Vec<Constraint> = (0..grid_rows)
        .map(|_| Constraint::Length(HEATMAP_HEIGHT))
        .collect();
    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(area);

    #[allow(clippy::type_complexity)]
    let panel_data: [(&str, &[u64], &[u64; 4], &[Color; 5]); 4] = [
        (&input_title, &input_b, &input_t, &t.input_colors),
        (&output_title, &output_b, &output_t, &t.output_colors),
        (&lines_title, &lines_b, &lines_t, &t.lines_colors),
        (&rate_title, &rate_b, &RATE_THRESHOLDS, &t.rate_colors),
    ];

    let mut idx = 0usize;
    for row_area in row_areas.iter() {
        let col_constraints: Vec<Constraint> = (0..grid_cols)
            .map(|_| Constraint::Ratio(1, grid_cols as u32))
            .collect();
        let col_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(col_constraints)
            .split(*row_area);

        for col_area in col_areas.iter() {
            if idx < num_panels {
                let (title, data, thresh, colors) = &panel_data[idx];
                render_weekly_one(
                    frame,
                    *col_area,
                    title,
                    data,
                    active,
                    &col_labels,
                    thresh,
                    colors,
                    tick,
                    expanded,
                );
                idx += 1;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_weekly_one(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    buckets: &[u64],
    active: usize,
    col_labels: &[String],
    thresholds: &[u64; 4],
    colors: &[Color; 5],
    tick: usize,
    expanded: bool,
) {
    if area.height < HEATMAP_HEIGHT || area.width < 12 {
        return;
    }

    let t = theme();

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(t.heatmap_title)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(t.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let label_cols: u16 = 4;
    let grid_width = inner.width.saturating_sub(label_cols);
    let num_cols: usize = 7;
    let cell_w: u16 = if expanded {
        (grid_width / num_cols as u16).max(1)
    } else if grid_width / 2 >= num_cols as u16 {
        2
    } else {
        1
    };

    let total_grid_w = label_cols + num_cols as u16 * cell_w;
    let left_pad = (inner.width.saturating_sub(total_grid_w)) / 2;
    let gx = inner.x + left_pad;
    let fill_w = if expanded { cell_w } else { 1 };

    let visible_labels: Vec<&str> = col_labels.iter().map(|s| s.as_str()).collect();
    render_intraday_col_labels(
        frame,
        gx,
        inner.y,
        total_grid_w,
        label_cols,
        cell_w,
        &visible_labels,
    );

    let buf = frame.buffer_mut();
    for (row, label) in WEEKLY_ROW_LABELS.iter().enumerate().take(WEEKLY_ROWS) {
        let y = inner.y + 1 + row as u16;
        if y >= inner.y + inner.height || y >= buf.area().bottom() {
            break;
        }

        for (ci, ch) in label.chars().enumerate() {
            let cx = gx + ci as u16;
            if cx < buf.area().right() {
                let cell = &mut buf[(cx, y)];
                cell.set_char(ch);
                cell.set_fg(t.heatmap_label);
            }
        }

        for col in 0..num_cols {
            let bi = col * WEEKLY_ROWS + row;
            let cx = gx + label_cols + (col as u16) * cell_w;
            if cx >= buf.area().right() {
                break;
            }
            if bi >= buckets.len() || bi >= active {
                // future — leave blank
            } else {
                let val = buckets[bi];
                if val == 0 {
                    fill_cell(buf, cx, y, fill_w, "\u{00b7}", t.dot_empty);
                } else {
                    let level = token_level(val, thresholds);
                    fill_cell(buf, cx, y, fill_w, "\u{2580}", colors[level]);
                }
            }
        }
    }

    let sep_y = inner.y + 1 + WEEKLY_ROWS as u16;
    render_scanner_separator(
        frame,
        inner.x,
        sep_y,
        inner.width,
        inner.y + inner.height,
        colors[4],
        tick,
    );

    let trend_y = sep_y + 1;
    let trend_h = (inner.y + inner.height).saturating_sub(trend_y);
    if trend_h >= 2 && active > 0 {
        let raw: Vec<f64> = buckets[..active].iter().map(|&v| v as f64).collect();
        render_trend_line(
            frame,
            Rect::new(inner.x, trend_y, inner.width, trend_h),
            &raw,
            colors[4],
        );
    }
}

fn render_intraday_col_labels(
    frame: &mut Frame,
    x: u16,
    y: u16,
    width: u16,
    label_cols: u16,
    cell_w: u16,
    labels: &[&str],
) {
    let mut buf = vec![' '; width as usize];
    let mut last_label_end: usize = 0;

    for (i, label) in labels.iter().enumerate() {
        let col = label_cols as usize + i * cell_w as usize;
        if col >= last_label_end {
            for (j, ch) in label.chars().enumerate() {
                let pos = col + j;
                if pos < buf.len() {
                    buf[pos] = ch;
                }
            }
            last_label_end = col + label.len() + 1;
        }
    }
    let text: String = buf.into_iter().collect();
    frame.render_widget(
        Paragraph::new(Span::styled(
            text,
            Style::default().fg(theme().heatmap_label),
        )),
        Rect::new(x, y, width, 1),
    );
}
