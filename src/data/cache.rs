use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{NaiveDate, Timelike};
use serde::{Deserialize, Serialize};

use super::parser::Event;
use super::tokens::DailyTokens;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DayEntry {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_creation: u64,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub lines_suggested: u64,
    #[serde(default)]
    pub lines_accepted: u64,
    #[serde(default)]
    pub lines_added: u64,
    #[serde(default)]
    pub lines_deleted: u64,
    /// Estimated active minutes for this day (activity clustering).
    #[serde(default)]
    pub active_minutes: u64,
}

/// Full cache: source_root -> cwd -> date (YYYY-MM-DD) -> metrics.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cache(HashMap<String, HashMap<String, HashMap<String, DayEntry>>>);

impl Cache {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[cfg(test)]
    pub fn get_root(&self, root: &str) -> Option<&HashMap<String, HashMap<String, DayEntry>>> {
        self.0.get(root)
    }

    pub fn roots(
        &self,
    ) -> impl Iterator<Item = (&String, &HashMap<String, HashMap<String, DayEntry>>)> {
        self.0.iter()
    }

    pub fn entry_root(&mut self, root: String) -> &mut HashMap<String, HashMap<String, DayEntry>> {
        self.0.entry(root).or_default()
    }

    pub fn get_root_mut(
        &mut self,
        root: &str,
    ) -> Option<&mut HashMap<String, HashMap<String, DayEntry>>> {
        self.0.get_mut(root)
    }

    /// Iterate all (root, cwd, date_str, entry) tuples, optionally filtered by root and/or cwds.
    pub fn iter_filtered<'a>(
        &'a self,
        source_root: Option<&'a str>,
        project_cwds: Option<&'a [String]>,
    ) -> impl Iterator<Item = (&'a str, &'a str, &'a str, &'a DayEntry)> {
        self.0.iter().flat_map(move |(root, cwd_map)| {
            let root_matches = source_root.is_none_or(|sr| sr == root);
            let iter: Box<dyn Iterator<Item = _> + 'a> = if root_matches {
                Box::new(cwd_map.iter().flat_map(move |(cwd, days)| {
                    let cwd_matches = project_cwds.is_none_or(|cwds| cwds.contains(cwd));
                    let iter: Box<dyn Iterator<Item = _> + 'a> = if cwd_matches {
                        Box::new(days.iter().map(move |(date_str, entry)| {
                            (root.as_str(), cwd.as_str(), date_str.as_str(), entry)
                        }))
                    } else {
                        Box::new(std::iter::empty())
                    };
                    iter
                }))
            } else {
                Box::new(std::iter::empty())
            };
            iter
        })
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

fn cache_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    home.join(".config").join("ccmeter").join("history.json")
}

pub fn load() -> Cache {
    let path = cache_path();
    if !path.exists() {
        return Cache::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(cache: &Cache) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(cache) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::write(&path, &json);
        }
    }
}

// ---------------------------------------------------------------------------
// Build from parsed events
// ---------------------------------------------------------------------------

/// Gap threshold in minutes for activity clustering.
const GAP_THRESHOLD: u16 = 5;

