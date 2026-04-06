use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use chrono::{Local, NaiveDate};
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

use crate::config::discovery;
use crate::config::overrides::{self, Overrides};
use crate::config::settings::Settings;
use crate::data::cache;
use crate::data::parser;
use crate::data::tokens::{DailyTokens, MinuteTokens};
use crate::ui::cards;
use crate::ui::heatmap;
use crate::ui::settings_view::{KeyResult, SettingsState};
use crate::ui::time_filter::{TimeFilter, date_in_filter, filter_daily};

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

pub(crate) enum View {
    Main,
    Settings(Box<SettingsState>),
}

pub(crate) struct CachedKpi {
    pub(crate) cost: f64,
    pub(crate) streak: u32,
    pub(crate) active: usize,
    pub(crate) total_days: usize,
    pub(crate) avg: u64,
    pub(crate) efficiency: f64,
}

impl CachedKpi {
    fn compute(filtered: &DailyTokens) -> Self {
        let (active, total_days) = filtered.active_and_total_days();
        Self {
            cost: filtered.total_cost(),
            streak: filtered.current_streak(),
            active,
            total_days,
            avg: filtered.avg_tokens_per_active_day(),
            efficiency: filtered.avg_efficiency(),
        }
    }
}

type ReloadResult = (cache::Cache, Vec<parser::Event>);

// ---------------------------------------------------------------------------
// Sub-structs for logical grouping
// ---------------------------------------------------------------------------

/// Raw parsed data (cache + events + derived time-series).
pub(crate) struct AppData {
    pub(crate) merged_cache: cache::Cache,
    pub(crate) all_events: Vec<parser::Event>,
    pub(crate) daily_tokens: DailyTokens,
    pub(crate) thresholds: heatmap::Thresholds,
    pub(crate) minute_tokens: MinuteTokens,
}

/// Project configuration (discovery results + user overrides).
pub(crate) struct AppConfig {
    pub(crate) overrides: Overrides,
    pub(crate) settings: Settings,
    pub(crate) groups: Vec<discovery::ProjectGroup>,
    pub(crate) raw_groups: Arc<Vec<discovery::ProjectGroup>>,
    pub(crate) session_map: Arc<HashMap<String, (String, String)>>,
    pub(crate) source_names: Vec<String>,
    pub(crate) source_roots: Vec<Option<String>>,
}

