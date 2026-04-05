use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single usage event extracted from a JSONL line.
#[derive(Debug, Clone)]
pub struct Event {
    pub timestamp: DateTime<Utc>,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cost_usd: f64,
    /// Lines suggested by Claude (from Edit/Write tool_use in assistant messages).
    pub lines_suggested: u64,
    /// Lines actually accepted/applied (from structuredPatch in user messages).
    pub lines_accepted: u64,
    /// Lines added ('+' in patches, or new lines in diffs).
    pub lines_added: u64,
    /// Lines deleted ('-' in patches, or removed lines in diffs).
    pub lines_deleted: u64,
    /// Basename of the JSONL file (session UUID).
    pub session_file: String,
}

// ---------------------------------------------------------------------------
// Serde helpers — mirror the actual JSONL structure
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RawLine {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    line_type: Option<String>,
    message: Option<RawMessage>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
    #[serde(rename = "toolUseResult")]
    tool_use_result: Option<RawToolUseResult>,
}

#[derive(Deserialize)]
struct RawMessage {
    model: Option<String>,
    usage: Option<RawUsage>,
    content: Option<Vec<RawContentBlock>>,
}

#[derive(Deserialize)]
struct RawContentBlock {
    #[serde(rename = "type")]
    block_type: Option<String>,
    name: Option<String>,
    input: Option<RawToolInput>,
}

#[derive(Deserialize)]
struct RawToolInput {
    old_string: Option<String>,
    new_string: Option<String>,
}

#[derive(Deserialize)]
struct RawUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct RawToolUseResult {
    #[serde(rename = "structuredPatch")]
    structured_patch: Option<Vec<RawPatchHunk>>,
    #[serde(rename = "originalFile")]
    original_file: Option<String>,
    content: Option<String>,
    #[serde(rename = "oldString")]
    old_string: Option<String>,
}

#[derive(Deserialize)]
struct RawPatchHunk {
    lines: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a list of JSONL files in parallel and return all events.
pub fn parse_session_files(paths: &[PathBuf]) -> Vec<Event> {
    let mut events: Vec<Event> = paths
        .par_iter()
        .filter_map(|p| parse_one_file(p))
        .flatten()
        .collect();

    events.sort_by_key(|e| e.timestamp);
    events
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

fn parse_one_file(path: &Path) -> Option<Vec<Event>> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);

    let session_file = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();

    let events: Vec<Event> = reader
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.is_empty())
        .filter_map(|line| parse_line(&line, &session_file))
        .collect();

    Some(events)
}

/// Count the actual changed lines between old and new (not common/context lines).
/// Returns (added, removed).
fn count_diff_lines(old: Option<&str>, new: Option<&str>) -> (u64, u64) {
    let old_lines: Vec<&str> = old.unwrap_or("").lines().collect();
    let new_lines: Vec<&str> = new.unwrap_or("").lines().collect();

    // Build a multiset (HashMap) of old lines for O(n) matching.
    let mut old_counts: HashMap<&str, usize> = HashMap::new();
    for ol in &old_lines {
        *old_counts.entry(ol).or_default() += 1;
    }

    let mut added = 0u64;
    for nl in &new_lines {
        match old_counts.get_mut(nl) {
            Some(count) if *count > 0 => *count -= 1,
            _ => added += 1,
        }
    }
    let removed: u64 = old_counts.values().sum::<usize>() as u64;
    (added, removed)
}

/// Compute cost for an event, using costUSD if available, otherwise model pricing.
fn compute_cost(
    cost_usd: f64,
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read: u64,
) -> f64 {
    if cost_usd > 0.0 {
        cost_usd
    } else {
        let (input_price, output_price, cache_read_price) = super::models::model_pricing(model);
        let fresh_input = input_tokens.saturating_sub(cache_read);
        (fresh_input as f64 * input_price
            + cache_read as f64 * cache_read_price
            + output_tokens as f64 * output_price)
            / super::models::TOKENS_PER_MILLION
    }
}