pub fn from_events(events: &[Event], session_info: &HashMap<String, (String, String)>) -> Cache {
    let mut cache = Cache::new();

    // Collect minute_of_day per (root, cwd, date, session) for active time calculation.
    // Key: (root, cwd, date_key, session_file) → Vec<minute_of_day>
    let mut session_minutes: HashMap<(String, String, String, String), Vec<u16>> = HashMap::new();

    for ev in events {
        let (root, cwd) = match session_info.get(&ev.session_file) {
            Some(pair) => pair,
            None => continue,
        };
        let local = ev.timestamp.with_timezone(&chrono::Local);
        let date_key = local.date_naive().format("%Y-%m-%d").to_string();
        let entry = cache
            .entry_root(root.clone())
            .entry(cwd.clone())
            .or_default()
            .entry(date_key.clone())
            .or_default();
        entry.input += ev.input_tokens;
        entry.output += ev.output_tokens;
        entry.cache_read += ev.cache_read_input_tokens;
        entry.cache_creation += ev.cache_creation_input_tokens;
        entry.cost += ev.cost_usd;
        entry.lines_suggested += ev.lines_suggested;
        entry.lines_accepted += ev.lines_accepted;
        entry.lines_added += ev.lines_added;
        entry.lines_deleted += ev.lines_deleted;

        let minute_of_day = local.hour() as u16 * 60 + local.minute() as u16;
        session_minutes
            .entry((root.clone(), cwd.clone(), date_key, ev.session_file.clone()))
            .or_default()
            .push(minute_of_day);
    }

    // Group session intervals by (root, cwd, date), then merge across sessions.
    let mut day_intervals: HashMap<(String, String, String), Vec<(u16, u16)>> = HashMap::new();
    for ((root, cwd, date_key, _session), mut minutes) in session_minutes {
        minutes.sort();
        minutes.dedup();
        let intervals = cluster_to_intervals(&minutes);
        day_intervals
            .entry((root, cwd, date_key))
            .or_default()
            .extend(intervals);
    }

    for ((root, cwd, date_key), mut intervals) in day_intervals {
        let active = merge_intervals_duration(&mut intervals);
        if let Some(entry) = cache
            .entry_root(root)
            .get_mut(&cwd)
            .and_then(|days| days.get_mut(&date_key))
        {
            entry.active_minutes = active;
        }
    }

    cache
}

/// Cluster sorted, deduped minute values into intervals `[start, end]`.
fn cluster_to_intervals(minutes: &[u16]) -> Vec<(u16, u16)> {
    if minutes.is_empty() {
        return Vec::new();
    }
    let mut intervals = Vec::new();
    let mut start = minutes[0];
    let mut end = minutes[0];
    for &m in &minutes[1..] {
        if m - end <= GAP_THRESHOLD {
            end = m;
        } else {
            intervals.push((start, end));
            start = m;
            end = m;
        }
    }
    intervals.push((start, end));
    intervals
}

/// Merge overlapping/adjacent intervals and return total duration in minutes.
fn merge_intervals_duration(intervals: &mut [(u16, u16)]) -> u64 {
    if intervals.is_empty() {
        return 0;
    }
    intervals.sort();
    let mut merged: Vec<(u16, u16)> = vec![intervals[0]];
    for &(s, e) in &intervals[1..] {
        let last = merged.last_mut().unwrap();
        if s <= last.1.saturating_add(1) {
            last.1 = last.1.max(e);
        } else {
            merged.push((s, e));
        }
    }
    merged
        .iter()
        .map(|(s, e)| (*e as u64 - *s as u64) + 1)
        .sum()
}

/// Compute active minutes from sorted, deduped minute-of-day values using
/// gap-based clustering. Public so that `EventIndex::build_subday_cache` can
/// reuse the same logic.
pub fn cluster_active_minutes(minutes: &[u16]) -> u64 {
    let mut intervals = cluster_to_intervals(minutes);
    merge_intervals_duration(&mut intervals)
}

// ---------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------