/// Pre-computed values for the current view (rebuilt when filters change).
pub(crate) struct RenderCache {
    pub(crate) filtered: DailyTokens,
    pub(crate) kpi: CachedKpi,
    pub(crate) minute_model: HashMap<(String, String), HashMap<(NaiveDate, u16), f64>>,
    pub(crate) cards: Vec<cards::ProjectCard>,
    pub(crate) range: (NaiveDate, NaiveDate),
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub(crate) struct App {
    pub(crate) data: AppData,
    pub(crate) config: AppConfig,
    pub(crate) render: RenderCache,

    pub(crate) view: View,
    pub(crate) time_filter: TimeFilter,
    pub(crate) source_index: usize,
    pub(crate) project_index: Option<usize>,

    pub(crate) render_dirty: bool,
    pub(crate) card_scroll: usize,

    pub(crate) reloading: bool,
    reload_tx: mpsc::Sender<ReloadResult>,
    reload_rx: mpsc::Receiver<ReloadResult>,

    pub(crate) start_time: std::time::Instant,
    pub(crate) last_reload: std::time::Instant,
    pub(crate) reload_interval: Duration,
}

impl App {
    /// Runs `App::new()` on a background thread so the main thread can
    /// render a loading screen while project discovery and JSONL parsing
    /// happen. The `App` is boxed to keep the channel slot small.
    pub(crate) fn spawn_load() -> mpsc::Receiver<Box<App>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(Box::new(App::new()));
        });
        rx
    }

    pub(crate) fn new() -> Self {
        let (raw_groups, root_cwd_map, session_map) =
            discovery::discover_project_groups_with_root_map();
        let raw_groups = Arc::new(raw_groups);
        let session_map = Arc::new(session_map);
        let (merged_cache, all_events) = load_data(&raw_groups, &session_map);

        let (daily_tokens, thresholds) = compute_daily_and_thresholds(&merged_cache, None, None);
        let minute_tokens = cache::build_minute_tokens(&all_events, &session_map, None, None);

        let mut overrides = Overrides::load();
        let groups = overrides::apply_overrides(&raw_groups, &mut overrides);
        let (source_names, source_roots) = build_source_list(&root_cwd_map);
        let source_index: usize = 0;
        let project_index: Option<usize> = None;
        let settings = Settings::load();
        let time_filter = settings.time_filter.unwrap_or(TimeFilter::All);

        let cwds_filter = project_cwds_static(&groups, project_index);

        let render = build_render_cache(
            &daily_tokens,
            &all_events,
            &session_map,
            &groups,
            &merged_cache,
            &overrides,
            source_roots[source_index].as_deref(),
            cwds_filter.as_deref(),
            time_filter,
        );

        let (reload_tx, reload_rx) = mpsc::channel::<ReloadResult>();

        App {
            data: AppData {
                merged_cache,
                all_events,
                daily_tokens,
                thresholds,
                minute_tokens,
            },
            config: AppConfig {
                overrides,
                settings,
                groups,
                raw_groups,
                session_map,
                source_names,
                source_roots,
            },
            render,
            view: View::Main,
            time_filter,
            source_index,
            project_index,
            render_dirty: false,
            card_scroll: 0,
            reloading: false,
            reload_tx,
            reload_rx,
            start_time: std::time::Instant::now(),
            last_reload: std::time::Instant::now(),
            reload_interval: Duration::from_secs(5 * 60),
        }
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn project_cwds(&self) -> Option<Vec<String>> {
        project_cwds_static(&self.config.groups, self.project_index)
    }

    fn recompute_tokens(&mut self) {
        let cwds_filter = self.project_cwds();
        let source_root = self.config.source_roots[self.source_index].as_deref();
        let (d, t) = compute_daily_and_thresholds(
            &self.data.merged_cache,
            source_root,
            cwds_filter.as_deref(),
        );
        self.data.daily_tokens = d;
        self.data.thresholds = t;
        let source_root = self.config.source_roots[self.source_index].as_deref();
        self.data.minute_tokens = cache::build_minute_tokens(
            &self.data.all_events,
            &self.config.session_map,
            source_root,
            cwds_filter.as_deref(),
        );
        self.render_dirty = true;
    }

    fn recompute_render_cache(&mut self) {
        let cwds_filter = self.project_cwds();
        let source_root = self.config.source_roots[self.source_index].as_deref();
        self.render = build_render_cache(
            &self.data.daily_tokens,
            &self.data.all_events,
            &self.config.session_map,
            &self.config.groups,
            &self.data.merged_cache,
            &self.config.overrides,
            source_root,
            cwds_filter.as_deref(),
            self.time_filter,
        );
        self.render_dirty = false;
    }

    // ------------------------------------------------------------------
    // Event loop helpers (called from main)
    // ------------------------------------------------------------------

    pub(crate) fn pre_render(&mut self) {
        if let View::Settings(state) = &mut self.view {
            state.tick = (self.start_time.elapsed().as_millis() / 80) as usize;
        }
        if self.render_dirty {
            self.recompute_render_cache();
        }
    }

    pub(crate) fn handle_reload(&mut self) {
        if self.last_reload.elapsed() >= self.reload_interval && !self.reloading {
            spawn_reload(
                &self.config.raw_groups,
                &self.config.session_map,
                &self.reload_tx,
            );
            self.reloading = true;
            self.last_reload = std::time::Instant::now();
        }

        if self.reloading
            && let Ok((c, e)) = self.reload_rx.try_recv()
        {
            self.data.merged_cache = c;
            self.data.all_events = e;
            self.reloading = false;
            self.recompute_tokens();
        }
    }

    /// Returns false if the app should quit.
    pub(crate) fn handle_input(&mut self, key: crossterm::event::KeyEvent) -> bool {
        if key.kind != KeyEventKind::Press {
            return true;
        }

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }

        // Tab/BackTab cycle time filters globally, except in Settings where Tab switches tabs.
        if !matches!(self.view, View::Settings(_)) {
            match key.code {
                KeyCode::Tab => {
                    self.time_filter = self.time_filter.next();
                    self.config.settings.time_filter = Some(self.time_filter);
                    self.config.settings.save();
                    self.card_scroll = 0;
                    self.render_dirty = true;
                    return true;
                }
                KeyCode::BackTab => {
                    self.source_index = (self.source_index + 1) % self.config.source_names.len();
                    self.card_scroll = 0;
                    self.recompute_tokens();
                    return true;
                }
                _ => {}
            }
        }

        match &mut self.view {
            View::Main => match key.code {
                KeyCode::Esc if self.project_index.is_some() => {
                    self.project_index = None;
                    self.card_scroll = 0;
                    self.recompute_tokens();
                }
                KeyCode::Char('q') => return false,
                KeyCode::Char('r') if !self.reloading => {
                    spawn_reload(
                        &self.config.raw_groups,
                        &self.config.session_map,
                        &self.reload_tx,
                    );
                    self.reloading = true;
                    self.last_reload = std::time::Instant::now();
                }
                KeyCode::Char('.') => {
                    self.view = View::Settings(Box::new(SettingsState::new(&self.config.groups)));
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.card_scroll = self.card_scroll.saturating_add(1);
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.card_scroll = self.card_scroll.saturating_sub(1);
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    self.project_index = match self.project_index {
                        None if !self.config.groups.is_empty() => Some(0),
                        Some(i) if i + 1 < self.config.groups.len() => Some(i + 1),
                        _ => None,
                    };
                    self.card_scroll = 0;
                    self.recompute_tokens();
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    self.project_index = match self.project_index {
                        None if !self.config.groups.is_empty() => {
                            Some(self.config.groups.len() - 1)
                        }
                        Some(0) => None,
                        Some(i) => Some(i - 1),
                        _ => None,
                    };
                    self.card_scroll = 0;
                    self.recompute_tokens();
                }
                _ => {}
            },
            View::Settings(state) => {
                match state.handle_key(
                    key,
                    &self.config.groups,
                    &mut self.config.overrides,
                    &mut self.config.settings,
                ) {
                    KeyResult::Rebuild => {
                        let selected = state.selected;
                        let tick = state.tick;
                        let tab = state.active_tab();
                        self.config.groups = overrides::apply_overrides(
                            &self.config.raw_groups,
                            &mut self.config.overrides,
                        );
                        if let Some(idx) = self.project_index
                            && idx >= self.config.groups.len()
                        {
                            self.project_index = None;
                        }
                        let mut new_state =
                            SettingsState::with_selected(&self.config.groups, selected, Some(tab));
                        new_state.tick = tick;
                        self.view = View::Settings(Box::new(new_state));
                        self.render_dirty = true;
                    }
                    KeyResult::Close => {
                        self.view = View::Main;
                        self.render_dirty = true;
                    }
                    KeyResult::Continue => {}
                }
            }
        }

        true
    }
}

