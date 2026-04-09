use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use super::heatmap::fill_cell;
use super::theme::theme;
use crate::config::discovery::{self, OverrideInfo, ProjectGroup};
use crate::config::overrides::Overrides;
use crate::config::settings::Settings;

#[derive(Debug, Clone, Copy, PartialEq)]
/// Settings view is split into tabs, cycled with the Tab key.
pub enum SettingsTab {
    Projects,
    Display,
}

impl SettingsTab {
    const ALL: &'static [SettingsTab] = &[SettingsTab::Projects, SettingsTab::Display];

    fn label(self) -> &'static str {
        match self {
            Self::Projects => "Projects",
            Self::Display => "Display",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Projects => Self::Display,
            Self::Display => Self::Projects,
        }
    }
}

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
        self.cursor += c.len_utf8();
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            let prev = self.input[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input.remove(prev);
            self.cursor = prev;
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.input.len() {
            self.input.remove(self.cursor);
        }
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.input[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor = self.input[self.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor + i)
                .unwrap_or(self.input.len());
        }
    }

    fn home(&mut self) {
        self.cursor = 0;
    }

    fn end(&mut self) {
        self.cursor = self.input.len();
    }

    /// Display column position (character count up to the cursor byte offset).
    fn display_col(&self) -> usize {
        self.input[..self.cursor].chars().count()
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
    tab: SettingsTab,
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
            tab: SettingsTab::Projects,
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

    pub fn with_selected(
        groups: &[ProjectGroup],
        selected: usize,
        tab: Option<SettingsTab>,
    ) -> Self {
        let mut s = Self::new(groups);
        s.selected = selected.min(s.rows.len().saturating_sub(1));
        if let Some(t) = tab {
            s.tab = t;
        }
        s
    }

    pub fn active_tab(&self) -> SettingsTab {
        self.tab
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
        settings: &mut Settings,
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

        // Tab switching (available on all tabs)
        if key.code == KeyCode::Tab {
            self.tab = self.tab.next();
            return KeyResult::Continue;
        }

        match self.tab {
            SettingsTab::Projects => self.handle_projects_key(key, groups, overrides),
            SettingsTab::Display => self.handle_display_key(key, settings),
        }
    }

    fn handle_projects_key(
        &mut self,
        key: KeyEvent,
        groups: &[ProjectGroup],
        overrides: &mut Overrides,
    ) -> KeyResult {
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

    fn handle_display_key(&mut self, key: KeyEvent, settings: &mut Settings) -> KeyResult {
        match key.code {
            KeyCode::Esc | KeyCode::Char('.') => return KeyResult::Close,
            KeyCode::Left | KeyCode::Right => {
                settings.expanded_heatmap = !settings.expanded_heatmap;
                settings.save();
                return KeyResult::Rebuild;
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
        settings: &Settings,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(2),
            ])
            .split(area);

        self.render_tab_bar(frame, chunks[0]);

        match self.tab {
            SettingsTab::Projects => {
                self.render_list(frame, chunks[1], groups, overrides);
                if self.search.is_some() {
                    self.render_search_bar(frame, chunks[2]);
                } else {
                    self.render_status_bar(frame, chunks[2], groups, overrides);
                }
            }
            SettingsTab::Display => {
                self.render_display_tab(frame, chunks[1], settings);
                self.render_display_status_bar(frame, chunks[2]);
            }
        }

        if let Some(modal) = &self.rename_modal {
            self.render_rename_modal(frame, area, modal);
        }

        if self.confirm_reset {
            self.render_confirm_reset(frame, area);
        }
    }

    fn render_tab_bar(&self, frame: &mut Frame, area: Rect) {
        let t = theme();
        let mut spans = Vec::new();
        for (i, tab) in SettingsTab::ALL.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", Style::default().fg(t.divider)));
            }
            if *tab == self.tab {
                spans.push(Span::styled(
                    tab.label(),
                    Style::default()
                        .fg(t.heatmap_title)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                ));
            } else {
                spans.push(Span::styled(tab.label(), Style::default().fg(t.text_dim)));
            }
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
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

        let cursor_x = area.x + 3 + search.text.display_col() as u16;
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
        let char_count = modal.text.input.chars().count();
        let cursor_chars = modal.text.display_col();

        let display = if char_count <= input_width {
            format!("{:<width$}", modal.text.input, width = input_width)
        } else {
            let start_chars = cursor_chars.saturating_sub(input_width.saturating_sub(1));
            let visible: String = modal
                .text
                .input
                .chars()
                .skip(start_chars)
                .take(input_width)
                .collect();
            format!("{:<width$}", visible, width = input_width)
        };
        let cursor_x = if char_count <= input_width {
            cursor_chars
        } else {
            cursor_chars - cursor_chars.saturating_sub(input_width.saturating_sub(1))
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
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .border_style(if self.merge_first.is_some() {
                        Style::default().fg(t.warning)
                    } else {
                        Style::default().fg(t.border)
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

    fn render_display_tab(&self, frame: &mut Frame, area: Rect, settings: &Settings) {
        let t = theme();
        let is_expanded = settings.expanded_heatmap;

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(6),
                Constraint::Min(0),
            ])
            .split(area);

        // --- Heatmap mode selector (←→ to switch) ---
        let label = Line::from(vec![
            Span::styled("  Heatmap mode: ", Style::default().fg(t.text_dim)),
            Span::styled(
                if is_expanded { "Expanded" } else { "Compact" },
                Style::default()
                    .fg(t.heatmap_title)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        let setting_block = Paragraph::new(vec![Line::raw(""), label]);
        frame.render_widget(setting_block, chunks[0]);

        // --- Inline preview ---
        if chunks[1].width >= 30 {
            let cols_area = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(chunks[1]);

            self.render_mini_heatmap(frame, cols_area[0], !is_expanded, false);
            self.render_mini_heatmap(frame, cols_area[1], is_expanded, true);
        }
    }

    fn render_mini_heatmap(&self, frame: &mut Frame, area: Rect, is_active: bool, expanded: bool) {
        let t = theme();

        let label = if expanded { "Expanded" } else { "Compact" };
        let (border_color, title_style) = if is_active {
            (
                t.heatmap_title,
                Style::default()
                    .fg(t.heatmap_title)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            (t.border, Style::default().fg(t.text_dim))
        };

        let mut title_spans = vec![Span::styled(format!(" {} ", label), title_style)];
        if is_active {
            title_spans.push(Span::styled(
                " \u{25c0} active ",
                Style::default()
                    .fg(t.heatmap_title)
                    .add_modifier(Modifier::DIM),
            ));
        }

        let block = Block::default()
            .title(Line::from(title_spans))
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if inner.width < 4 || inner.height < 2 {
            return;
        }

        let sample_cols: u16 = 7;
        let sample_rows: u16 = inner.height.min(4);
        let cell_w: u16 = if expanded {
            (inner.width / sample_cols).max(2)
        } else {
            2
        };
        let fill_w: u16 = if expanded { cell_w } else { 1 };
        let grid_w = sample_cols * cell_w;
        let left_pad = if expanded {
            (inner.width.saturating_sub(grid_w)) / 2
        } else {
            1
        };

        let pattern: &[&[u8]] = &[
            &[0, 1, 2, 3, 2, 1, 0],
            &[1, 2, 3, 4, 3, 2, 1],
            &[0, 1, 3, 4, 4, 2, 0],
            &[1, 2, 2, 3, 3, 1, 0],
        ];
        let colors = &t.input_colors;

        let buf = frame.buffer_mut();
        for row in 0..sample_rows as usize {
            let y = inner.y + row as u16;
            if y >= buf.area().bottom() {
                break;
            }
            let pat_row = &pattern[row % pattern.len()];
            for col in 0..sample_cols as usize {
                let cx = inner.x + left_pad + col as u16 * cell_w;
                if cx >= buf.area().right() {
                    break;
                }
                let level = pat_row[col % pat_row.len()] as usize;
                if level == 0 {
                    fill_cell(buf, cx, y, fill_w, "\u{00b7}", t.dot_empty);
                } else {
                    fill_cell(buf, cx, y, fill_w, "\u{2580}", colors[level.min(4)]);
                }
            }
        }
    }

    fn render_display_status_bar(&self, frame: &mut Frame, area: Rect) {
        let hints = vec![
            Span::styled(" ←→ ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("Switch mode   "),
            Span::styled(" Tab ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("Switch tab   "),
            Span::styled(" . ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("Back"),
        ];
        render_hint_bar(frame, area, hints);
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

        render_hint_bar(frame, area, hints);
    }
}

fn render_hint_bar(frame: &mut Frame, area: Rect, hints: Vec<Span<'_>>) {
    let bar = Paragraph::new(Line::from(hints))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme().divider)),
        );
    frame.render_widget(bar, area);
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return if max == 1 {
            "…".to_string()
        } else {
            String::new()
        };
    }
    // Find the last valid char boundary at or before (max - 1) bytes
    // to leave room for the '…' suffix.
    let target = max - 1;
    let mut boundary = target.min(s.len());
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}…", &s[..boundary])
}