/// Merge two caches: for each root + cwd + date, take max of each field.
///
/// This preserves historical high-water marks even if JSONL files are deleted.
pub fn merge(old: Cache, fresh: &Cache) -> Cache {
    let mut result = old;
    for (root, cwd_map) in fresh.roots() {
        let root_target = result.entry_root(root.clone());
        for (cwd, days) in cwd_map.iter() {
            let cwd_target = root_target.entry(cwd.clone()).or_default();
            for (date, entry) in days.iter() {
                let t = cwd_target.entry(date.clone()).or_default();
                t.input = t.input.max(entry.input);
                t.output = t.output.max(entry.output);
                t.cache_read = t.cache_read.max(entry.cache_read);
                t.cache_creation = t.cache_creation.max(entry.cache_creation);
                t.cost = t.cost.max(entry.cost);
                t.lines_suggested = t.lines_suggested.max(entry.lines_suggested);
                t.lines_accepted = t.lines_accepted.max(entry.lines_accepted);
                t.lines_added = t.lines_added.max(entry.lines_added);
                t.lines_deleted = t.lines_deleted.max(entry.lines_deleted);
                t.active_minutes = t.active_minutes.max(entry.active_minutes);
            }
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Convert to DailyTokens (using iter_filtered)
// ---------------------------------------------------------------------------

fn accumulate_days(daily: &mut DailyTokens, date: NaiveDate, entry: &DayEntry) {
    *daily.input.entry(date).or_default() += entry.input;
    *daily.output.entry(date).or_default() += entry.output;
    if entry.lines_suggested > 0 {
        *daily.lines_suggested.entry(date).or_default() += entry.lines_suggested;
    }
    if entry.lines_accepted > 0 {
        *daily.lines_accepted.entry(date).or_default() += entry.lines_accepted;
    }
    if entry.lines_added > 0 {
        *daily.lines_added.entry(date).or_default() += entry.lines_added;
    }
    if entry.lines_deleted > 0 {
        *daily.lines_deleted.entry(date).or_default() += entry.lines_deleted;
    }
    *daily.cost.entry(date).or_default() += entry.cost;
}

/// Convert the cache into DailyTokens, optionally filtered by root and/or cwds.
pub fn to_daily_tokens_filtered(
    cache: &Cache,
    source_root: Option<&str>,
    project_cwds: Option<&[String]>,
) -> DailyTokens {
    let mut daily = DailyTokens::default();
    for (_root, _cwd, date_str, entry) in cache.iter_filtered(source_root, project_cwds) {
        let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") else {
            continue;
        };
        accumulate_days(&mut daily, date, entry);
    }
    daily
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_entry(cache: &mut Cache, root: &str, cwd: &str, date: &str, entry: DayEntry) {
        cache
            .entry_root(root.into())
            .entry(cwd.into())
            .or_default()
            .insert(date.into(), entry);
    }

    #[test]
    fn merge_takes_max() {
        let mut old = Cache::new();
        insert_entry(
            &mut old,
            "root",
            "proj",
            "2026-01-01",
            DayEntry {
                input: 100,
                output: 50,
                lines_suggested: 10,
                lines_accepted: 8,
                lines_added: 5,
                lines_deleted: 3,
                ..Default::default()
            },
        );

        let mut fresh = Cache::new();
        insert_entry(
            &mut fresh,
            "root",
            "proj",
            "2026-01-01",
            DayEntry {
                input: 80,
                output: 60,
                lines_suggested: 12,
                lines_accepted: 7,
                lines_added: 4,
                lines_deleted: 5,
                ..Default::default()
            },
        );

        let merged = merge(old, &fresh);
        let root = merged.get_root("root").unwrap();
        let day = &root["proj"]["2026-01-01"];
        assert_eq!(day.input, 100);
        assert_eq!(day.output, 60);
        assert_eq!(day.lines_suggested, 12);
        assert_eq!(day.lines_accepted, 8);
        assert_eq!(day.lines_added, 5);
        assert_eq!(day.lines_deleted, 5);
    }

    #[test]
    fn merge_adds_new_days() {
        let old = Cache::new();
        let mut fresh = Cache::new();
        insert_entry(
            &mut fresh,
            "root",
            "proj",
            "2026-02-01",
            DayEntry {
                input: 200,
                ..Default::default()
            },
        );

        let merged = merge(old, &fresh);
        let root = merged.get_root("root").unwrap();
        assert_eq!(root["proj"]["2026-02-01"].input, 200);
    }

    #[test]
    fn merge_adds_new_cwd() {
        let mut old = Cache::new();
        insert_entry(
            &mut old,
            "root",
            "proj_a",
            "2026-01-01",
            DayEntry {
                input: 100,
                ..Default::default()
            },
        );

        let mut fresh = Cache::new();
        insert_entry(
            &mut fresh,
            "root",
            "proj_b",
            "2026-01-01",
            DayEntry {
                input: 200,
                ..Default::default()
            },
        );

        let merged = merge(old, &fresh);
        let root = merged.get_root("root").unwrap();
        assert_eq!(root["proj_a"]["2026-01-01"].input, 100);
        assert_eq!(root["proj_b"]["2026-01-01"].input, 200);
    }

    #[test]
    fn to_daily_tokens_sums_across_roots_and_cwds() {
        let mut cache = Cache::new();
        insert_entry(
            &mut cache,
            "root_a",
            "proj_a",
            "2026-03-01",
            DayEntry {
                input: 100,
                lines_accepted: 10,
                lines_added: 6,
                lines_deleted: 4,
                ..Default::default()
            },
        );
        insert_entry(
            &mut cache,
            "root_b",
            "proj_b",
            "2026-03-01",
            DayEntry {
                input: 200,
                lines_accepted: 20,
                lines_added: 15,
                lines_deleted: 5,
                ..Default::default()
            },
        );

        let daily = to_daily_tokens_filtered(&cache, None, None);
        let date = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        assert_eq!(daily.input[&date], 300);
        assert_eq!(daily.lines_accepted[&date], 30);
        assert_eq!(daily.lines_added[&date], 21);
        assert_eq!(daily.lines_deleted[&date], 9);
    }

    #[test]
    fn to_daily_tokens_for_root_filters() {
        let mut cache = Cache::new();
        insert_entry(
            &mut cache,
            "root_a",
            "proj",
            "2026-03-01",
            DayEntry {
                input: 100,
                ..Default::default()
            },
        );
        insert_entry(
            &mut cache,
            "root_b",
            "proj",
            "2026-03-01",
            DayEntry {
                input: 200,
                ..Default::default()
            },
        );

        let daily_a = to_daily_tokens_filtered(&cache, Some("root_a"), None);
        let daily_b = to_daily_tokens_filtered(&cache, Some("root_b"), None);
        let date = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        assert_eq!(daily_a.input[&date], 100);
        assert_eq!(daily_b.input[&date], 200);
    }

    #[test]
    fn to_daily_tokens_skips_invalid_dates() {
        let mut cache = Cache::new();
        insert_entry(
            &mut cache,
            "root",
            "proj",
            "not-a-date",
            DayEntry {
                input: 999,
                ..Default::default()
            },
        );

        let daily = to_daily_tokens_filtered(&cache, None, None);
        assert!(daily.input.is_empty());
    }

    #[test]
    fn from_events_groups_by_root_and_cwd() {
        use chrono::{TimeZone, Utc};

        let mut session_info = HashMap::new();
        session_info.insert("session1.jsonl".into(), ("root_a".into(), "/proj/a".into()));
        session_info.insert("session2.jsonl".into(), ("root_b".into(), "/proj/b".into()));

        let events = vec![
            Event {
                timestamp: Utc.with_ymd_and_hms(2026, 1, 15, 10, 0, 0).unwrap(),
                model: String::new(),
                input_tokens: 100,
                output_tokens: 50,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
                cost_usd: 1.0,
                lines_suggested: 10,
                lines_accepted: 8,
                lines_added: 5,
                lines_deleted: 3,
                session_file: "session1.jsonl".into(),
            },
            Event {
                timestamp: Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap(),
                model: String::new(),
                input_tokens: 200,
                output_tokens: 100,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
                cost_usd: 2.0,
                lines_suggested: 0,
                lines_accepted: 0,
                lines_added: 0,
                lines_deleted: 0,
                session_file: "session2.jsonl".into(),
            },
        ];

        let cache = from_events(&events, &session_info);
        let root_a = cache.get_root("root_a").unwrap();
        let root_b = cache.get_root("root_b").unwrap();
        assert_eq!(root_a["/proj/a"]["2026-01-15"].input, 100);
        assert_eq!(root_b["/proj/b"]["2026-01-15"].input, 200);
        assert_eq!(root_a["/proj/a"]["2026-01-15"].lines_added, 5);
    }

    #[test]
    fn from_events_skips_unknown_session() {
        use chrono::{TimeZone, Utc};

        let session_info = HashMap::new();

        let events = vec![Event {
            timestamp: Utc.with_ymd_and_hms(2026, 1, 15, 10, 0, 0).unwrap(),
            model: String::new(),
            input_tokens: 100,
            output_tokens: 50,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            cost_usd: 1.0,
            lines_suggested: 0,
            lines_accepted: 0,
            lines_added: 0,
            lines_deleted: 0,
            session_file: "unknown.jsonl".into(),
        }];

        let cache = from_events(&events, &session_info);
        assert!(cache.is_empty());
    }
}