// ---------------------------------------------------------------------------
// Free helper functions
// ---------------------------------------------------------------------------

fn project_cwds_static(
    groups: &[discovery::ProjectGroup],
    idx: Option<usize>,
) -> Option<Vec<String>> {
    idx.map(|i| {
        groups[i]
            .sources
            .iter()
            .filter_map(|s| s.cwd.clone())
            .collect()
    })
}

#[allow(clippy::too_many_arguments)]
fn build_render_cache(
    daily_tokens: &DailyTokens,
    all_events: &[parser::Event],
    session_map: &HashMap<String, (String, String)>,
    groups: &[discovery::ProjectGroup],
    merged_cache: &cache::Cache,
    overrides: &Overrides,
    source_root: Option<&str>,
    project_cwds: Option<&[String]>,
    time_filter: TimeFilter,
) -> RenderCache {
    let filtered = filter_daily(daily_tokens, time_filter);
    let today_snap = Local::now().date_naive();
    let kpi = CachedKpi::compute(&filtered);
    let model_tokens = cards::build_model_tokens(
        all_events,
        session_map,
        groups,
        source_root,
        |d| date_in_filter(d, time_filter, today_snap),
        project_cwds,
    );
    let model_daily = cards::build_model_daily_costs(
        all_events,
        session_map,
        groups,
        source_root,
        |d| date_in_filter(d, time_filter, today_snap),
        project_cwds,
    );
    let minute_model = if time_filter.is_intraday() {
        cards::build_minute_model_costs(
            all_events,
            session_map,
            groups,
            source_root,
            |d| date_in_filter(d, time_filter, today_snap),
            project_cwds,
        )
    } else {
        HashMap::new()
    };
    let cards = cards::build_cards(
        groups,
        merged_cache,
        overrides,
        source_root,
        |d| date_in_filter(d, time_filter, today_snap),
        &model_tokens,
        project_cwds,
        &model_daily,
    );
    let range = compute_range(&filtered, time_filter, today_snap);

    RenderCache {
        filtered,
        kpi,
        minute_model,
        cards,
        range,
    }
}

