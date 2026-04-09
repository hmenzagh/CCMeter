use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use chrono::{Local, NaiveDate};
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

use crate::config::discovery;
use crate::config::overrides::{self, Overrides};
use crate::config::settings::Settings;
use crate::data::cache;
use crate::data::index::EventIndex;
use crate::data::parser;
use crate::data::oauth::{OAuthCredential, UsagePoller};
use crate::data::hit_history::{self, HitHistory};
use crate::data::rate_history::{self, RateHistory};
use crate::data::rate_limits::RateLimitHit;
use crate::data::tokens::{DailyTokens, MinuteTokens};
use crate::ui::cards;
use crate::ui::heatmap;
use crate::ui::settings_view::{KeyResult, SettingsState};
use crate::ui::time_filter::{TimeFilter, date_in_filter, filter_daily};
use crate::update_check::{self, UpdateInfo};

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

pub(crate) enum View {
    RateTracking,
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

type ReloadResult = (cache::Cache, EventIndex);

// ---------------------------------------------------------------------------
// Sub-structs for logical grouping
// ---------------------------------------------------------------------------

/// Raw parsed data (cache + compact index + derived time-series).
pub(crate) struct AppData {
    pub(crate) merged_cache: cache::Cache,
    pub(crate) index: EventIndex,
    pub(crate) daily_tokens: DailyTokens,
    pub(crate) thresholds: heatmap::Thresholds,
    pub(crate) minute_tokens: MinuteTokens,
    pub(crate) rate_limit_hits: Vec<RateLimitHit>,
    pub(crate) oauth_credentials: Vec<OAuthCredential>,
    pub(crate) rate_history: RateHistory,
    pub(crate) hit_history: HitHistory,
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
    /// Maps display position → index in `config.groups`.
    /// Navigation with ←/→ follows this order so it matches the card rendering order.
    pub(crate) display_order: Vec<usize>,
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
    pub(crate) rate_tracking_selected: usize,

    pub(crate) reloading: bool,
    reload_tx: mpsc::Sender<ReloadResult>,
    reload_rx: mpsc::Receiver<ReloadResult>,

    pub(crate) start_time: std::time::Instant,
    pub(crate) last_reload: std::time::Instant,

    usage_poller: UsagePoller,
    pub(crate) reload_interval: Duration,

    pub(crate) update_info: Option<UpdateInfo>,
    update_rx: mpsc::Receiver<UpdateInfo>,
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
        let (merged_cache, index) = load_data(&raw_groups, &session_map);

        let (daily_tokens, thresholds) = compute_daily_and_thresholds(&merged_cache, None, None);
        let minute_tokens = index.build_minute_tokens(None, None);

        let root_paths: Vec<std::path::PathBuf> = root_cwd_map.keys().cloned().collect();
        let mut fresh_hits = crate::data::rate_limits::discover_rate_limit_hits(&root_paths);
        compute_hit_tokens(&mut fresh_hits, &index);
        let mut hit_history = hit_history::load();
        let rate_limit_hits = hit_history.merge_fresh_hits(&fresh_hits);
        if !fresh_hits.is_empty() {
            hit_history::save(&hit_history);
        }
        let oauth_credentials = crate::data::oauth::discover_credentials_with_usage(&root_paths);
        let rate_history = rate_history::load();

        let mut overrides = Overrides::load();
        let groups = overrides::apply_overrides(&raw_groups, &mut overrides);
        let (source_names, source_roots) = build_source_list(&root_cwd_map);
        let source_index: usize = 0;
        let project_index: Option<usize> = None;
        let settings = Settings::load();
        let time_filter = settings.time_filter.unwrap_or(TimeFilter::All);
        let initial_view = match settings.last_view.as_deref() {
            Some("rate_tracking") => View::RateTracking,
            _ => View::Main, // includes "main" and any unknown value
        };

        let cwds_filter = project_cwds_static(&groups, project_index);

        let render = build_render_cache(
            &daily_tokens,
            &minute_tokens,
            &index,
            &groups,
            &merged_cache,
            &overrides,
            source_roots[source_index].as_deref(),
            cwds_filter.as_deref(),
            time_filter,
        );

        let (reload_tx, reload_rx) = mpsc::channel::<ReloadResult>();
        let usage_poller = UsagePoller::new(&oauth_credentials);
        let update_rx = update_check::spawn_check();

