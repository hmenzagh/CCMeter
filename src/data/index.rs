use std::collections::{HashMap, HashSet};

use chrono::{NaiveDate, Timelike};

use super::models::normalize_model;
use super::parser::Event;
use super::tokens::MinuteTokens;

// ---------------------------------------------------------------------------
// Model enum — 4 variants, stored as u8
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ModelId {
    Opus = 0,
    Sonnet = 1,
    Haiku = 2,
    Other = 3,
}

impl ModelId {
    fn from_raw(model: &str) -> Self {
        match normalize_model(model) {
            "opus" => Self::Opus,
            "sonnet" => Self::Sonnet,
            "haiku" => Self::Haiku,
            _ => Self::Other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Opus => "opus",
            Self::Sonnet => "sonnet",
            Self::Haiku => "haiku",
            Self::Other => "other",
        }
    }
}

// ---------------------------------------------------------------------------
// Compact pre-aggregated entry
// ---------------------------------------------------------------------------

/// One record per unique (root, cwd, model, date, minute) combination.
#[derive(Debug, Clone)]
pub struct CompactEntry {
    pub root_idx: u16,
    pub cwd_idx: u16,
    pub model: ModelId,
    pub date: NaiveDate,
    pub minute: u16,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
    pub lines_accepted: u64,
    pub lines_suggested: u64,
}

// ---------------------------------------------------------------------------
// Combined model stats result
// ---------------------------------------------------------------------------

pub struct ModelStats {
    pub tokens: HashMap<(String, String), u64>,
    pub daily_costs: HashMap<(String, String), HashMap<NaiveDate, f64>>,
    pub minute_costs: HashMap<(String, String), HashMap<(NaiveDate, u16), f64>>,
}

// ---------------------------------------------------------------------------
// EventIndex
// ---------------------------------------------------------------------------

/// Compact, pre-indexed replacement for `Vec<Event>`.
///
/// Events are resolved (session → root/cwd) and aggregated by
/// (root, cwd, model, date, minute_of_day) to drastically reduce
/// memory compared to keeping every raw event.
pub struct EventIndex {
    cwds: Vec<String>,
    root_intern: HashMap<String, u16>,
    cwd_intern: HashMap<String, u16>,
    entries: Vec<CompactEntry>,
}