fn load_data(
    raw_groups: &[discovery::ProjectGroup],
    session_map: &HashMap<String, (String, String)>,
) -> (cache::Cache, Vec<parser::Event>) {
    let all_session_files: Vec<std::path::PathBuf> = raw_groups
        .iter()
        .flat_map(|g| g.sources.iter())
        .flat_map(|s| s.session_files.iter().cloned())
        .collect();
    let events = parser::parse_session_files(&all_session_files);

    let old_cache = cache::load();
    let fresh_cache = cache::from_events(&events, session_map);
    let merged = cache::merge(old_cache, &fresh_cache);
    cache::save(&merged);
    (merged, events)
}

fn spawn_reload(
    raw_groups: &Arc<Vec<discovery::ProjectGroup>>,
    session_map: &Arc<HashMap<String, (String, String)>>,
    tx: &mpsc::Sender<ReloadResult>,
) {
    let raw_groups = Arc::clone(raw_groups);
    let session_map = Arc::clone(session_map);
    let tx = tx.clone();
    std::thread::spawn(move || {
        let result = load_data(&raw_groups, &session_map);
        let _ = tx.send(result);
    });
}

fn compute_daily_and_thresholds(
    cache: &cache::Cache,
    source_root: Option<&str>,
    project_cwds: Option<&[String]>,
) -> (DailyTokens, heatmap::Thresholds) {
    let daily = cache::to_daily_tokens_filtered(cache, source_root, project_cwds);
    let t = heatmap::Thresholds {
        input: heatmap::compute_thresholds(&daily.input),
        output: heatmap::compute_thresholds(&daily.output),
        lines_changed: heatmap::compute_thresholds(&daily.lines_accepted),
    };
    (daily, t)
}

fn compute_range(
    filtered: &DailyTokens,
    tf: TimeFilter,
    today: NaiveDate,
) -> (NaiveDate, NaiveDate) {
    match tf {
        TimeFilter::Hour1 | TimeFilter::Hour12 | TimeFilter::Today => (today, today),
        TimeFilter::LastWeek => (today - chrono::Duration::days(6), today),
        TimeFilter::LastMonth => (today - chrono::Duration::days(29), today),
        TimeFilter::All => {
            let earliest = filtered
                .cost
                .keys()
                .chain(filtered.input.keys())
                .min()
                .copied()
                .unwrap_or(today);
            (earliest, today)
        }
    }
}

fn build_source_list(
    root_map: &HashMap<std::path::PathBuf, std::collections::HashSet<String>>,
) -> (Vec<String>, Vec<Option<String>>) {
    let home = dirs::home_dir().unwrap_or_default();

    if root_map.len() <= 1 {
        return (vec!["All".to_string()], vec![None]);
    }

    let mut roots: Vec<&std::path::PathBuf> = root_map.keys().collect();
    roots.sort();

    let mut names = vec!["All".to_string()];
    let mut root_keys: Vec<Option<String>> = vec![None];

    for root in roots {
        let display: String = root
            .strip_prefix(&home)
            .unwrap_or(root)
            .to_string_lossy()
            .trim_start_matches('/')
            .trim_end_matches("/projects")
            .to_string();
        names.push(display);
        root_keys.push(Some(root.to_string_lossy().to_string()));
    }

    (names, root_keys)
}
