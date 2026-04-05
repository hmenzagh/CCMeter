use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use super::theme::theme;
use crate::config::discovery::{self, OverrideInfo, ProjectGroup};
use crate::config::overrides::Overrides;

#[derive(Debug, Clone, Copy, PartialEq)]
enum RowKind {
    Group(usize),
    Source(usize, usize),
}

pub enum KeyResult {
    Continue,
    Close,
    Rebuild,
}

struct TextInput {
    input: String,
    cursor: usize,
}

impl TextInput {
    fn new(initial: String) -> Self {
        let cursor = initial.len();
        Self {
            input: initial,
            cursor,
        }
    }

    fn empty() -> Self {
        Self {
            input: String::new(),
            cursor: 0,
        }
    }

    fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.input.remove(self.cursor - 1);
            self.cursor -= 1;
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.input.len() {
            self.input.remove(self.cursor);
        }
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor += 1;
        }
    }

    fn home(&mut self) {
        self.cursor = 0;
    }

    fn end(&mut self) {
        self.cursor = self.input.len();
    }
}

struct RenameModal {
    group_index: usize,
    original_name: String,
    text: TextInput,
}

struct SearchState {
    text: TextInput,
}

pub struct SettingsState {
    rows: Vec<RowKind>,
    pub selected: usize,
    expanded: HashSet<usize>,
    merge_first: Option<usize>,
    rename_modal: Option<RenameModal>,
    search: Option<SearchState>,
    confirm_reset: bool,
    pub tick: usize,
}

impl SettingsState {
    pub fn new(groups: &[ProjectGroup]) -> Self {
        let mut s = Self {
            rows: Vec::new(),
            selected: 0,
            expanded: HashSet::new(),
            merge_first: None,
            rename_modal: None,
            search: None,
            confirm_reset: false,
            tick: 0,
        };
        s.rebuild_rows(groups);
        s
    }

    pub fn with_selected(groups: &[ProjectGroup], selected: usize) -> Self {
        let mut s = Self::new(groups);
        s.selected = selected.min(s.rows.len().saturating_sub(1));
        s
    }

    fn rebuild_rows(&mut self, groups: &[ProjectGroup]) {
        self.rows.clear();
        let filter = self.search.as_ref().map(|s| s.text.input.to_lowercase());
        for (gi, group) in groups.iter().enumerate() {
            if let Some(ref q) = filter
                && !q.is_empty()
                && !group.name.to_lowercase().contains(q.as_str())
            {
                continue;
            }
            self.rows.push(RowKind::Group(gi));
            if self.expanded.contains(&gi) {
                for si in 0..group.sources.len() {
                    self.rows.push(RowKind::Source(gi, si));
                }
            }
        }
        if self.selected >= self.rows.len() && !self.rows.is_empty() {
            self.selected = self.rows.len() - 1;
        }
    }

