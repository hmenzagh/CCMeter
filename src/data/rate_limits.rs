use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Timelike, Utc};
use rayon::prelude::*;
use serde::Deserialize;

/// A single rate-limit hit extracted from a JSONL line.
#[derive(Debug, Clone)]
pub struct RateLimitHit {
    pub timestamp: DateTime<Utc>,
    /// The human-readable message (e.g. "You've hit your limit · resets 6pm (Europe/Paris)").
    #[allow(dead_code)]
    pub message: String,
    /// Which source root this came from (e.g. `~/.claude/projects` or `~/.claude-pro/projects`).
    pub source_root: String,
    /// Session duration in minutes: time from first assistant message to this hit,
    /// considering only messages from the same source_root.
    pub session_duration_min: Option<f64>,
}

// ---------------------------------------------------------------------------
// Serde helpers — only what we need
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawHitLine {
    timestamp: Option<String>,
    message: Option<RawHitMessage>,
    error: Option<String>,
    #[serde(rename = "isApiErrorMessage")]
    is_api_error: Option<bool>,
}

#[derive(Deserialize)]
struct RawHitMessage {
    content: Option<Vec<RawHitContent>>,
}

#[derive(Deserialize)]
struct RawHitContent {
    text: Option<String>,
}

// ---------------------------------------------------------------------------
// Internal: find rate-limit lines in a single file
// ---------------------------------------------------------------------------

struct RawHit {
    timestamp: DateTime<Utc>,
    message: String,
}

fn scan_file_for_hits(path: &Path) -> Vec<RawHit> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    let mut hits = Vec::new();

    for line in reader.lines().map_while(Result::ok) {
        // Fast pre-filter to avoid parsing every line
        if !line.contains("\"rate_limit\"") || !line.contains("\"isApiErrorMessage\"") {
            continue;
        }
        let Ok(raw) = serde_json::from_str::<RawHitLine>(&line) else {
            continue;
        };
        if raw.error.as_deref() != Some("rate_limit") || raw.is_api_error != Some(true) {
            continue;
        }
        let Some(ts_str) = raw.timestamp else {
            continue;
        };
        let Ok(ts) = DateTime::parse_from_rfc3339(&ts_str) else {
            continue;
        };

        let message = raw
            .message
            .and_then(|m| m.content)
            .and_then(|c| c.into_iter().next())
            .and_then(|b| b.text)
            .unwrap_or_default();

        hits.push(RawHit {
            timestamp: ts.with_timezone(&Utc),
            message,
        });
    }
    hits
}

// ---------------------------------------------------------------------------
// Internal: find first assistant timestamp in a file
// ---------------------------------------------------------------------------