/// Try to extract an Event from a single JSON line.
/// Handles both assistant messages (tokens + suggested lines) and user messages (accepted lines).
fn parse_line(line: &str, session_file: &str) -> Option<Event> {
    let raw: RawLine = serde_json::from_str(line).ok()?;

    let ts_str = raw.timestamp?;
    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
        .ok()?
        .with_timezone(&Utc);

    match raw.line_type.as_deref() {
        Some("assistant") => {
            let msg = raw.message?;
            let usage = msg.usage?;

            let model = msg.model.unwrap_or_default();
            let input_tokens = usage.input_tokens.unwrap_or(0);
            let output_tokens = usage.output_tokens.unwrap_or(0);
            let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
            let cache_creation = usage.cache_creation_input_tokens.unwrap_or(0);
            let raw_cost = raw.cost_usd.unwrap_or(0.0);
            let cost_usd = compute_cost(raw_cost, &model, input_tokens, output_tokens, cache_read);

            // Count lines suggested via Edit/Write tool_use blocks
            let mut lines_suggested = 0u64;
            let mut lines_added_total = 0u64;
            let mut lines_deleted_total = 0u64;
            if let Some(content) = &msg.content {
                for block in content {
                    if block.block_type.as_deref() != Some("tool_use") {
                        continue;
                    }
                    if let Some(input) = &block.input {
                        match block.name.as_deref() {
                            Some("Edit") => {
                                let (added, removed) = count_diff_lines(
                                    input.old_string.as_deref(),
                                    input.new_string.as_deref(),
                                );
                                lines_suggested += added + removed;
                                lines_added_total += added;
                                lines_deleted_total += removed;
                            }
                            Some("Write") => {
                                // Write suggested lines are computed on the
                                // user/accepted side where we have originalFile.
                            }
                            _ => {}
                        }
                    }
                }
            }

            Some(Event {
                timestamp,
                model,
                input_tokens,
                output_tokens,
                cache_read_input_tokens: cache_read,
                cache_creation_input_tokens: cache_creation,
                cost_usd,
                lines_suggested,
                lines_accepted: 0,
                lines_added: lines_added_total,
                lines_deleted: lines_deleted_total,
                session_file: session_file.to_owned(),
            })
        }
        Some("user") => {
            let tur = raw.tool_use_result?;
            let patches = tur.structured_patch?;

            let mut patch_added = 0u64;
            let mut patch_deleted = 0u64;
            for hunk in &patches {
                if let Some(lines) = &hunk.lines {
                    for l in lines {
                        if l.starts_with('+') {
                            patch_added += 1;
                        } else if l.starts_with('-') {
                            patch_deleted += 1;
                        }
                    }
                }
            }
            if patch_added + patch_deleted == 0 {
                return None;
            }

            let is_write = tur.old_string.is_none() && tur.content.is_some();
            let (suggested, accepted, added, deleted) = if is_write {
                // Write: compute real diff from originalFile vs content
                let (a, d) = count_diff_lines(tur.original_file.as_deref(), tur.content.as_deref());
                // Both suggested and accepted = the diff (change was accepted)
                (a + d, a + d, a, d)
            } else {
                // Edit: accepted = patch lines, suggested counted on assistant side
                (0, patch_added + patch_deleted, patch_added, patch_deleted)
            };

            Some(Event {
                timestamp,
                model: String::new(),
                input_tokens: 0,
                output_tokens: 0,
                cache_read_input_tokens: 0,
                cache_creation_input_tokens: 0,
                cost_usd: 0.0,
                lines_suggested: suggested,
                lines_accepted: accepted,
                lines_added: added,
                lines_deleted: deleted,
                session_file: session_file.to_owned(),
            })
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_line(json: &str) -> String {
        json.to_string()
    }

    #[test]
    fn parses_valid_assistant_line() {
        let line = make_line(
            r#"{
            "type": "assistant",
            "timestamp": "2026-04-01T12:00:00.000Z",
            "message": {
                "model": "claude-opus-4-6",
                "usage": {
                    "input_tokens": 1000,
                    "output_tokens": 200,
                    "cache_read_input_tokens": 500,
                    "cache_creation_input_tokens": 300
                }
            }
        }"#,
        );

        let ev = parse_line(&line, "test.jsonl").expect("should parse");
        assert_eq!(ev.model, "claude-opus-4-6");
        assert_eq!(ev.input_tokens, 1000);
        assert_eq!(ev.output_tokens, 200);
        assert_eq!(ev.cache_read_input_tokens, 500);
        assert_eq!(ev.cache_creation_input_tokens, 300);
        // cost computed from model pricing: (500*5.0 + 500*0.50 + 200*25.0) / 1M
        assert!((ev.cost_usd - 0.00775).abs() < 1e-9);
        assert_eq!(ev.lines_suggested, 0);
        assert_eq!(ev.lines_added, 0);
        assert_eq!(ev.lines_deleted, 0);
    }

    #[test]
    fn skips_user_lines() {
        let line = make_line(
            r#"{
            "type": "user",
            "timestamp": "2026-04-01T12:00:00.000Z",
            "cwd": "/tmp"
        }"#,
        );
        assert!(parse_line(&line, "test.jsonl").is_none());
    }

    #[test]
    fn skips_missing_timestamp() {
        let line = make_line(
            r#"{
            "type": "assistant",
            "message": {
                "model": "claude-opus-4-6",
                "usage": { "input_tokens": 10, "output_tokens": 5 }
            }
        }"#,
        );
        assert!(parse_line(&line, "test.jsonl").is_none());
    }

    #[test]
    fn skips_missing_usage() {
        let line = make_line(
            r#"{
            "type": "assistant",
            "timestamp": "2026-04-01T12:00:00.000Z",
            "message": { "model": "claude-opus-4-6" }
        }"#,
        );
        assert!(parse_line(&line, "test.jsonl").is_none());
    }

    #[test]
    fn handles_optional_cost() {
        let line = make_line(
            r#"{
            "type": "assistant",
            "timestamp": "2026-04-01T12:00:00.000Z",
            "costUSD": 1.23,
            "message": {
                "model": "claude-sonnet-4-6",
                "usage": { "input_tokens": 10, "output_tokens": 5 }
            }
        }"#,
        );

        let ev = parse_line(&line, "test.jsonl").expect("should parse");
        assert!((ev.cost_usd - 1.23).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_user_patch_lines() {
        let line = make_line(
            r#"{
            "type": "user",
            "timestamp": "2026-04-01T12:00:00.000Z",
            "toolUseResult": {
                "structuredPatch": [
                    { "lines": ["+added1", "+added2", "-removed1", " context"] }
                ]
            }
        }"#,
        );

        let ev = parse_line(&line, "test.jsonl").expect("should parse");
        assert_eq!(ev.lines_accepted, 3); // 2 added + 1 removed
        assert_eq!(ev.lines_added, 2); // "+added1", "+added2"
        assert_eq!(ev.lines_deleted, 1); // "-removed1"
        assert_eq!(ev.input_tokens, 0);
    }

    #[test]
    fn skips_garbage_line() {
        assert!(parse_line("not json at all", "test.jsonl").is_none());
        assert!(parse_line("", "test.jsonl").is_none());
        assert!(parse_line("{}", "test.jsonl").is_none());
    }

    #[test]
    fn tolerates_missing_optional_usage_fields() {
        let line = make_line(
            r#"{
            "type": "assistant",
            "timestamp": "2026-04-01T12:00:00.000Z",
            "message": {
                "model": "claude-haiku-4-5",
                "usage": { "input_tokens": 100, "output_tokens": 50 }
            }
        }"#,
        );

        let ev = parse_line(&line, "test.jsonl").expect("should parse");
        assert_eq!(ev.cache_read_input_tokens, 0);
        assert_eq!(ev.cache_creation_input_tokens, 0);
    }

    #[test]
    fn tolerates_extra_fields() {
        let line = make_line(
            r#"{
            "type": "assistant",
            "timestamp": "2026-04-01T12:00:00.000Z",
            "sessionId": "abc-123",
            "uuid": "def-456",
            "parentUuid": null,
            "cwd": "/tmp",
            "requestId": "req_xyz",
            "message": {
                "model": "claude-opus-4-6",
                "id": "msg_abc",
                "role": "assistant",
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 10,
                    "output_tokens": 5,
                    "service_tier": "standard"
                }
            }
        }"#,
        );

        let ev = parse_line(&line, "test.jsonl").expect("should parse");
        assert_eq!(ev.model, "claude-opus-4-6");
    }
}
