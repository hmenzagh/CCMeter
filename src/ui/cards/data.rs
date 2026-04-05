use std::collections::HashMap;

use chrono::{NaiveDate, Timelike};

use crate::config::discovery::ProjectGroup;
use crate::config::overrides::Overrides;
use crate::data::cache::{Cache, DayEntry};

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

pub struct ProjectCard {
    pub name: String,
    pub root_key: String,
    pub is_starred: bool,
    pub last_activity: NaiveDate,
    pub first_activity: NaiveDate,
    pub total_cost: f64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub tokens_cache: u64,
    pub lines_added: u64,
    pub lines_deleted: u64,
    pub lines_suggested: u64,
    pub lines_accepted: u64,
    pub efficiency: f64,
    pub daily_costs: Vec<(NaiveDate, f64)>,
    pub daily_tokens_in: Vec<(NaiveDate, u64)>,
    pub daily_tokens_out: Vec<(NaiveDate, u64)>,
    pub model_shares: Vec<(String, f64)>,
    pub model_daily_costs: Vec<(String, Vec<(NaiveDate, f64)>)>,
    pub sessions: usize,
    pub active_days: usize,
    /// Estimated active time in minutes (activity clustering with gap threshold).
    pub time_minutes: u64,
}

// ---------------------------------------------------------------------------
// Aggregation
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn build_cards(
    groups: &[ProjectGroup],
    cache: &Cache,
    overrides: &Overrides,
    source_root: Option<&str>,
    date_filter: impl Fn(NaiveDate) -> bool,
    model_tokens: &HashMap<(String, String), u64>,
    project_cwds: Option<&[String]>,
    model_daily_costs_map: &HashMap<(String, String), HashMap<NaiveDate, f64>>,
) -> Vec<ProjectCard> {
    let mut cards = Vec::new();

    for group in groups {
        let root_key = group.root_key();

        if overrides.is_hidden(&root_key) {
            continue;
        }

        let display_name = overrides
            .get_name(&root_key)
            .map(|s| s.to_string())
            .unwrap_or_else(|| group.name.clone());

        let cwds: Vec<String> = group.sources.iter().filter_map(|s| s.cwd.clone()).collect();

        if let Some(filter_cwds) = project_cwds
            && !cwds.iter().any(|c| filter_cwds.contains(c))
        {
            continue;
        }

        let mut total_cost = 0.0f64;
        let mut tokens_in = 0u64;
        let mut tokens_out = 0u64;
        let mut tokens_cache = 0u64;
        let mut lines_added = 0u64;
        let mut lines_deleted = 0u64;
        let mut lines_suggested = 0u64;
        let mut lines_accepted = 0u64;
        let mut last_activity: Option<NaiveDate> = None;
        let mut first_activity: Option<NaiveDate> = None;
        let mut cost_by_day: HashMap<NaiveDate, f64> = HashMap::new();
        let mut tokens_in_by_day: HashMap<NaiveDate, u64> = HashMap::new();
        let mut tokens_out_by_day: HashMap<NaiveDate, u64> = HashMap::new();
        let mut cached_active_minutes: u64 = 0;

        for (_root, _cwd, date_str, entry) in cache.iter_filtered(source_root, Some(&cwds)) {
            let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") else {
                continue;
            };
            if !date_filter(date) {
                continue;
            }

            accumulate_entry(
                entry,
                date,
                &mut total_cost,
                &mut tokens_in,
                &mut tokens_out,
                &mut tokens_cache,
                &mut lines_added,
                &mut lines_deleted,
                &mut last_activity,
                &mut cost_by_day,
            );
            lines_suggested += entry.lines_suggested;
            lines_accepted += entry.lines_accepted;
            *tokens_in_by_day.entry(date).or_default() += entry.input;
            *tokens_out_by_day.entry(date).or_default() += entry.output;
            cached_active_minutes += entry.active_minutes;
            match first_activity {
                Some(prev) if date < prev => first_activity = Some(date),
                None => first_activity = Some(date),
                _ => {}
            }
        }

        let last_activity = match last_activity {
            Some(d) => d,
            None => continue,
        };
        let first_activity = first_activity.unwrap_or(last_activity);

        let total_lines = lines_added + lines_deleted;
        let total_tokens = tokens_in + tokens_out;
        let efficiency = if total_lines > 0 {
            total_tokens as f64 / total_lines as f64
        } else {
            0.0
        };

        let active_days = cost_by_day.len();
        let sessions = group.total_sessions;

        let mut daily_costs: Vec<(NaiveDate, f64)> = cost_by_day.into_iter().collect();
        daily_costs.sort_by_key(|&(d, _)| d);
        let mut daily_tokens_in: Vec<(NaiveDate, u64)> = tokens_in_by_day.into_iter().collect();
        daily_tokens_in.sort_by_key(|&(d, _)| d);
        let mut daily_tokens_out: Vec<(NaiveDate, u64)> = tokens_out_by_day.into_iter().collect();
        daily_tokens_out.sort_by_key(|&(d, _)| d);

        let model_shares = compute_model_shares(&root_key, model_tokens);

        let model_order = ["opus", "sonnet", "haiku", "other"];
        let mut model_daily_costs: Vec<(String, Vec<(NaiveDate, f64)>)> = Vec::new();
        for &model in &model_order {
            let key = (root_key.clone(), model.to_string());
            if let Some(day_map) = model_daily_costs_map.get(&key) {
                let mut series: Vec<(NaiveDate, f64)> =
                    day_map.iter().map(|(&d, &c)| (d, c)).collect();
                series.sort_by_key(|&(d, _)| d);
                if !series.is_empty() {
                    model_daily_costs.push((model.to_string(), series));
                }
            }
        }

        let time_minutes = cached_active_minutes;

        cards.push(ProjectCard {
            name: display_name,
            root_key: root_key.clone(),
            is_starred: overrides.is_starred(&root_key),
            last_activity,
            first_activity,
            total_cost,
            tokens_in,
            tokens_out,
            tokens_cache,
            lines_added,
            lines_deleted,
            lines_suggested,
            lines_accepted,
            efficiency,
            daily_costs,
            daily_tokens_in,
            daily_tokens_out,
            model_shares,
            model_daily_costs,
            sessions,
            active_days,
            time_minutes,
        });
    }

    cards.sort_by(|a, b| match (a.is_starred, b.is_starred) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => b
            .total_cost
            .partial_cmp(&a.total_cost)
            .unwrap_or(std::cmp::Ordering::Equal),
    });

    cards
}

