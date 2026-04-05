use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph},
};

use super::cards;
use super::heatmap;
use super::theme::theme;
use super::time_filter::TimeFilter;
use crate::app::{App, View};

// ---------------------------------------------------------------------------
// impl App — drawing methods
// ---------------------------------------------------------------------------

impl App {
    pub(crate) fn draw(&self, frame: &mut Frame) {
        let area = frame.area();

        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(area);

        self.draw_footer(frame, outer[2]);

        let top_cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(30),
                Constraint::Percentage(40),
                Constraint::Percentage(30),
            ])
            .split(outer[0]);
        self.draw_header(frame, &top_cols);

        let content_area = outer[1];
        const MIN_WIDTH: u16 = 50;
        const MIN_HEIGHT: u16 = 30;
        if content_area.width < MIN_WIDTH || content_area.height < MIN_HEIGHT {
            self.draw_too_small_popup(frame, content_area, MIN_WIDTH, MIN_HEIGHT);
        } else {
            match &self.view {
                View::Main => self.draw_main_dashboard(frame, content_area),
                View::Settings(state) => {
                    state.render(
                        frame,
                        content_area,
                        &self.config.groups,
                        &self.config.overrides,
                    );
                }
            }
        }
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        let t = theme();
        let footer_text = if self.reloading {
            "⟳ Reloading…"
        } else if self.project_index.is_some() {
            "Esc Back   Tab Period   ←→ Project   r Reload   q Quit"
        } else {
            "Tab Period   ⇧Tab Source   ←→ Project   ↑↓ Scroll   r Reload   . Settings   q Quit"
        };
        let footer = Paragraph::new(Span::styled(
            footer_text,
            Style::default().fg(if self.reloading {
                t.warning
            } else {
                t.text_dim
            }),
        ))
        .alignment(Alignment::Center);
        frame.render_widget(footer, area);
    }

    fn draw_header(&self, frame: &mut Frame, top_cols: &[Rect]) {
        let t = theme();
        {
            let time_labels: Vec<&str> = TimeFilter::ALL.iter().map(|f| f.label()).collect();
            let selected = self.time_filter.index();
            let spans = scrollable_tabs(
                &time_labels,
                selected,
                top_cols[0].width as usize,
                Style::default().fg(t.text_dim),
                Style::default()
                    .fg(t.tokens_out)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(t.divider),
            );
            frame.render_widget(Paragraph::new(Line::from(spans)), top_cols[0]);
        }

        let proj_display = match self.project_index {
            None => "All projects".to_string(),
            Some(i) => {
                let name = &self.config.groups[i].name;
                let total = self.config.groups.len();
                format!("◀ {} ({}/{}) ▶", name, i + 1, total)
            }
        };
        let proj_style = if self.project_index.is_some() {
            Style::default().fg(t.cost).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.text_dim)
        };
        let proj_line =
            Paragraph::new(Span::styled(proj_display, proj_style)).alignment(Alignment::Center);
        frame.render_widget(proj_line, top_cols[1]);

        {
            let src_labels: Vec<&str> = self
                .config
                .source_names
                .iter()
                .map(|s| s.as_str())
                .collect();
            let spans = scrollable_tabs(
                &src_labels,
                self.source_index,
                top_cols[2].width as usize,
                Style::default().fg(t.text_dim),
                Style::default().fg(t.cache).add_modifier(Modifier::BOLD),
                Style::default().fg(t.divider),
            );
            frame.render_widget(
                Paragraph::new(Line::from(spans)).alignment(Alignment::Right),
                top_cols[2],
            );
        }
    }

    fn draw_too_small_popup(&self, frame: &mut Frame, area: Rect, min_w: u16, min_h: u16) {
        let t = theme();
        let msg = format!(
            "Terminal too small ({}x{})\nMinimum: {}x{}",
            area.width, area.height, min_w, min_h,
        );
        let popup_w = 30u16.min(area.width);
        let popup_h = 4u16.min(area.height);
        let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
        let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
        let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);
        let popup = Paragraph::new(msg)
            .alignment(Alignment::Center)
            .style(Style::default().fg(t.error))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .border_style(Style::default().fg(t.error))
                    .title(" ⚠ "),
            );
        frame.render_widget(popup, popup_area);
    }

    fn draw_main_dashboard(&self, frame: &mut Frame, content_area: Rect) {
        let t = theme();
        let tick = (self.start_time.elapsed().as_millis() / 150) as usize;
        static STAR_FRAMES: [&str; 12] =
            [" ", "·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢", "·"];
        let star = STAR_FRAMES[tick % STAR_FRAMES.len()];
        let pulse = ((tick % 12) as f32 / 11.0 * std::f32::consts::PI).sin();
        let (sr, sg, sb) = t.star_base;
        let (ar, ag, ab) = t.star_amplitude;
        let r = sr + (ar * pulse) as u8;
        let g = sg + (ag * pulse) as u8;
        let b = sb + (ab * pulse) as u8;
        let star_style = Style::default().fg(Color::Rgb(r, g, b));

        let title_spans = vec![
            Span::raw(" "),
            Span::styled(star, star_style),
            Span::styled(
                " CCMeter ",
                Style::default().fg(t.title).add_modifier(Modifier::BOLD),
            ),
        ];
        let block = Block::default()
            .title(Line::from(title_spans))
            .borders(Borders::ALL);
        let inner = block.inner(content_area);
        frame.render_widget(block, content_area);

        let kpi_cols: usize = if inner.width >= 50 {
            5
        } else if inner.width >= 30 {
            3
        } else {
            1
        };
        let kpi_rows = 5_usize.div_ceil(kpi_cols);
        let kpi_h = (kpi_rows * 3) as u16;
        let max_heatmap_h = inner.height.saturating_sub(kpi_h + 1);
        let heatmap_h = heatmap::grid_height(inner.width, max_heatmap_h);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(heatmap_h),
                Constraint::Length(kpi_h),
                Constraint::Min(1),
            ])
            .split(inner);

        let anim_tick = (self.start_time.elapsed().as_millis() / 80) as usize;
        if self.time_filter.is_intraday() {
            let mode = match self.time_filter {
                TimeFilter::Hour1 => heatmap::IntradayMode::Hour1,
                TimeFilter::Hour12 => heatmap::IntradayMode::Hours12,
                TimeFilter::Today => heatmap::IntradayMode::Today,
                _ => unreachable!(),
            };
            heatmap::render_intraday(frame, chunks[0], &self.data.minute_tokens, mode, anim_tick);
        } else if self.time_filter == TimeFilter::LastWeek {
            heatmap::render_weekly(
                frame,
                chunks[0],
                &self.data.minute_tokens,
                self.render.range,
                anim_tick,
            );
        } else {
            heatmap::render(
                frame,
                chunks[0],
                &self.render.filtered,
                &self.data.thresholds,
                anim_tick,
                self.render.range,
            );
        }

        self.draw_kpi_grid(frame, chunks[1], kpi_cols, kpi_rows);

        let (range_start, range_end) = self.render.range;
        if self.project_index.is_some() {
            let gran = cards::DetailGranularity::from_time_filter(
                self.time_filter == TimeFilter::Today,
                self.time_filter == TimeFilter::Hour12,
                self.time_filter == TimeFilter::Hour1,
            );
            cards::render_detail(
                frame,
                chunks[2],
                &self.render.cards,
                anim_tick,
                range_start,
                range_end,
                gran,
                &self.data.minute_tokens,
                &self.render.minute_model,
            );
        } else {
            cards::render(
                frame,
                chunks[2],
                &self.render.cards,
                anim_tick,
                range_start,
                range_end,
                self.card_scroll,
            );
        }
    }

    fn draw_kpi_grid(&self, frame: &mut Frame, area: Rect, kpi_cols: usize, kpi_rows: usize) {
        let t = theme();
        let cost = self.render.kpi.cost;
        let streak = self.render.kpi.streak;
        let active = self.render.kpi.active;
        let total_days = self.render.kpi.total_days;
        let avg = self.render.kpi.avg;
        let efficiency = self.render.kpi.efficiency;

        let cost_str = if cost >= 100.0 {
            format!("${:.0}", cost)
        } else {
            format!("${:.2}", cost)
        };
        let avg_str = crate::data::models::format_tokens(avg);
        let eff_str = if efficiency > 0.0 {
            format!("{:.0} tok/ln", efficiency)
        } else {
            "—".to_string()
        };

        let streak_str = format!("{} days", streak);
        let active_str = format!("{}/{}", active, total_days);
        let values: [(&str, &str, Color); 5] = [
            (&cost_str, " Cost USD ", t.cost),
            (&streak_str, " Streak ", t.tokens_in),
            (&active_str, " Active days ", t.tokens_out),
            (&avg_str, " Avg/day ", t.cache),
            (&eff_str, " Efficiency ", t.lines_positive),
        ];

        let kpi_row_constraints: Vec<Constraint> =
            (0..kpi_rows).map(|_| Constraint::Length(3)).collect();
        let kpi_row_areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints(kpi_row_constraints)
            .split(area);

        let mut kpi_idx = 0usize;
        for kpi_row_area in kpi_row_areas.iter() {
            let remaining = 5 - kpi_idx;
            let in_this_row = remaining.min(kpi_cols);
            let col_constraints: Vec<Constraint> = (0..in_this_row)
                .map(|_| Constraint::Ratio(1, in_this_row as u32))
                .collect();
            let col_areas = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(col_constraints)
                .split(*kpi_row_area);

            for col_area in col_areas.iter() {
                if kpi_idx < 5 {
                    let (val, label, color) = &values[kpi_idx];
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
                    kpi_idx += 1;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Scrollable tabs widget
// ---------------------------------------------------------------------------

fn scrollable_tabs<'a>(
    labels: &[&'a str],
    selected: usize,
    max_width: usize,
    normal: Style,
    highlight: Style,
    divider_style: Style,
) -> Vec<Span<'a>> {
    let divider = " │ ";
    let div_len = divider.len();

    let item_widths: Vec<usize> = labels.iter().map(|l| l.len()).collect();
    let total_width: usize = item_widths.iter().sum::<usize>()
        + if labels.len() > 1 {
            (labels.len() - 1) * div_len
        } else {
            0
        };

    if total_width <= max_width || labels.is_empty() {
        let mut spans = Vec::new();
        for (i, &label) in labels.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(divider, divider_style));
            }
            let style = if i == selected { highlight } else { normal };
            spans.push(Span::styled(label, style));
        }
        return spans;
    }

    let arrow_left = "◀ ";
    let arrow_right = " ▶";
    let arrow_w = 2;

    let mut start = selected;
    let mut end = selected;
    let mut used = item_widths[selected];

    loop {
        let left_cost = if start > 0 {
            div_len + item_widths[start - 1] + if start - 1 == 0 { 0 } else { arrow_w }
        } else {
            usize::MAX
        };
        let right_cost = if end < labels.len() - 1 {
            div_len
                + item_widths[end + 1]
                + if end + 1 == labels.len() - 1 {
                    0
                } else {
                    arrow_w
                }
        } else {
            usize::MAX
        };

        let expanded = if left_cost <= right_cost {
            if start > 0 {
                let new_used = used + div_len + item_widths[start - 1];
                let future_left = if start - 1 > 0 { arrow_w } else { 0 };
                let future_right = if end < labels.len() - 1 { arrow_w } else { 0 };
                if new_used + future_left + future_right <= max_width {
                    start -= 1;
                    used = new_used;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            if end < labels.len() - 1 {
                let new_used = used + div_len + item_widths[end + 1];
                let future_left = if start > 0 { arrow_w } else { 0 };
                let future_right = if end + 1 < labels.len() - 1 {
                    arrow_w
                } else {
                    0
                };
                if new_used + future_left + future_right <= max_width {
                    end += 1;
                    used = new_used;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };

        if !expanded {
            let expanded2 = if left_cost > right_cost {
                if start > 0 {
                    let new_used = used + div_len + item_widths[start - 1];
                    let future_left = if start - 1 > 0 { arrow_w } else { 0 };
                    let future_right = if end < labels.len() - 1 { arrow_w } else { 0 };
                    if new_used + future_left + future_right <= max_width {
                        start -= 1;
                        used = new_used;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                if end < labels.len() - 1 {
                    let new_used = used + div_len + item_widths[end + 1];
                    let future_left = if start > 0 { arrow_w } else { 0 };
                    let future_right = if end + 1 < labels.len() - 1 {
                        arrow_w
                    } else {
                        0
                    };
                    if new_used + future_left + future_right <= max_width {
                        end += 1;
                        used = new_used;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            };
            if !expanded2 {
                break;
            }
        }
    }

    let mut spans = Vec::new();
    if start > 0 {
        spans.push(Span::styled(arrow_left, divider_style));
    }
    for (i, label) in labels.iter().enumerate().take(end + 1).skip(start) {
        if i > start {
            spans.push(Span::styled(divider, divider_style));
        }
        let style = if i == selected { highlight } else { normal };
        spans.push(Span::styled(*label, style));
    }
    if end < labels.len() - 1 {
        spans.push(Span::styled(arrow_right, divider_style));
    }
    spans
}