fn first_assistant_timestamp(path: &Path) -> Option<DateTime<Utc>> {
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    for line in reader.lines().map_while(Result::ok) {
        if !line.contains("\"assistant\"") {
            continue;
        }
        #[derive(Deserialize)]
        struct Stub {
            timestamp: Option<String>,
            #[serde(rename = "type")]
            line_type: Option<String>,
            message: Option<StubMsg>,
        }
        #[derive(Deserialize)]
        struct StubMsg {
            usage: Option<serde_json::Value>,
        }
        let Ok(stub) = serde_json::from_str::<Stub>(&line) else {
            continue;
        };
        if stub.line_type.as_deref() != Some("assistant") {
            continue;
        }
        // Only count lines that have actual usage (real API calls)
        if stub.message.and_then(|m| m.usage).is_none() {
            continue;
        }
        let ts_str = stub.timestamp?;
        let ts = DateTime::parse_from_rfc3339(&ts_str).ok()?;
        return Some(ts.with_timezone(&Utc));
    }
    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Discover all rate-limit hits across all JSONL files in the given source roots.
/// Each root is a path like `~/.claude/projects` or `~/.claude-pro/projects`.
/// Returns hits sorted by timestamp (most recent first), de-duplicated by minute.
pub fn discover_rate_limit_hits(source_roots: &[PathBuf]) -> Vec<RateLimitHit> {
    // Collect all (file, source_root) pairs
    let mut file_root_pairs: Vec<(PathBuf, String)> = Vec::new();
    for root in source_roots {
        let root_str = root.to_string_lossy().to_string();
        collect_jsonl_recursive(root, &root_str, &mut file_root_pairs);
    }

    // Scan all files for rate-limit hits in parallel
    let raw_hits: Vec<(RawHit, String)> = file_root_pairs
        .par_iter()
        .flat_map(|(path, root_str)| {
            scan_file_for_hits(path)
                .into_iter()
                .map(|h| (h, root_str.clone()))
                .collect::<Vec<_>>()
        })
        .collect();

    // De-duplicate by (source_root, 15-minute bucket)
    let mut seen = std::collections::HashSet::new();
    let mut deduped: Vec<(RawHit, String)> = Vec::new();
    // Sort chronologically first so we keep the earliest per bucket
    let mut sorted = raw_hits;
    sorted.sort_by_key(|(h, _)| h.timestamp);
    for pair in sorted {
        let ts = pair.0.timestamp;
        let bucket = format!(
            "{}-{:02}",
            ts.format("%Y-%m-%dT%H"),
            ts.minute() / 15 * 15,
        );
        let key = (pair.1.clone(), bucket);
        if seen.insert(key) {
            deduped.push(pair);
        }
    }

    // Only keep hits from the last 30 days
    let cutoff = Utc::now() - chrono::Duration::days(30);
    deduped.retain(|(h, _)| h.timestamp >= cutoff);

    // Pre-cache first assistant timestamp per file (avoids reopening files per-hit)
    let file_first_ts: std::collections::HashMap<PathBuf, DateTime<Utc>> = file_root_pairs
        .par_iter()
        .filter_map(|(path, _)| {
            first_assistant_timestamp(path).map(|ts| (path.clone(), ts))
        })
        .collect();

    // Group cached timestamps by source root
    let ts_by_root: std::collections::HashMap<&str, Vec<DateTime<Utc>>> = {
        let mut map: std::collections::HashMap<&str, Vec<DateTime<Utc>>> =
            std::collections::HashMap::new();
        for (path, root_str) in &file_root_pairs {
            if let Some(&ts) = file_first_ts.get(path) {
                map.entry(root_str.as_str()).or_default().push(ts);
            }
        }
        map
    };

    let mut hits: Vec<RateLimitHit> = deduped
        .into_iter()
        .map(|(raw, root_str)| {
            let duration = ts_by_root
                .get(root_str.as_str())
                .and_then(|timestamps| {
                    let window_start = raw.timestamp - chrono::Duration::hours(5);
                    timestamps
                        .iter()
                        .filter(|ts| **ts >= window_start && **ts <= raw.timestamp)
                        .min()
                        .map(|first| (raw.timestamp - *first).num_seconds() as f64 / 60.0)
                });
            RateLimitHit {
                timestamp: raw.timestamp,
                message: raw.message,
                source_root: root_str,
                session_duration_min: duration,
            }
        })
        .collect();

    // Sort most recent first
    hits.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    hits
}

fn collect_jsonl_recursive(dir: &Path, root_str: &str, out: &mut Vec<(PathBuf, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") && path.is_file() {
            out.push((path, root_str.to_string()));
        } else if path.is_dir() {
            collect_jsonl_recursive(&path, root_str, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ccmeter_test_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_jsonl(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
        path
    }

    #[test]
    fn detects_rate_limit_hit() {
        let tmp = make_tmp_dir("detect");
        write_jsonl(
            &tmp,
            "session.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.000Z","message":{"model":"claude-opus-4-6","usage":{"input_tokens":100,"output_tokens":50}}}"#,
                r#"{"type":"assistant","timestamp":"2026-04-01T14:00:00.000Z","message":{"content":[{"type":"text","text":"You've hit your limit · resets 6pm"}]},"error":"rate_limit","isApiErrorMessage":true}"#,
            ],
        );
        let hits = discover_rate_limit_hits(&[tmp.clone()]);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].message.contains("hit your limit"));
        let dur = hits[0].session_duration_min.unwrap();
        assert!((dur - 120.0).abs() < 1.0);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn ignores_non_api_error() {
        let tmp = make_tmp_dir("ignore");
        write_jsonl(
            &tmp,
            "session.jsonl",
            &[
                r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.000Z","message":{"content":[{"type":"text","text":"rate_limit mentioned in conversation"}]}}"#,
            ],
        );
        let hits = discover_rate_limit_hits(&[tmp.clone()]);
        assert!(hits.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn deduplicates_same_minute() {
        let tmp = make_tmp_dir("dedup");
        let sub = tmp.join("sub");
        std::fs::create_dir(&sub).unwrap();
        let rl_line = r#"{"type":"assistant","timestamp":"2026-04-01T14:00:30.000Z","message":{"content":[{"type":"text","text":"limit hit"}]},"error":"rate_limit","isApiErrorMessage":true}"#;
        write_jsonl(&tmp, "a.jsonl", &[rl_line]);
        write_jsonl(&sub, "b.jsonl", &[rl_line]);
        let hits = discover_rate_limit_hits(&[tmp.clone()]);
        assert_eq!(hits.len(), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