#[allow(clippy::too_many_arguments)]
fn accumulate_entry(
    entry: &DayEntry,
    date: NaiveDate,
    total_cost: &mut f64,
    tokens_in: &mut u64,
    tokens_out: &mut u64,
    tokens_cache: &mut u64,
    lines_added: &mut u64,
    lines_deleted: &mut u64,
    last_activity: &mut Option<NaiveDate>,
    cost_by_day: &mut HashMap<NaiveDate, f64>,
) {
    *total_cost += entry.cost;
    *tokens_in += entry.input;
    *tokens_out += entry.output;
    *tokens_cache += entry.cache_read + entry.cache_creation;
    *lines_added += entry.lines_added;
    *lines_deleted += entry.lines_deleted;
    *cost_by_day.entry(date).or_default() += entry.cost;

    match last_activity {
        Some(prev) if date > *prev => *last_activity = Some(date),
        None => *last_activity = Some(date),
        _ => {}
    }
}

fn build_cwd_to_root(groups: &[ProjectGroup]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for group in groups {
        let root_key = group.root_key();
        for source in &group.sources {
            if let Some(cwd) = &source.cwd {
                map.insert(cwd.clone(), root_key.clone());
            }
        }
    }
    map
}

fn filtered_model_events<'a>(
    events: &'a [crate::data::parser::Event],
    session_map: &'a HashMap<String, (String, String)>,
    cwd_to_root: &'a HashMap<String, String>,
    source_root: Option<&'a str>,
    date_filter: &'a impl Fn(NaiveDate) -> bool,
    project_cwds: Option<&'a [String]>,
) -> impl Iterator<
    Item = (
        &'a str,
        &'static str,
        NaiveDate,
        &'a crate::data::parser::Event,
    ),
> {
    events.iter().filter_map(move |ev| {
        if ev.model.is_empty() {
            return None;
        }
        let (root, cwd) = session_map.get(&ev.session_file)?;
        if let Some(filter_root) = source_root
            && root != filter_root
        {
            return None;
        }
        if let Some(cwds) = project_cwds
            && !cwds.contains(cwd)
        {
            return None;
        }
        let date = ev.timestamp.date_naive();
        if !date_filter(date) {
            return None;
        }
        let root_path = cwd_to_root.get(cwd.as_str())?;
        let model_name = crate::data::models::normalize_model(&ev.model);
        Some((root_path.as_str(), model_name, date, ev))
    })
}