impl EventIndex {
    /// Build from raw events + session map. After this call the raw events
    /// can be dropped.
    pub fn build(
        events: &[Event],
        session_info: &HashMap<String, (String, String)>,
    ) -> Self {
        let mut root_intern: HashMap<String, u16> = HashMap::new();
        let mut cwd_intern: HashMap<String, u16> = HashMap::new();
        let mut roots: Vec<String> = Vec::new();
        let mut cwds: Vec<String> = Vec::new();

        // Aggregate into a HashMap first, then flatten.
        type Key = (u16, u16, ModelId, NaiveDate, u16);
        let mut agg: HashMap<Key, (u64, u64, f64, u64, u64)> = HashMap::new();

        for ev in events {
            let (root, cwd) = match session_info.get(&ev.session_file) {
                Some(pair) => pair,
                None => continue,
            };

            let root_idx = *root_intern.entry(root.clone()).or_insert_with(|| {
                let idx = roots.len() as u16;
                roots.push(root.clone());
                idx
            });
            let cwd_idx = *cwd_intern.entry(cwd.clone()).or_insert_with(|| {
                let idx = cwds.len() as u16;
                cwds.push(cwd.clone());
                idx
            });

            let model = if ev.model.is_empty() {
                ModelId::Other
            } else {
                ModelId::from_raw(&ev.model)
            };

            let local = ev.timestamp.with_timezone(&chrono::Local);
            let date = local.date_naive();
            let minute = local.hour() as u16 * 60 + local.minute() as u16;

            let key = (root_idx, cwd_idx, model, date, minute);
            let acc = agg.entry(key).or_default();
            acc.0 += ev.input_tokens;
            acc.1 += ev.output_tokens;
            acc.2 += ev.cost_usd;
            acc.3 += ev.lines_accepted;
            acc.4 += ev.lines_suggested;
        }

        let entries: Vec<CompactEntry> = agg
            .into_iter()
            .map(|((root_idx, cwd_idx, model, date, minute), (inp, out, cost, la, ls))| {
                CompactEntry {
                    root_idx,
                    cwd_idx,
                    model,
                    date,
                    minute,
                    input_tokens: inp,
                    output_tokens: out,
                    cost,
                    lines_accepted: la,
                    lines_suggested: ls,
                }
            })
            .collect();


        EventIndex {
            cwds,
            root_intern,
            cwd_intern,
            entries,
        }
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Build MinuteTokens (for intraday heatmap), filtered by root and/or cwds.
    pub fn build_minute_tokens(
        &self,
        source_root: Option<&str>,
        project_cwds: Option<&[String]>,
    ) -> MinuteTokens {
        let root_filter = source_root.and_then(|sr| self.root_intern.get(sr).copied());
        let cwd_filter = project_cwds.map(|cwds| self.cwd_set(cwds));

        let mut mt = MinuteTokens::default();
        for e in &self.entries {
            if !self.matches_filter(e, root_filter, cwd_filter.as_ref()) {
                continue;
            }
            let key = (e.date, e.minute);
            *mt.input.entry(key).or_default() += e.input_tokens;
            *mt.output.entry(key).or_default() += e.output_tokens;
            *mt.lines_accepted.entry(key).or_default() += e.lines_accepted;
            *mt.lines_suggested.entry(key).or_default() += e.lines_suggested;
            *mt.cost.entry(key).or_default() += e.cost;
        }
        mt
    }

    /// Build all model-level aggregations in a single pass over entries.
    pub fn build_model_stats(
        &self,
        cwd_to_root: &HashMap<String, String>,
        source_root: Option<&str>,
        date_filter: &impl Fn(NaiveDate) -> bool,
        project_cwds: Option<&[String]>,
        include_minute: bool,
    ) -> ModelStats {
        let root_filter = source_root.and_then(|sr| self.root_intern.get(sr).copied());
        let cwd_filter = project_cwds.map(|cwds| self.cwd_set(cwds));

        // Map cwd_idx → root_key_idx so multiple cwds sharing a root_key aggregate correctly.
        let mut rk_intern: HashMap<&str, u16> = HashMap::new();
        let mut rk_strings: Vec<&str> = Vec::new();
        let cwd_to_rk: HashMap<u16, u16> = self
            .cwds
            .iter()
            .enumerate()
            .filter_map(|(i, cwd)| {
                let rk = cwd_to_root.get(cwd.as_str())?.as_str();
                let rk_idx = *rk_intern.entry(rk).or_insert_with(|| {
                    let idx = rk_strings.len() as u16;
                    rk_strings.push(rk);
                    idx
                });
                Some((i as u16, rk_idx))
            })
            .collect();

        let mut tok_agg: HashMap<(u16, ModelId), u64> = HashMap::new();
        let mut daily_agg: HashMap<(u16, ModelId), HashMap<NaiveDate, f64>> = HashMap::new();
        let mut minute_agg: HashMap<(u16, ModelId), HashMap<(NaiveDate, u16), f64>> =
            HashMap::new();

        for e in &self.entries {
            if !self.matches_filter(e, root_filter, cwd_filter.as_ref()) {
                continue;
            }
            if !date_filter(e.date) {
                continue;
            }
            let rk_idx = match cwd_to_rk.get(&e.cwd_idx) {
                Some(&idx) => idx,
                None => continue,
            };

            let key = (rk_idx, e.model);

            let total = e.input_tokens + e.output_tokens;
            if e.model != ModelId::Other || total > 0 {
                *tok_agg.entry(key).or_default() += total;
            }

            if e.cost > 0.0 {
                *daily_agg
                    .entry(key)
                    .or_default()
                    .entry(e.date)
                    .or_default() += e.cost;
            }

            if include_minute && e.cost > 0.0 {
                *minute_agg
                    .entry(key)
                    .or_default()
                    .entry((e.date, e.minute))
                    .or_default() += e.cost;
            }
        }

        // Convert (rk_idx, ModelId) → (String, String).
        let resolve = |k: &(u16, ModelId)| -> (String, String) {
            (rk_strings[k.0 as usize].to_string(), k.1.as_str().to_string())
        };

        let tokens = tok_agg
            .into_iter()
            .map(|(k, v)| (resolve(&k), v))
            .collect();

        let daily_costs = daily_agg
            .into_iter()
            .map(|(k, v)| (resolve(&k), v))
            .collect();

        let minute_costs = if include_minute {
            minute_agg
                .into_iter()
                .map(|(k, v)| (resolve(&k), v))
                .collect()
        } else {
            HashMap::new()
        };

        ModelStats {
            tokens,
            daily_costs,
            minute_costs,
        }
    }

    // ------------------------------------------------------------------
    // Filter helpers
    // ------------------------------------------------------------------

    fn cwd_set(&self, cwds: &[String]) -> HashSet<u16> {
        cwds.iter()
            .filter_map(|c| self.cwd_intern.get(c).copied())
            .collect()
    }

    fn matches_filter(
        &self,
        entry: &CompactEntry,
        root_filter: Option<u16>,
        cwd_filter: Option<&HashSet<u16>>,
    ) -> bool {
        if let Some(ri) = root_filter
            && entry.root_idx != ri
        {
            return false;
        }
        if let Some(cwds) = cwd_filter
            && !cwds.contains(&entry.cwd_idx)
        {
            return false;
        }
        true
    }
}