        let mut app = App {
            data: AppData {
                merged_cache,
                index,
                daily_tokens,
                thresholds,
                minute_tokens,
                rate_limit_hits,
                oauth_credentials,
                rate_history,
                hit_history,
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
            view: initial_view,
            time_filter,
            source_index,
            project_index,
            render_dirty: false,
            card_scroll: 0,
            rate_tracking_selected: 0,
            reloading: false,
            reload_tx,
            reload_rx,
            start_time: std::time::Instant::now(),
            last_reload: std::time::Instant::now(),
            usage_poller,
            reload_interval: Duration::from_secs(5 * 60),
            update_info: None,
            update_rx,
        };
        app.record_rate_history();
        app
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn project_cwds(&self) -> Option<Vec<String>> {
        let group_index = self.project_index.map(|i| self.render.display_order[i]);
        project_cwds_static(&self.config.groups, group_index)
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
        self.data.minute_tokens = self
            .data
            .index
            .build_minute_tokens(source_root, cwds_filter.as_deref());
        self.render_dirty = true;
    }

    fn recompute_render_cache(&mut self) {
        let cwds_filter = self.project_cwds();
        let source_root = self.config.source_roots[self.source_index].as_deref();
        let prev_display_order = std::mem::take(&mut self.render.display_order);
        self.render = build_render_cache(
            &self.data.daily_tokens,
            &self.data.minute_tokens,
            &self.data.index,
            &self.config.groups,
            &self.data.merged_cache,
            &self.config.overrides,
            source_root,
            cwds_filter.as_deref(),
            self.time_filter,
        );
        // When viewing a single project, build_render_cache produces a
        // display_order with only one entry. Preserve the full ordering
        // so that ←/→ navigation keeps working.
        if self.project_index.is_some() {
            self.render.display_order = prev_display_order;
        }
        self.render_dirty = false;
    }

    /// Persist estimated token capacity for active 5h sessions into rate-history.
    fn record_rate_history(&mut self) {
        let today = Local::now().date_naive();
        let mut changed = false;

        for cred in &self.data.oauth_credentials {
            let source_root = cred.source_root.to_string_lossy().to_string();
            let Some(usage) = &cred.usage else {
                continue;
            };
            let Some(five_h) = &usage.five_hour else {
                continue;
            };
            let Some(resets_at_str) = &five_h.resets_at else {
                continue;
            };
            let Ok(resets_at) = chrono::DateTime::parse_from_rfc3339(resets_at_str) else {
                continue;
            };

            let utilization = five_h.utilization;
            if utilization <= 0.0 {
                continue;
            }

            let resets_utc = resets_at.with_timezone(&chrono::Utc);
            let session_start_utc = resets_utc - chrono::Duration::hours(5);
            let now_utc = chrono::Utc::now();
            let start_local = session_start_utc
                .with_timezone(&chrono::Local)
                .naive_local();
            let end_local = now_utc
                .with_timezone(&chrono::Local)
                .naive_local();
            let tokens = self.data.index.tokens_in_window(&source_root, start_local, end_local);
            if tokens == 0 {
                continue;
            }

            let estimated_tokens = (tokens as f64 / (utilization / 100.0)).round() as u64;
            self.data.rate_history.record(
                &source_root,
                resets_at_str,
                estimated_tokens,
                today,
            );
            changed = true;
        }

        if changed {
            rate_history::save(&self.data.rate_history);
        }
    }

    // ------------------------------------------------------------------
    // Event loop helpers (called from main)
    // ------------------------------------------------------------------

    pub(crate) fn pre_render(&mut self) {
        if let View::Settings(state) = &mut self.view {
            state.tick = (self.start_time.elapsed().as_millis() / 80) as usize;
        }
        if self.update_info.is_none()
            && let Ok(info) = self.update_rx.try_recv()
        {
            self.update_info = Some(info);
        }
        if self.render_dirty {
            self.recompute_render_cache();
        }
    }

    fn switch_view(&mut self, view: View) {
        let key = match &view {
            View::RateTracking => "rate_tracking",
            View::Main => "main",
            View::Settings(_) => return,
        };
        self.view = view;
        self.render_dirty = true;
        self.config.settings.last_view = Some(key.to_string());
        self.config.settings.save();
    }

    pub(crate) fn handle_reload(&mut self) {
        // Poll usage API for each credential on its own random timer.
        // A JSONL reload is triggered only after a successful usage poll.
        let usage_updated = self.usage_poller
            .poll(&mut self.data.oauth_credentials);

        if usage_updated
            && !self.reloading
            && self.last_reload.elapsed() >= Duration::from_millis(500)
        {
            spawn_reload(
                &self.config.raw_groups,
                &self.config.session_map,
                &self.reload_tx,
            );
            self.reloading = true;
            self.last_reload = std::time::Instant::now();
        }

        if self.reloading
            && let Ok((c, idx)) = self.reload_rx.try_recv()
        {
            self.data.merged_cache = c;
            self.data.index = idx;
            self.reloading = false;
            self.recompute_tokens();
            self.record_rate_history();
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

        // Tab/BackTab cycle time filters globally, except in Settings/RateTracking.
        if !matches!(self.view, View::Settings(_) | View::RateTracking) {
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
            View::RateTracking => match key.code {
                KeyCode::Char('`') => {
                    self.switch_view(View::Main);
                }
                KeyCode::Char('q') => return false,
                KeyCode::Right | KeyCode::Char('l') => {
                    let n = self.data.oauth_credentials.len();
                    if n > 0 {
                        self.rate_tracking_selected = (self.rate_tracking_selected + 1) % n;
                    }
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    let n = self.data.oauth_credentials.len();
                    if n > 0 {
                        self.rate_tracking_selected = (self.rate_tracking_selected + n - 1) % n;
                    }
                }
                KeyCode::Char('r') if !self.reloading => {
                    self.usage_poller.force_poll_all();
                }
                _ => {}
            },
            View::Main => match key.code {
                KeyCode::Esc if self.project_index.is_some() => {
                    self.project_index = None;
                    self.card_scroll = 0;
                    self.recompute_tokens();
                }
                KeyCode::Char('q') => return false,
                KeyCode::Char('`') => {
                    self.switch_view(View::RateTracking);
                }
                KeyCode::Char('r') if !self.reloading => {
                    self.usage_poller.force_poll_all();
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
                    let len = self.render.display_order.len();
                    self.project_index = match self.project_index {
                        None if len > 0 => Some(0),
                        Some(i) if i + 1 < len => Some(i + 1),
                        _ => None,
                    };
                    self.card_scroll = 0;
                    self.recompute_tokens();
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    let len = self.render.display_order.len();
                    self.project_index = match self.project_index {
                        None if len > 0 => Some(len - 1),
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
                            && idx >= self.render.display_order.len()
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
    minute_tokens: &MinuteTokens,
    index: &EventIndex,
    groups: &[discovery::ProjectGroup],
    merged_cache: &cache::Cache,
    overrides: &Overrides,
    source_root: Option<&str>,
    project_cwds: Option<&[String]>,
    time_filter: TimeFilter,
) -> RenderCache {
    let today_snap = Local::now().date_naive();
    let subday = time_filter.subday_start();

    // For sub-day filters (1H, 12H), build DailyTokens from minute-level data
    // so that KPI values reflect the actual time window.
    let filtered = if let Some((sd, sm)) = subday {
        minute_tokens.to_daily_filtered(sd, sm, today_snap)
    } else {
        filter_daily(daily_tokens, time_filter)
    };

    let kpi = CachedKpi::compute(&filtered);
    let cwd_to_root = cards::build_cwd_to_root(groups);
    let date_filter = |d: NaiveDate| date_in_filter(d, time_filter, today_snap);
    let stats = index.build_model_stats(
        &cwd_to_root,
        source_root,
        &date_filter,
        project_cwds,
        time_filter.is_intraday(),
        subday,
    );

    // For sub-day filters, build a cache from the index filtered by minute
    // so that card costs reflect the actual time window.
    let effective_cache;
    let cache_ref = if let Some((sd, sm)) = subday {
        effective_cache = index.build_subday_cache(sd, sm, today_snap);
        &effective_cache
    } else {
        merged_cache
    };
    let cards = cards::build_cards(
        groups,
        cache_ref,
        overrides,
        source_root,
        date_filter,
        &stats.tokens,
        project_cwds,
        &stats.daily_costs,
    );

    // Build a mapping from display position (card order) → group index.
    let root_to_group: std::collections::HashMap<String, usize> = groups
        .iter()
        .enumerate()
        .map(|(i, g)| (g.root_key(), i))
        .collect();
    let display_order: Vec<usize> = cards
        .iter()
        .filter_map(|c| root_to_group.get(&c.root_key).copied())
        .collect();

    let range = compute_range(&filtered, time_filter, today_snap);

    RenderCache {
        filtered,
        kpi,
        minute_model: stats.minute_costs,
        cards,
        range,
        display_order,
    }
}

fn compute_hit_tokens(hits: &mut [RateLimitHit], index: &EventIndex) {
    for hit in hits.iter_mut() {
        let Some(dur_min) = hit.session_duration_min else {
            continue;
        };
        let dur_secs = (dur_min * 60.0) as i64;
        let session_start_utc = hit.timestamp - chrono::Duration::seconds(dur_secs);
        let start_local = session_start_utc
            .with_timezone(&chrono::Local)
            .naive_local();
        let end_local = hit
            .timestamp
            .with_timezone(&chrono::Local)
            .naive_local();
        let tokens = index.tokens_in_window(&hit.source_root, start_local, end_local);
        hit.tokens = tokens;
    }
}

fn load_data(
    raw_groups: &[discovery::ProjectGroup],
    session_map: &HashMap<String, (String, String)>,
) -> (cache::Cache, EventIndex) {
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

    let index = EventIndex::build(&events, session_map);
    (merged, index)
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
        let (cache, index) = load_data(&raw_groups, &session_map);
        let _ = tx.send((cache, index));
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