pub fn build_model_tokens(
    events: &[crate::data::parser::Event],
    session_map: &HashMap<String, (String, String)>,
    groups: &[ProjectGroup],
    source_root: Option<&str>,
    date_filter: impl Fn(NaiveDate) -> bool,
    project_cwds: Option<&[String]>,
) -> HashMap<(String, String), u64> {
    let cwd_to_root = build_cwd_to_root(groups);
    let mut result: HashMap<(String, String), u64> = HashMap::new();

    for (root_path, model_name, _date, ev) in filtered_model_events(
        events,
        session_map,
        &cwd_to_root,
        source_root,
        &date_filter,
        project_cwds,
    ) {
        let total = ev.input_tokens + ev.output_tokens;
        *result
            .entry((root_path.to_string(), model_name.to_string()))
            .or_default() += total;
    }

    result
}

pub fn build_model_daily_costs(
    events: &[crate::data::parser::Event],
    session_map: &HashMap<String, (String, String)>,
    groups: &[ProjectGroup],
    source_root: Option<&str>,
    date_filter: impl Fn(NaiveDate) -> bool,
    project_cwds: Option<&[String]>,
) -> HashMap<(String, String), HashMap<NaiveDate, f64>> {
    let cwd_to_root = build_cwd_to_root(groups);
    let mut result: HashMap<(String, String), HashMap<NaiveDate, f64>> = HashMap::new();

    for (root_path, model_name, date, ev) in filtered_model_events(
        events,
        session_map,
        &cwd_to_root,
        source_root,
        &date_filter,
        project_cwds,
    ) {
        if ev.cost_usd <= 0.0 {
            continue;
        }
        *result
            .entry((root_path.to_string(), model_name.to_string()))
            .or_default()
            .entry(date)
            .or_default() += ev.cost_usd;
    }

    result
}

pub fn build_minute_model_costs(
    events: &[crate::data::parser::Event],
    session_map: &HashMap<String, (String, String)>,
    groups: &[ProjectGroup],
    source_root: Option<&str>,
    date_filter: impl Fn(NaiveDate) -> bool,
    project_cwds: Option<&[String]>,
) -> HashMap<(String, String), HashMap<(NaiveDate, u16), f64>> {
    let cwd_to_root = build_cwd_to_root(groups);
    let mut result: HashMap<(String, String), HashMap<(NaiveDate, u16), f64>> = HashMap::new();

    for ev in events {
        if ev.model.is_empty() || ev.cost_usd <= 0.0 {
            continue;
        }
        let (root, cwd) = match session_map.get(&ev.session_file) {
            Some(pair) => pair,
            None => continue,
        };
        if source_root.is_some_and(|sr| sr != root) {
            continue;
        }
        if project_cwds.is_some_and(|cwds| !cwds.contains(cwd)) {
            continue;
        }
        let local = ev.timestamp.with_timezone(&chrono::Local);
        let date = local.date_naive();
        if !date_filter(date) {
            continue;
        }
        let root_path = match cwd_to_root.get(cwd.as_str()) {
            Some(r) => r,
            None => continue,
        };
        let model_name = crate::data::models::normalize_model(&ev.model);
        let minute_of_day = local.hour() as u16 * 60 + local.minute() as u16;
        *result
            .entry((root_path.clone(), model_name.to_string()))
            .or_default()
            .entry((date, minute_of_day))
            .or_default() += ev.cost_usd;
    }

    result
}

fn compute_model_shares(
    root_key: &str,
    model_tokens: &HashMap<(String, String), u64>,
) -> Vec<(String, f64)> {
    let mut totals: HashMap<String, u64> = HashMap::new();
    let mut grand_total = 0u64;

    for ((rk, model), &tokens) in model_tokens {
        if rk == root_key {
            *totals.entry(model.clone()).or_default() += tokens;
            grand_total += tokens;
        }
    }

    if grand_total == 0 {
        return Vec::new();
    }

    let mut shares: Vec<(String, f64)> = totals
        .into_iter()
        .map(|(model, tokens)| (model, tokens as f64 / grand_total as f64))
        .collect();

    shares.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    shares
}
