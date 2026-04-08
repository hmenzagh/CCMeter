use std::path::PathBuf;

use chrono::{NaiveDate, Timelike};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateHistoryEntry {
    pub date: NaiveDate,
    pub resets_at: String,
    pub estimated_tokens: u64,
    pub source_root: String,
    /// Pre-bucketed hour key for dedup (e.g. "2026-04-08T15").
    #[serde(default)]
    pub bucket: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RateHistory {
    pub entries: Vec<RateHistoryEntry>,
}

fn history_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    home.join(".config")
        .join("ccmeter")
        .join("rate-history.json")
}

pub fn load() -> RateHistory {
    let path = history_path();
    if !path.exists() {
        return RateHistory::default();
    }
    let mut history: RateHistory = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let cutoff = chrono::Utc::now().date_naive() - chrono::Duration::days(90);
    history.entries.retain(|e| e.date >= cutoff);

    // Backfill bucket field for entries saved before this field existed.
    for entry in &mut history.entries {
        if entry.bucket.is_empty() {
            entry.bucket = bucket_resets_at(&entry.resets_at);
        }
    }

    history
}

pub fn save(history: &RateHistory) {
    let path = history_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(history) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::write(&path, &json);
        }
    }
}

/// Bucket a `resets_at` RFC3339 timestamp to the nearest hour
/// to handle API timestamps oscillating around the hour boundary.
fn bucket_resets_at(resets_at: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(resets_at) {
        Ok(dt) => {
            let rounded = if dt.minute() >= 30 {
                dt + chrono::Duration::minutes(60 - dt.minute() as i64)
            } else {
                dt - chrono::Duration::minutes(dt.minute() as i64)
            };
            rounded.format("%Y-%m-%dT%H").to_string()
        }
        Err(_) => resets_at.to_string(),
    }
}

impl RateHistory {
    /// Record or update an estimation for the given 5h window.
    pub fn record(
        &mut self,
        source_root: &str,
        resets_at: &str,
        estimated_tokens: u64,
        date: NaiveDate,
    ) {
        let bucket = bucket_resets_at(resets_at);
        if let Some(existing) = self.entries.iter_mut().find(|e| {
            e.source_root == source_root && e.bucket == bucket
        }) {
            existing.estimated_tokens = estimated_tokens;
            existing.resets_at = resets_at.to_string();
            existing.date = date;
        } else {
            self.entries.push(RateHistoryEntry {
                date,
                resets_at: resets_at.to_string(),
                estimated_tokens,
                source_root: source_root.to_string(),
                bucket,
            });
        }
    }

    /// Get all entries for a given source root.
    pub fn entries_for_root(&self, source_root: &str) -> Vec<&RateHistoryEntry> {
        self.entries
            .iter()
            .filter(|e| e.source_root == source_root)
            .collect()
    }
}