    /// Handle a key event.
    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        groups: &[ProjectGroup],
        overrides: &mut Overrides,
    ) -> KeyResult {
        if key.kind != KeyEventKind::Press {
            return KeyResult::Continue;
        }

        // Handle confirm reset
        if self.confirm_reset {
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    overrides.reset_all();
                    overrides.save();
                    self.confirm_reset = false;
                    return KeyResult::Rebuild;
                }
                _ => {
                    self.confirm_reset = false;
                }
            }
            return KeyResult::Continue;
        }

        // Handle search input
        if let Some(search) = &mut self.search {
            match key.code {
                KeyCode::Esc => {
                    self.search = None;
                    self.rebuild_rows(groups);
                }
                KeyCode::Enter => {
                    self.search = None;
                }
                KeyCode::Backspace => {
                    search.text.backspace();
                    self.selected = 0;
                    self.rebuild_rows(groups);
                }
                KeyCode::Left => search.text.move_left(),
                KeyCode::Right => search.text.move_right(),
                KeyCode::Up => {
                    if self.selected > 0 {
                        self.selected -= 1;
                    }
                }
                KeyCode::Down => {
                    if self.selected + 1 < self.rows.len() {
                        self.selected += 1;
                    }
                }
                KeyCode::Char(c) => {
                    search.text.insert_char(c);
                    self.selected = 0;
                    self.rebuild_rows(groups);
                }
                _ => {}
            }
            return KeyResult::Continue;
        }

        // Handle rename modal input
        if let Some(modal) = &mut self.rename_modal {
            match key.code {
                KeyCode::Esc => {
                    self.rename_modal = None;
                }
                KeyCode::Enter => {
                    let gi = modal.group_index;
                    let new_name = modal.text.input.trim().to_string();
                    let root_key = groups[gi].root_key();
                    overrides.rename(&root_key, &new_name);
                    overrides.save();
                    self.rename_modal = None;
                    return KeyResult::Rebuild;
                }
                KeyCode::Backspace => modal.text.backspace(),
                KeyCode::Delete => modal.text.delete(),
                KeyCode::Left => modal.text.move_left(),
                KeyCode::Right => modal.text.move_right(),
                KeyCode::Home => modal.text.home(),
                KeyCode::End => modal.text.end(),
                KeyCode::Char(c) => modal.text.insert_char(c),
                _ => {}
            }
            return KeyResult::Continue;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('.') => {
                if self.merge_first.is_some() {
                    self.merge_first = None;
                } else {
                    return KeyResult::Close;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected + 1 < self.rows.len() {
                    self.selected += 1;
                }
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Left => {
                if let Some(&RowKind::Group(gi)) = self.rows.get(self.selected) {
                    if self.expanded.contains(&gi) {
                        self.expanded.remove(&gi);
                    } else if groups[gi].sources.len() > 1 {
                        self.expanded.insert(gi);
                    }
                    self.rebuild_rows(groups);
                }
            }
            KeyCode::Char('m') => {
                if let Some(&RowKind::Group(gi)) = self.rows.get(self.selected) {
                    if let Some(first) = self.merge_first {
                        if first != gi && first < groups.len() && gi < groups.len() {
                            let a = groups[first].root_key();
                            let b = groups[gi].root_key();
                            overrides.add_merge(&a, &b);
                            overrides.save();
                            self.merge_first = None;
                            return KeyResult::Rebuild;
                        } else {
                            self.merge_first = None;
                        }
                    } else {
                        self.merge_first = Some(gi);
                    }
                }
            }
            KeyCode::Char('s') => match self.rows.get(self.selected) {
                Some(&RowKind::Group(gi)) => {
                    if groups[gi].sources.len() > 1 {
                        let key = groups[gi].root_key();
                        overrides.add_split(&key);
                        overrides.save();
                        return KeyResult::Rebuild;
                    }
                }
                Some(&RowKind::Source(gi, si)) => {
                    if groups[gi].sources.len() > 1
                        && let Some(cwd) = &groups[gi].sources[si].cwd
                    {
                        overrides.extract_source(cwd);
                        overrides.save();
                        return KeyResult::Rebuild;
                    }
                }
                None => {}
            },
            KeyCode::Char('r') => {
                if let Some(&RowKind::Group(gi)) = self.rows.get(self.selected) {
                    let group = &groups[gi];
                    match &group.override_info {
                        Some(OverrideInfo::Split { original_root }) => {
                            let key = original_root.to_string_lossy().to_string();
                            overrides.remove_overrides_for(&key);
                            overrides.save();
                            return KeyResult::Rebuild;
                        }
                        Some(OverrideInfo::Merged) => {
                            let key = group.root_key();
                            overrides.remove_overrides_for(&key);
                            overrides.save();
                            return KeyResult::Rebuild;
                        }
                        None => {}
                    }
                }
            }
            KeyCode::Char('f') => {
                if let Some(&RowKind::Group(gi)) = self.rows.get(self.selected) {
                    let key = groups[gi].root_key();
                    overrides.toggle_star(&key);
                    overrides.save();
                    return KeyResult::Rebuild;
                }
            }
            KeyCode::Char('v') => {
                if let Some(&RowKind::Group(gi)) = self.rows.get(self.selected) {
                    let key = groups[gi].root_key();
                    overrides.toggle_hidden(&key);
                    overrides.save();
                    return KeyResult::Rebuild;
                }
            }
            KeyCode::Char('n') => {
                if let Some(&RowKind::Group(gi)) = self.rows.get(self.selected) {
                    let original = discovery::derive_group_name(&groups[gi].root_path);
                    let current_name = groups[gi].name.clone();
                    self.rename_modal = Some(RenameModal {
                        group_index: gi,
                        original_name: original,
                        text: TextInput::new(current_name),
                    });
                }
            }
            KeyCode::Char('/') => {
                self.search = Some(SearchState {
                    text: TextInput::empty(),
                });
            }
            KeyCode::Char('R') => {
                self.confirm_reset = true;
            }
            _ => {}
        }
        KeyResult::Continue
    }

    pub fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        groups: &[ProjectGroup],
        overrides: &Overrides,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(2)])
            .split(area);

        self.render_list(frame, chunks[0], groups, overrides);

        if self.search.is_some() {
            self.render_search_bar(frame, chunks[1]);
        } else {
            self.render_status_bar(frame, chunks[1], groups, overrides);
        }

        if let Some(modal) = &self.rename_modal {
            self.render_rename_modal(frame, area, modal);
        }

        if self.confirm_reset {
            self.render_confirm_reset(frame, area);
        }
    }

    fn render_search_bar(&self, frame: &mut Frame, area: Rect) {
        let t = theme();
        let Some(search) = self.search.as_ref() else {
            return;
        };
        let bar = Paragraph::new(Line::from(vec![
            Span::styled(
                " / ",
                Style::default().fg(t.duration).add_modifier(Modifier::BOLD),
            ),
            Span::raw(&search.text.input),
        ]))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(t.duration)),
        );
        frame.render_widget(bar, area);

        let cursor_x = area.x + 3 + search.text.cursor as u16;
        let cursor_y = area.y + 1;
        frame.set_cursor_position((cursor_x, cursor_y));
    }

    fn render_confirm_reset(&self, frame: &mut Frame, area: Rect) {
        let t = theme();
        let modal_width = 40u16.min(area.width.saturating_sub(4));
        let modal_height = 5u16;
        let x = area.x + (area.width.saturating_sub(modal_width)) / 2;
        let y = area.y + (area.height.saturating_sub(modal_height)) / 2;
        let modal_area = Rect::new(x, y, modal_width, modal_height);

        frame.render_widget(Clear, modal_area);

        let block = Block::default()
            .title(" Reset all overrides ")
            .title_style(
                Style::default()
                    .fg(t.lines_negative)
                    .add_modifier(Modifier::BOLD),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.lines_negative))
            .border_type(ratatui::widgets::BorderType::Rounded);

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let text = Paragraph::new(Line::from("Are you sure?")).alignment(Alignment::Center);
        frame.render_widget(text, Rect::new(inner.x, inner.y, inner.width, 1));

        let hints = Paragraph::new(Line::from(vec![
            Span::styled(
                " y ",
                Style::default()
                    .fg(t.lines_negative)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("Confirm   "),
            Span::styled(
                " Esc ",
                Style::default()
                    .fg(t.lines_negative)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("Cancel"),
        ]))
        .alignment(Alignment::Center);
        frame.render_widget(hints, Rect::new(inner.x, inner.y + 2, inner.width, 1));
    }

    fn render_rename_modal(&self, frame: &mut Frame, area: Rect, modal: &RenameModal) {
        let t = theme();
        let modal_width = 56u16.min(area.width.saturating_sub(4));
        let modal_height = 7u16;
        let x = area.x + (area.width.saturating_sub(modal_width)) / 2;
        let y = area.y + (area.height.saturating_sub(modal_height)) / 2;
        let modal_area = Rect::new(x, y, modal_width, modal_height);

        frame.render_widget(Clear, modal_area);

        let block = Block::default()
            .title(" Rename project ")
            .title_style(Style::default().fg(t.duration).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.duration))
            .border_type(ratatui::widgets::BorderType::Rounded);

        let inner = block.inner(modal_area);
        frame.render_widget(block, modal_area);

        let label_area = Rect::new(inner.x, inner.y, inner.width, 1);
        let original = truncate_str(&modal.original_name, inner.width as usize);
        let label = Paragraph::new(Line::from(vec![
            Span::styled("was: ", Style::default().fg(t.text_dim)),
            Span::styled(
                original,
                Style::default()
                    .fg(t.text_dim)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));
        frame.render_widget(label, label_area);

        let input_area = Rect::new(inner.x, inner.y + 2, inner.width, 1);
        let input_width = input_area.width as usize;

        let display = if modal.text.input.len() <= input_width {
            format!("{:<width$}", modal.text.input, width = input_width)
        } else {
            let start = modal
                .text
                .cursor
                .saturating_sub(input_width.saturating_sub(1));
            let end = modal.text.input.len().min(start + input_width);
            format!(
                "{:<width$}",
                &modal.text.input[start..end],
                width = input_width
            )
        };
        let cursor_x = if modal.text.input.len() <= input_width {
            modal.text.cursor
        } else {
            modal.text.cursor
                - modal
                    .text
                    .cursor
                    .saturating_sub(input_width.saturating_sub(1))
        };

        let input = Paragraph::new(Span::styled(
            display,
            Style::default().bg(t.text_dim).fg(t.text_primary),
        ));
        frame.render_widget(input, input_area);
        frame.set_cursor_position((input_area.x + cursor_x as u16, input_area.y));

        let hints_area = Rect::new(inner.x, inner.y + 4, inner.width, 1);
        let hints = Paragraph::new(Line::from(vec![
            Span::styled(
                " Enter ",
                Style::default().fg(t.duration).add_modifier(Modifier::BOLD),
            ),
            Span::raw("Confirm   "),
            Span::styled(
                " Esc ",
                Style::default().fg(t.duration).add_modifier(Modifier::BOLD),
            ),
            Span::raw("Cancel"),
        ]))
        .alignment(Alignment::Center);
        frame.render_widget(hints, hints_area);
    }

    fn render_list(
        &self,
        frame: &mut Frame,
        area: Rect,
        groups: &[ProjectGroup],
        overrides: &Overrides,
    ) {
        let t = theme();
        let title = if self.merge_first.is_some() {
            " Settings — MERGE: select 2nd project "
        } else {
            " Settings — Projects "
        };

        let content_width = area.width.saturating_sub(4) as usize; // borders + padding

        let items: Vec<ListItem> = self
            .rows
            .iter()
            .map(|row| match row {
                RowKind::Group(gi) => {
                    let group = &groups[*gi];
                    let is_merge_first = self.merge_first == Some(*gi);
                    self.render_group_row(group, *gi, is_merge_first, content_width, overrides)
                }
                RowKind::Source(gi, si) => {
                    let group = &groups[*gi];
                    let source = &group.sources[*si];
                    self.render_source_row(source, &group.root_path, content_width)
                }
            })
            .collect();

        let mut list_state = ListState::default().with_selected(Some(self.selected));

        let list = List::new(items)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(if self.merge_first.is_some() {
                        Style::default().fg(t.warning)
                    } else {
                        Style::default()
                    }),
            )
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        frame.render_stateful_widget(list, area, &mut list_state);
    }

    fn render_group_row(
        &self,
        group: &ProjectGroup,
        gi: usize,
        is_merge_first: bool,
        width: usize,
        overrides: &Overrides,
    ) -> ListItem<'static> {
        let root_key = group.root_key();
        let starred = overrides.is_starred(&root_key);
        let hidden = overrides.is_hidden(&root_key);

        let expanded = self.expanded.contains(&gi);

        // Fixed-width prefix: star(1) + hidden(1) + arrow(1) + space(1) = 4 display columns
        let star_ch = if starred { '\u{2605}' } else { ' ' }; // ★ or space
        let hidden_ch = if hidden { '\u{2298}' } else { ' ' }; // ⊘ or space
        let arrow_ch = if group.sources.len() > 1 {
            if expanded { '\u{25BE}' } else { '\u{25B8}' } // ▾ or ▸
        } else {
            ' '
        };
        let prefix = format!("{star_ch}{hidden_ch}{arrow_ch} "); // 4 display columns

        // Sessions always right-aligned, tags right after the name
        let sessions_str = format!("{:>5} sessions", group.total_sessions);

        let override_tag: &str = match &group.override_info {
            Some(OverrideInfo::Split { .. }) => " [split]",
            Some(OverrideInfo::Merged) => " [merged]",
            None => "",
        };
        // Layout: prefix(4) | name | tags | pad... | sessions
        let tags_len = override_tag.len();
        let right_len = 2 + sessions_str.len(); // "  XXXXX sessions"
        let name_max = width.saturating_sub(4 + tags_len + right_len).max(10);
        let name = truncate_str(&group.name, name_max);

        // Padding fills the gap between name+tags and sessions
        let used = 4 + name.len() + tags_len + right_len;
        let pad = width.saturating_sub(used);

        let dim = if hidden {
            Modifier::DIM
        } else {
            Modifier::empty()
        };

        let t = theme();
        let mut spans: Vec<Span<'static>> = Vec::new();

        if starred {
            spans.push(Span::styled(
                prefix,
                Style::default().fg(t.warning).add_modifier(Modifier::BOLD),
            ));
            let rainbow = &t.rainbow;
            for (i, ch) in name.chars().enumerate() {
                let color = rainbow[(self.tick + i) % rainbow.len()];
                spans.push(Span::styled(
                    String::from(ch),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
            }
        } else {
            spans.push(Span::styled(
                prefix,
                Style::default().fg(t.warning).add_modifier(dim),
            ));
            spans.push(Span::styled(
                name,
                Style::default().add_modifier(Modifier::BOLD | dim),
            ));
        }

        if let Some(info) = &group.override_info {
            let (tag, color) = match info {
                OverrideInfo::Split { .. } => (override_tag, t.duration),
                OverrideInfo::Merged => (override_tag, t.warning),
            };
            spans.push(Span::styled(
                tag.to_string(),
                Style::default().fg(color).add_modifier(dim),
            ));
        }

        spans.push(Span::styled(
            format!("{:>width$}", sessions_str, width = pad + sessions_str.len()),
            Style::default().add_modifier(dim),
        ));

        let mut item = ListItem::new(Line::from(spans));
        if is_merge_first {
            item = item.style(Style::default().fg(t.warning));
        }
        item
    }

    fn render_source_row(
        &self,
        source: &crate::config::discovery::ProjectSource,
        group_root: &std::path::Path,
        width: usize,
    ) -> ListItem<'static> {
        let display_name = match &source.cwd {
            Some(cwd) => {
                let cwd_path = std::path::Path::new(cwd);
                cwd_path
                    .strip_prefix(group_root)
                    .ok()
                    .and_then(|rel| rel.to_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("./{s}"))
                    .unwrap_or_else(|| cwd.clone())
            }
            None => source.dir_name.clone(),
        };
        let files_str = format!("{:>4} sessions", source.session_files.len());

        // "    \u{2514} " = 6 display columns (4 spaces + └ + space)
        let prefix = "    \u{2514} ";
        let prefix_cols = 6;
        let right_len = 2 + files_str.len();
        let name_width = width.saturating_sub(prefix_cols + right_len).max(10);
        let display = truncate_str(&display_name, name_width);
        let padded = format!("{:<width$}", display, width = name_width);

        let line = Line::from(vec![
            Span::raw(prefix.to_string()),
            Span::styled(padded, Style::default().fg(theme().text_dim)),
            Span::raw(format!("  {files_str}")),
        ]);

        ListItem::new(line)
    }

    fn render_status_bar(
        &self,
        frame: &mut Frame,
        area: Rect,
        groups: &[ProjectGroup],
        overrides: &Overrides,
    ) {
        let hints = if self.merge_first.is_some() {
            vec![
                Span::styled(" m ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("Confirm merge   "),
                Span::styled(" Esc ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("Cancel"),
            ]
        } else {
            let mut h = vec![
                Span::styled(" ↑↓ ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("Navigate   "),
            ];

            match self.rows.get(self.selected) {
                Some(&RowKind::Group(gi)) if gi < groups.len() => {
                    let group = &groups[gi];
                    h.push(Span::styled(
                        " Enter ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    h.push(Span::raw("Expand   "));
                    h.push(Span::styled(
                        " m ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    h.push(Span::raw("Merge   "));
                    if group.sources.len() > 1 {
                        h.push(Span::styled(
                            " s ",
                            Style::default().add_modifier(Modifier::BOLD),
                        ));
                        h.push(Span::raw("Split all   "));
                    }
                    if group.override_info.is_some() {
                        h.push(Span::styled(
                            " r ",
                            Style::default().add_modifier(Modifier::BOLD),
                        ));
                        h.push(Span::raw("Reset   "));
                    }
                    let key = group.root_key();
                    let star_label = if overrides.is_starred(&key) {
                        "Unstar"
                    } else {
                        "Star"
                    };
                    let vis_label = if overrides.is_hidden(&key) {
                        "Show"
                    } else {
                        "Hide"
                    };
                    h.push(Span::styled(
                        " f ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    h.push(Span::raw(format!("{star_label}   ")));
                    h.push(Span::styled(
                        " v ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    h.push(Span::raw(format!("{vis_label}   ")));
                    h.push(Span::styled(
                        " n ",
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    h.push(Span::raw("Rename   "));
                }
                Some(&RowKind::Source(gi, _)) if gi < groups.len() => {
                    if groups[gi].sources.len() > 1 {
                        h.push(Span::styled(
                            " s ",
                            Style::default().add_modifier(Modifier::BOLD),
                        ));
                        h.push(Span::raw("Extract   "));
                    }
                }
                _ => {}
            }

            h.push(Span::styled(
                " / ",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            h.push(Span::raw("Search   "));
            h.push(Span::styled(
                " R ",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            h.push(Span::raw("Reset all   "));
            h.push(Span::styled(
                " . ",
                Style::default().add_modifier(Modifier::BOLD),
            ));
            h.push(Span::raw("Back"));
            h
        };

        let bar = Paragraph::new(Line::from(hints)).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme().text_dim)),
        );
        frame.render_widget(bar, area);
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else if max > 1 {
        format!("{}…", &s[..max - 1])
    } else {
        s[..max].to_string()
    }
}
