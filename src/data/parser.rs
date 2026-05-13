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
    /// Canonical dedup key for assistant events. Prefers Anthropic's API
    /// request id (`req_…`); falls back to `message.id` (`msg_…`) when the
    /// JSONL omits `requestId` — the case for proxied API calls (corporate
    /// Bedrock gateways, third-party LLM proxies) that strip request
    /// headers. Both id-spaces are globally unique, so either works for
    /// deduping events rewritten across files (sub-agent mirrors,
    /// auto-compact retries) or across streaming chunks within one file.
    /// `None` for user-side events (`lines_accepted`, etc).
    pub request_id: Option<String>,
    /// Raw `costUSD` read verbatim from the JSONL (the authoritative billed
    /// amount, cumulative per streaming snapshot when emitted by Claude
    /// Code). Preserved separately from `cost_usd` so `deltaize_stream` can
    /// delta the cumulative rather than fall back to a token-pricing
    /// approximation. `None` when the field is absent (Pro-plan logs, which
    /// never carry `costUSD`).
    pub raw_cost_usd: Option<f64>,
    /// Per-line UUID from the JSONL. Used to dedup user-side patch events
    /// that get mirrored across parent and sub-agent files (same uuid, same
    /// content). Assistant events are deduped by `request_id` instead.
    pub line_uuid: Option<String>,
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
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    /// Per-line UUID assigned by Claude Code. Preserved byte-identical when
    /// the same event is mirrored into a sub-agent's JSONL, so we can use it
    /// to dedup user-side patch events (which have no `requestId`).
    uuid: Option<String>,
}

#[derive(Deserialize)]
struct RawMessage {
    /// Anthropic message id (`msg_…`). Globally unique per API call, stable
    /// across the streaming content-block lines Claude Code writes for one
    /// response. Used as the dedup-key fallback when `requestId` is absent.
    id: Option<String>,
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

/// Parse a list of JSONL files in parallel and dedup so totals match
/// Anthropic's billing (1 `request_id` = 1 HTTP request = 1 invoice line).
/// See `dedup_by_request_id` for the canonicalization algorithm.
pub fn parse_session_files(paths: &[PathBuf]) -> Vec<Event> {
    let raw: Vec<Event> = paths
        .par_iter()
        .filter_map(|p| parse_one_file(p))
        .flatten()
        .collect();

    dedup_by_request_id(raw)
}

fn dedup_by_request_id(events: Vec<Event>) -> Vec<Event> {
    let total = events.len();
    let mut without_req: Vec<Event> = Vec::new();
    let mut by_req: HashMap<String, HashMap<String, Vec<Event>>> = HashMap::new();
    for e in events {
        match &e.request_id {
            Some(id) => {
                let inner = by_req.entry(id.clone()).or_default();
                // Avoid cloning session_file on every event: only clone on
                // the first chunk per stream (i.e. per unique file).
                if let Some(stream) = inner.get_mut(&e.session_file) {
                    stream.push(e);
                } else {
                    inner.insert(e.session_file.clone(), vec![e]);
                }
            }
            None => without_req.push(e),
        }
    }

    // User-side events (patches) have no requestId but carry a per-line
    // `uuid` Claude Code preserves byte-identical across sub-agent mirrors.
    // Dedup by uuid; events without a uuid (older logs) pass through
    // unchanged — under-counting is worse than a rare over-count.
    let mut out: Vec<Event> = Vec::with_capacity(total);
    let mut seen_uuids: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(without_req.len());
    for e in without_req {
        match &e.line_uuid {
            Some(uuid) => {
                if seen_uuids.insert(uuid.clone()) {
                    out.push(e);
                }
            }
            None => out.push(e),
        }
    }

    for (_req, files) in by_req {
        let mut streams: Vec<Vec<Event>> = files.into_values().collect();
        // Sort streams by `session_file` for deterministic canonical pick:
        // HashMap iteration is randomized, and two mirrors with identical
        // `stream_score` would otherwise flip which one wins, making
        // provenance-asserting tests flaky.
        streams.sort_by(|a, b| {
            let ka = a.first().map(|e| e.session_file.as_str()).unwrap_or("");
            let kb = b.first().map(|e| e.session_file.as_str()).unwrap_or("");
            ka.cmp(kb)
        });
        // Sort chunks by timestamp so deltaization sees snapshots in order.
        for s in &mut streams {
            s.sort_by_key(|e| e.timestamp);
        }
        let Some(canonical_idx) = pick_canonical_index(&streams) else {
            continue;
        };

        // Ghost chunks emit per non-canonical timestamp not covered by the
        // canonical. Billing/model are zeroed so they can't inflate totals
        // or leak into `index::model_shares` (cleared model → `ModelId::Other`,
        // filtered by `total > 0`); line metrics are preserved because a
        // mirror timestamp absent from the canonical necessarily carries
        // unique Edit/Write content. Dedup by `timestamp` alone is safe:
        // mirrors are byte-identical at matching timestamps.
        let canonical_ts: std::collections::HashSet<DateTime<Utc>> =
            streams[canonical_idx].iter().map(|e| e.timestamp).collect();
        // Debug-only invariant guard: if two non-canonical streams land on
        // the same timestamp with different `lines_*`, the mirror format
        // has diverged and timestamp-only dedup would drop distinct content.
        let mut ghost_seen: std::collections::HashMap<DateTime<Utc>, (u64, u64, u64, u64)> =
            std::collections::HashMap::new();
        let mut ghosts: Vec<Event> = Vec::new();
        for (i, stream) in streams.iter().enumerate() {
            if i == canonical_idx {
                continue;
            }
            for e in stream {
                if canonical_ts.contains(&e.timestamp) {
                    continue;
                }
                let metrics = (
                    e.lines_suggested,
                    e.lines_added,
                    e.lines_deleted,
                    e.lines_accepted,
                );
                if let Some(prev) = ghost_seen.get(&e.timestamp) {
                    debug_assert_eq!(
                        prev, &metrics,
                        "mirror invariant violated at {}: {:?} vs {:?}",
                        e.timestamp, prev, metrics
                    );
                    continue;
                }
                ghost_seen.insert(e.timestamp, metrics);
                ghosts.push(Event {
                    timestamp: e.timestamp,
                    model: String::new(),
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cost_usd: 0.0,
                    lines_suggested: e.lines_suggested,
                    lines_accepted: e.lines_accepted,
                    lines_added: e.lines_added,
                    lines_deleted: e.lines_deleted,
                    session_file: e.session_file.clone(),
                    request_id: None,
                    raw_cost_usd: None,
                    line_uuid: None,
                });
            }
        }

        let canonical = streams.swap_remove(canonical_idx);
        out.extend(deltaize_stream(canonical));
        out.extend(ghosts);
    }

    out.sort_by_key(|e| e.timestamp);
    out
}

/// Score a stream by `(max_total_usage, chunk_count, lines_sum,
/// latest_timestamp)` and return the index of the stream with the highest
/// score. Ties are broken lexicographically: richer billing > more chunks
/// > more content > later.
///
/// Returns the index (rather than the stream itself) so the caller can keep
/// borrowing the other streams for ghost-chunk extraction before moving the
/// canonical out.
fn pick_canonical_index(streams: &[Vec<Event>]) -> Option<usize> {
    streams
        .iter()
        .enumerate()
        .max_by_key(|(_, s)| stream_score(s))
        .map(|(i, _)| i)
}

type StreamScore = (u64, usize, u64, DateTime<Utc>);

fn stream_score(s: &[Event]) -> StreamScore {
    let max_total = s
        .iter()
        .map(|e| {
            e.input_tokens
                + e.output_tokens
                + e.cache_read_input_tokens
                + e.cache_creation_input_tokens
        })
        .max()
        .unwrap_or(0);
    let chunks = s.len();
    let lines_sum: u64 = s
        .iter()
        .map(|e| e.lines_suggested + e.lines_added + e.lines_deleted)
        .sum();
    let latest_ts = s
        .iter()
        .map(|e| e.timestamp)
        .max()
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).unwrap());
    (max_total, chunks, lines_sum, latest_ts)
}

/// Convert cumulative snapshots into per-chunk deltas. First chunk's delta
/// equals its full snapshot (implicit `prev = 0`).
///
/// **Cost handling** — two paths, in order of fidelity:
/// 1. If **every** chunk in the stream carries a raw `costUSD` (API logs
///    where Claude Code records Anthropic's billed amount on each snapshot),
///    delta that cumulative value. This preserves whatever pricing
///    Anthropic actually applied — promo rates, cache-creation surcharges,
///    post-launch price changes the local table may not know about.
/// 2. Otherwise (Pro-plan logs, or any stream with even a single chunk
///    missing `costUSD`), recompute from delta tokens via `model_pricing`.
///
/// The all-or-none gate on path 1 is deliberate. A mixed stream (some raw
/// cost, some missing) cannot be reconciled correctly: emitting a
/// token-pricing estimate on the gap chunks would not advance the raw
/// cumulative baseline, so the next raw-cost chunk would re-emit spend
/// already covered by the gap estimates — inflating the total. Falling
/// back uniformly to tokens keeps the total internally consistent at the
/// cost of one stream's raw-cost fidelity.
fn deltaize_stream(mut stream: Vec<Event>) -> Vec<Event> {
    let mut prev_in = 0u64;
    let mut prev_out = 0u64;
    let mut prev_cr = 0u64;
    let mut prev_cc = 0u64;
    let mut prev_raw_cost = 0.0_f64;

    let use_raw_cost = !stream.is_empty() && stream.iter().all(|e| e.raw_cost_usd.is_some());

    for e in stream.iter_mut() {
        let d_in = e.input_tokens.saturating_sub(prev_in);
        let d_out = e.output_tokens.saturating_sub(prev_out);
        let d_cr = e.cache_read_input_tokens.saturating_sub(prev_cr);
        let d_cc = e.cache_creation_input_tokens.saturating_sub(prev_cc);

        // Snapshot monotonicity: advance only when the new value is larger,
        // so a transient regression (e.g. unordered mirror) doesn't poison
        // subsequent deltas.
        prev_in = prev_in.max(e.input_tokens);
        prev_out = prev_out.max(e.output_tokens);
        prev_cr = prev_cr.max(e.cache_read_input_tokens);
        prev_cc = prev_cc.max(e.cache_creation_input_tokens);

        e.input_tokens = d_in;
        e.output_tokens = d_out;
        e.cache_read_input_tokens = d_cr;
        e.cache_creation_input_tokens = d_cc;

        e.cost_usd = if use_raw_cost {
            let cur_cost = e
                .raw_cost_usd
                .expect("use_raw_cost invariant: every chunk has raw cost");
            let delta = (cur_cost - prev_raw_cost).max(0.0);
            prev_raw_cost = prev_raw_cost.max(cur_cost);
            delta
        } else {
            cost_from_tokens(&e.model, d_in, d_out, d_cr, d_cc)
        };
        // lines_* are per-chunk (not cumulative) — preserve as-is.
    }

    stream
}

/// Cost approximation from token counts via the local pricing table. Used
/// as fallback when the JSONL doesn't carry a raw `costUSD`. Mirrors
/// Anthropic's billing structure: fresh input at `input_price`, cache reads
/// at `cache_read_price`, cache creation at `input_price × 1.25` (the
/// default 5-min ephemeral TTL that Claude Code uses), output at
/// `output_price`.
fn cost_from_tokens(
    model: &str,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
) -> f64 {
    // Mirrors `CACHE_CREATION_WEIGHT` in `index::split_entry_cost`.
    const CACHE_CREATION_MULTIPLIER: f64 = 1.25;
    let (input_price, output_price, cache_read_price) = super::models::model_pricing(model);
    let fresh_input = input.saturating_sub(cache_read);
    (fresh_input as f64 * input_price
        + cache_read as f64 * cache_read_price
        + cache_creation as f64 * input_price * CACHE_CREATION_MULTIPLIER
        + output as f64 * output_price)
        / super::models::TOKENS_PER_MILLION
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

            // Prefer Anthropic's `requestId` (`req_…`); fall back to
            // `message.id` (`msg_…`) when absent. Proxied gateways (corporate
            // Bedrock proxies, third-party LLM relays) often strip the
            // request header but preserve the message id. Without this
            // fallback, every chunk lands in `without_req` and the
            // line-uuid dedup never fires (each chunk has a unique uuid),
            // so streaming responses with N content-block lines get summed
            // N times — observed inflation: 2–3× on proxy users.
            let request_id = raw.request_id.or_else(|| msg.id.clone());
            let line_uuid = raw.uuid.clone();
            let model = msg.model.unwrap_or_default();
            let input_tokens = usage.input_tokens.unwrap_or(0);
            let output_tokens = usage.output_tokens.unwrap_or(0);
            let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
            let cache_creation = usage.cache_creation_input_tokens.unwrap_or(0);
            let raw_cost_usd = raw.cost_usd;
            let cost_usd = raw_cost_usd.filter(|c| *c > 0.0).unwrap_or_else(|| {
                cost_from_tokens(
                    &model,
                    input_tokens,
                    output_tokens,
                    cache_read,
                    cache_creation,
                )
            });

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
                request_id,
                raw_cost_usd,
                line_uuid,
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
                request_id: None,
                raw_cost_usd: None,
                line_uuid: raw.uuid,
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
    use chrono::Timelike;

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
        // cost from model pricing:
        //   fresh_input 500 * $5/M + cache_read 500 * $0.5/M
        //   + cache_creation 300 * $5/M * 1.25 + output 200 * $25/M
        //   = 2500 + 250 + 1875 + 5000 = 9625 micro-USD
        assert!((ev.cost_usd - 0.009625).abs() < 1e-9);
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

    #[test]
    fn captures_request_id() {
        let line = make_line(
            r#"{
            "type": "assistant",
            "timestamp": "2026-04-01T12:00:00.000Z",
            "requestId": "req_01abc",
            "message": {
                "id": "msg_01abc",
                "model": "claude-opus-4-6",
                "usage": { "input_tokens": 1, "output_tokens": 1 }
            }
        }"#,
        );
        let ev = parse_line(&line, "test.jsonl").expect("should parse");
        assert_eq!(ev.request_id.as_deref(), Some("req_01abc"));
    }

    // ------------------------------------------------------------------
    // Unit tests on deltaize_stream / stream_score
    // ------------------------------------------------------------------

    fn mk_event(
        ts: &str,
        req: &str,
        file: &str,
        model: &str,
        (i, o, cr, cc): (u64, u64, u64, u64),
        (ls, la, ld): (u64, u64, u64),
    ) -> Event {
        mk_event_with_cost(ts, req, file, model, (i, o, cr, cc), (ls, la, ld), None)
    }

    fn mk_event_with_cost(
        ts: &str,
        req: &str,
        file: &str,
        model: &str,
        (i, o, cr, cc): (u64, u64, u64, u64),
        (ls, la, ld): (u64, u64, u64),
        raw_cost_usd: Option<f64>,
    ) -> Event {
        Event {
            timestamp: DateTime::parse_from_rfc3339(ts)
                .unwrap()
                .with_timezone(&Utc),
            model: model.into(),
            input_tokens: i,
            output_tokens: o,
            cache_read_input_tokens: cr,
            cache_creation_input_tokens: cc,
            cost_usd: 0.0,
            lines_suggested: ls,
            lines_accepted: 0,
            lines_added: la,
            lines_deleted: ld,
            session_file: file.into(),
            request_id: Some(req.into()),
            raw_cost_usd,
            line_uuid: None,
        }
    }

    #[test]
    fn first_chunk_delta_equals_full_snapshot() {
        let stream = vec![mk_event(
            "2026-04-01T12:00:00.100Z",
            "r",
            "f.jsonl",
            "claude-opus-4-6",
            (3, 21, 6754, 13837),
            (0, 0, 0),
        )];
        let out = deltaize_stream(stream);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].input_tokens, 3);
        assert_eq!(out[0].output_tokens, 21);
        assert_eq!(out[0].cache_read_input_tokens, 6754);
        assert_eq!(out[0].cache_creation_input_tokens, 13837);
    }

    #[test]
    fn delta_handles_non_monotonic_snapshot_defensively() {
        // output regresses mid-stream (shouldn't happen in real data, but
        // saturating_sub + monotonic prev must prevent panic & underflow).
        let stream = vec![
            mk_event(
                "2026-04-01T12:00:00.100Z",
                "r",
                "f.jsonl",
                "claude-opus-4-6",
                (3, 50, 100, 0),
                (0, 0, 0),
            ),
            mk_event(
                "2026-04-01T12:00:00.200Z",
                "r",
                "f.jsonl",
                "claude-opus-4-6",
                (3, 30, 100, 0),
                (0, 0, 0),
            ), // regression
            mk_event(
                "2026-04-01T12:00:00.300Z",
                "r",
                "f.jsonl",
                "claude-opus-4-6",
                (3, 80, 100, 0),
                (0, 0, 0),
            ),
        ];
        let out = deltaize_stream(stream);
        // Expected deltas: out (50, 0, 30), since prev tracks the max.
        assert_eq!(out[0].output_tokens, 50);
        assert_eq!(out[1].output_tokens, 0);
        assert_eq!(out[2].output_tokens, 30);
        // Total reconciles to the max snapshot (80), not the last (80).
        let sum_out: u64 = out.iter().map(|e| e.output_tokens).sum();
        assert_eq!(sum_out, 80);
    }

    #[test]
    fn delta_cost_uses_raw_cumulative_when_present() {
        // Three chunks with a cumulative raw costUSD that diverges from the
        // pricing-table estimate — e.g. Anthropic applied promo pricing.
        // We must preserve Anthropic's billed amount via delta, not fall
        // back to the token-pricing approximation.
        let stream = vec![
            mk_event_with_cost(
                "2026-04-01T12:00:00.100Z",
                "r",
                "f.jsonl",
                "claude-opus-4-6",
                (100, 10, 500, 0),
                (0, 0, 0),
                Some(0.42),
            ),
            mk_event_with_cost(
                "2026-04-01T12:00:00.200Z",
                "r",
                "f.jsonl",
                "claude-opus-4-6",
                (100, 30, 500, 0),
                (0, 0, 0),
                Some(1.00),
            ),
            mk_event_with_cost(
                "2026-04-01T12:00:00.300Z",
                "r",
                "f.jsonl",
                "claude-opus-4-6",
                (100, 80, 500, 0),
                (0, 0, 0),
                Some(2.50),
            ),
        ];
        let out = deltaize_stream(stream);
        // Per-chunk deltas: 0.42, 0.58, 1.50. Sum = 2.50 (final cumulative).
        assert!((out[0].cost_usd - 0.42).abs() < 1e-9);
        assert!((out[1].cost_usd - 0.58).abs() < 1e-9);
        assert!((out[2].cost_usd - 1.50).abs() < 1e-9);
        let sum: f64 = out.iter().map(|e| e.cost_usd).sum();
        assert!(
            (sum - 2.50).abs() < 1e-9,
            "sum must reconstitute final raw cumulative cost"
        );

        // Sanity: differs from the token-pricing fallback (0.80 approx), so
        // we really preserved Anthropic's authoritative figure.
        let fallback = cost_from_tokens("claude-opus-4-6", 100, 80, 500, 0);
        assert!(
            (sum - fallback).abs() > 1e-3,
            "test invariant: raw cost must diverge from token-pricing estimate"
        );
    }

    #[test]
    fn mixed_raw_cost_stream_falls_back_to_tokens_without_inflating() {
        // Mixed streams must fall back to tokens uniformly. Mixing a token
        // estimate on a gap chunk with a delta of the next raw cumulative
        // would double-emit the gap's spend (raw cumulative already includes
        // it) and inflate the total.
        let stream = vec![
            mk_event_with_cost(
                "2026-04-01T12:00:00.100Z",
                "r",
                "f.jsonl",
                "claude-opus-4-6",
                (100, 10, 500, 0),
                (0, 0, 0),
                Some(0.42),
            ),
            mk_event_with_cost(
                "2026-04-01T12:00:00.200Z",
                "r",
                "f.jsonl",
                "claude-opus-4-6",
                (100, 30, 500, 0),
                (0, 0, 0),
                None, // gap
            ),
            mk_event_with_cost(
                "2026-04-01T12:00:00.300Z",
                "r",
                "f.jsonl",
                "claude-opus-4-6",
                (100, 80, 500, 0),
                (0, 0, 0),
                Some(1.00),
            ),
        ];
        let out = deltaize_stream(stream);

        // All-or-none: stream falls back to cost_from_tokens for every chunk.
        // Total must equal the token-pricing cost of the final snapshot
        // (100, 80, 500), never the inflated 1.30 that mixed delta gave.
        let sum: f64 = out.iter().map(|e| e.cost_usd).sum();
        let expected = cost_from_tokens("claude-opus-4-6", 100, 80, 500, 0);
        assert!(
            (sum - expected).abs() < 1e-9,
            "mixed stream must fall back to token pricing, got {sum} expected {expected}"
        );

        // And critically: the inflation path (0.42 + token_gap + 0.58 ≈ 1.30)
        // must not happen.
        assert!(sum < 1.30 - 1e-3, "must not inflate via hybrid delta");
    }

    #[test]
    fn delta_cost_sums_to_final_cost() {
        // Opus 4-6 pricing: in=5, out=25, cache_read=0.5 per M tokens.
        let stream = vec![
            mk_event(
                "2026-04-01T12:00:00.100Z",
                "r",
                "f.jsonl",
                "claude-opus-4-6",
                (100, 10, 500, 0),
                (0, 0, 0),
            ),
            mk_event(
                "2026-04-01T12:00:00.200Z",
                "r",
                "f.jsonl",
                "claude-opus-4-6",
                (100, 30, 500, 0),
                (0, 0, 0),
            ),
            mk_event(
                "2026-04-01T12:00:00.300Z",
                "r",
                "f.jsonl",
                "claude-opus-4-6",
                (100, 80, 500, 0),
                (0, 0, 0),
            ),
        ];
        let out = deltaize_stream(stream);
        let sum_cost: f64 = out.iter().map(|e| e.cost_usd).sum();
        let final_cost = cost_from_tokens("claude-opus-4-6", 100, 80, 500, 0);
        assert!(
            (sum_cost - final_cost).abs() < 1e-9,
            "sum of deltaized costs ({sum_cost}) must equal final cost ({final_cost})"
        );
    }

    #[test]
    fn canonical_stream_prefers_richer_file_on_mirror_tie() {
        // Two streams with the SAME max_total. Winner must be the one with
        // more chunks (proxy for completeness) and more lines.
        let rich = vec![
            mk_event(
                "2026-04-01T12:00:00.100Z",
                "r",
                "rich.jsonl",
                "claude-opus-4-6",
                (3, 21, 1000, 0),
                (0, 0, 0),
            ),
            mk_event(
                "2026-04-01T12:00:00.200Z",
                "r",
                "rich.jsonl",
                "claude-opus-4-6",
                (3, 100, 1000, 0),
                (0, 0, 0),
            ), // edit block
            mk_event(
                "2026-04-01T12:00:00.300Z",
                "r",
                "rich.jsonl",
                "claude-opus-4-6",
                (3, 200, 1000, 0),
                (40, 20, 20),
            ), // tool_use Edit, lines=40
        ];
        // Poor stream: single terminal snapshot, SAME max_total, but a later
        // timestamp (would have won the old latest-ts tiebreak).
        let poor = vec![mk_event(
            "2026-04-01T12:00:00.400Z", // LATER than rich's last chunk
            "r",
            "poor.jsonl",
            "claude-opus-4-6",
            (3, 200, 1000, 0),
            (0, 0, 0),
        )];
        let streams = vec![rich.clone(), poor.clone()];
        let idx = pick_canonical_index(&streams).unwrap();
        let canonical = &streams[idx];
        assert_eq!(canonical.len(), 3, "canonical must be the rich stream");
        assert_eq!(canonical[0].session_file, "rich.jsonl");
        // lines survive
        let total_lines: u64 = canonical.iter().map(|e| e.lines_suggested).sum();
        assert_eq!(total_lines, 40);
    }

    // ------------------------------------------------------------------
    // Integration tests via parse_session_files (tmp files on disk)
    // ------------------------------------------------------------------

    fn tmp_dir(label: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ccmeter_{label}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_file(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        use std::io::Write;
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        p
    }

    #[test]
    fn preserves_lines_when_chunks_share_cumulative_total() {
        // Two chunks under the same requestId with IDENTICAL cumulative
        // snapshots (text chunk then Edit chunk — newer Claude Code writes
        // final usage on every content-block line). lines_suggested from
        // the Edit chunk must survive even though its tokens delta to zero.
        let dir = tmp_dir("same_total");
        let text_line = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.100Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","content":[{"type":"text"}],"usage":{"input_tokens":3,"output_tokens":200,"cache_read_input_tokens":1000,"cache_creation_input_tokens":0}}}"#;
        let edit_line = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.200Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","content":[{"type":"tool_use","name":"Edit","input":{"old_string":"a\nb\nc","new_string":"A\nB\nC"}}],"usage":{"input_tokens":3,"output_tokens":200,"cache_read_input_tokens":1000,"cache_creation_input_tokens":0}}}"#;
        let path = write_file(&dir, "s.jsonl", &[text_line, edit_line]);

        let events = parse_session_files(&[path]);
        let _ = std::fs::remove_dir_all(&dir);

        let total_lines: u64 = events.iter().map(|e| e.lines_suggested).sum();
        assert!(
            total_lines > 0,
            "lines_suggested must survive; got {total_lines}"
        );
        // Billing invariant: summed deltas = final snapshot.
        let sum_out: u64 = events.iter().map(|e| e.output_tokens).sum();
        assert_eq!(sum_out, 200);
    }

    #[test]
    fn sums_lines_across_chunks_within_same_request_same_file() {
        let dir = tmp_dir("sum_lines");
        let c1 = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.100Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","content":[{"type":"tool_use","name":"Edit","input":{"old_string":"a\nb","new_string":"A\nB"}}],"usage":{"input_tokens":3,"output_tokens":50,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}"#;
        let c2 = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.200Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","content":[{"type":"tool_use","name":"Edit","input":{"old_string":"x\ny\nz","new_string":"X\nY\nZ"}}],"usage":{"input_tokens":3,"output_tokens":100,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}"#;
        let path = write_file(&dir, "s.jsonl", &[c1, c2]);

        let events = parse_session_files(&[path]);
        let _ = std::fs::remove_dir_all(&dir);

        let total_sugg: u64 = events.iter().map(|e| e.lines_suggested).sum();
        // Each Edit contributes added+removed; exact count depends on diff logic.
        // Minimum: c1 contributes >= 2, c2 contributes >= 3 → total >= 5.
        assert!(
            total_sugg >= 5,
            "expected at least 5 suggested lines, got {total_sugg}"
        );
    }

    #[test]
    fn does_not_double_count_lines_mirrored_across_files() {
        let dir = tmp_dir("mirror_lines");
        let edit = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.100Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","content":[{"type":"tool_use","name":"Edit","input":{"old_string":"a\nb","new_string":"A\nB"}}],"usage":{"input_tokens":3,"output_tokens":50,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}"#;
        let parent = write_file(&dir, "parent.jsonl", &[edit]);
        let agent = write_file(&dir, "agent-sub.jsonl", &[edit]);

        let events_mirrored = parse_session_files(&[parent.clone(), agent.clone()]);
        let _ = std::fs::remove_dir_all(&dir);

        // Reference: same edit in a single file.
        let dir2 = tmp_dir("mirror_single");
        let solo = write_file(&dir2, "solo.jsonl", &[edit]);
        let events_solo = parse_session_files(&[solo]);
        let _ = std::fs::remove_dir_all(&dir2);

        let mirrored_lines: u64 = events_mirrored.iter().map(|e| e.lines_suggested).sum();
        let solo_lines: u64 = events_solo.iter().map(|e| e.lines_suggested).sum();
        assert_eq!(
            mirrored_lines, solo_lines,
            "mirror must not inflate lines_suggested"
        );
    }

    #[test]
    fn multi_minute_request_preserves_chunk_timestamps() {
        let dir = tmp_dir("multi_minute");
        // Single stream spanning 19:11 → 19:12 → 19:14 (gap of 2 min).
        // All three timestamps must survive dedup so active_minutes
        // clustering (5-min gap threshold) sees the full interval.
        let c1 = r#"{"type":"assistant","timestamp":"2026-04-01T19:11:00.000Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","usage":{"input_tokens":10,"output_tokens":50,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let c2 = r#"{"type":"assistant","timestamp":"2026-04-01T19:12:00.000Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","usage":{"input_tokens":10,"output_tokens":150,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let c3 = r#"{"type":"assistant","timestamp":"2026-04-01T19:14:00.000Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","usage":{"input_tokens":10,"output_tokens":300,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let path = write_file(&dir, "s.jsonl", &[c1, c2, c3]);

        let events = parse_session_files(&[path]);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(events.len(), 3, "all three chunk timestamps must survive");
        // Minutes observed:
        let mut minutes: Vec<_> = events
            .iter()
            .map(|e| (e.timestamp.hour(), e.timestamp.minute()))
            .collect();
        minutes.sort();
        assert_eq!(minutes, vec![(19, 11), (19, 12), (19, 14)]);
        // Billing: deltas sum to final snapshot.
        let sum_out: u64 = events.iter().map(|e| e.output_tokens).sum();
        assert_eq!(sum_out, 300);
    }

    #[test]
    fn ghosts_dedup_across_multiple_non_canonical_mirrors() {
        // 3-file scenario: late terminal canonical + 2 byte-identical mirror
        // streams with chunks at the same early timestamps. Without
        // timestamp-only dedup the ghosts would be emitted twice (once per
        // mirror), inflating both line metrics and session_minutes counts.
        let dir = tmp_dir("multi_mirror_ghosts");
        let early1 = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.100Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","content":[{"type":"tool_use","name":"Edit","input":{"old_string":"a","new_string":"A"}}],"usage":{"input_tokens":3,"output_tokens":50,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}"#;
        let early2 = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:01.100Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","usage":{"input_tokens":3,"output_tokens":150,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}"#;
        let late = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:05.000Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","usage":{"input_tokens":3,"output_tokens":300,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}"#;

        // Two mirror files with identical early chunks.
        let pa = write_file(&dir, "a.jsonl", &[early1, early2]);
        let pb = write_file(&dir, "b.jsonl", &[early1, early2]);
        // Canonical: terminal snapshot, higher max_total.
        let pc = write_file(&dir, "c.jsonl", &[late]);

        let events = parse_session_files(&[pa, pb, pc]);
        let _ = std::fs::remove_dir_all(&dir);

        // 1 canonical chunk + exactly 2 ghosts (one per unique non-canonical
        // timestamp), NOT 4 (which would be ghosts × 2 mirrors).
        assert_eq!(events.len(), 3, "ghosts must be deduped across mirrors");
        let ghosts: Vec<&Event> = events.iter().filter(|e| e.output_tokens == 0).collect();
        assert_eq!(ghosts.len(), 2);

        // Billing untouched.
        let sum_out: u64 = events.iter().map(|e| e.output_tokens).sum();
        assert_eq!(sum_out, 300);

        // Edit lines counted exactly once (3+3 not 6+6 for added/deleted=1
        // each via count_diff_lines("a","A")).
        let total_added: u64 = events.iter().map(|e| e.lines_added).sum();
        let total_deleted: u64 = events.iter().map(|e| e.lines_deleted).sum();
        assert_eq!(total_added, 1, "Edit added counted once across mirrors");
        assert_eq!(total_deleted, 1, "Edit deleted counted once across mirrors");
    }

    #[test]
    fn ghost_model_is_cleared_to_avoid_zero_token_model_share_leak() {
        // Ghost events keep a timestamp + line metrics but must not register
        // as model usage in downstream aggregators (`index::model_shares`
        // accepts known models even with total=0). Clearing `model` makes
        // them fall through to ModelId::Other and be filtered out.
        let dir = tmp_dir("ghost_model_cleared");
        let early = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.100Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","usage":{"input_tokens":3,"output_tokens":50,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}"#;
        let late = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:05.000Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","usage":{"input_tokens":3,"output_tokens":300,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}"#;
        let pa = write_file(&dir, "a.jsonl", &[early]);
        let pb = write_file(&dir, "b.jsonl", &[late]);

        let events = parse_session_files(&[pa, pb]);
        let _ = std::fs::remove_dir_all(&dir);

        let ghosts: Vec<&Event> = events.iter().filter(|e| e.output_tokens == 0).collect();
        assert!(!ghosts.is_empty());
        assert!(
            ghosts.iter().all(|g| g.model.is_empty()),
            "ghost.model must be cleared to avoid leaking into model_shares"
        );
    }

    #[test]
    fn partial_overlap_preserves_unique_lines_from_non_canonical_stream() {
        // Pathological partial-overlap: the non-canonical stream carries a
        // unique Edit block (lines_suggested != 0), the canonical stream is
        // a terminal snapshot with higher max_total but no content blocks
        // recorded. The Edit's line metrics must survive via the ghost
        // chunk; otherwise we'd silently under-count code activity.
        let dir = tmp_dir("partial_overlap_lines");
        // a.jsonl: 2 chunks, the second carries an Edit (3 added, 2 removed
        // → lines_suggested = 5 from `count_diff_lines`).
        let a1 = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.100Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","content":[{"type":"thinking"}],"usage":{"input_tokens":3,"output_tokens":50,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}"#;
        let a2 = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:01.200Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","content":[{"type":"tool_use","name":"Edit","input":{"old_string":"a\nb\nc","new_string":"X\nY\nZ"}}],"usage":{"input_tokens":3,"output_tokens":150,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}"#;
        // b.jsonl: terminal snapshot only, higher max_total, no content blocks.
        let b1 = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:03.500Z","requestId":"r","message":{"id":"m","model":"claude-opus-4-6","usage":{"input_tokens":3,"output_tokens":300,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}"#;
        let pa = write_file(&dir, "a.jsonl", &[a1, a2]);
        let pb = write_file(&dir, "b.jsonl", &[b1]);

        let events = parse_session_files(&[pa, pb]);
        let _ = std::fs::remove_dir_all(&dir);

        // Billing comes from b's snapshot only.
        let sum_out: u64 = events.iter().map(|e| e.output_tokens).sum();
        assert_eq!(sum_out, 300, "billing must equal canonical snapshot");

        // Line metrics from a.jsonl's Edit are preserved via the ghost.
        let total_sugg: u64 = events.iter().map(|e| e.lines_suggested).sum();
        let total_added: u64 = events.iter().map(|e| e.lines_added).sum();
        let total_deleted: u64 = events.iter().map(|e| e.lines_deleted).sum();
        assert!(
            total_sugg > 0,
            "Edit lines_suggested must survive on the ghost; got {total_sugg}"
        );
        assert_eq!(total_added, 3, "all 3 new lines must be counted");
        assert_eq!(total_deleted, 3, "all 3 old lines must be counted");

        // Ghost itself: zero billing, non-zero lines, in a.jsonl.
        let ghost_with_lines: Vec<&Event> = events
            .iter()
            .filter(|e| e.output_tokens == 0 && e.lines_added > 0)
            .collect();
        assert_eq!(ghost_with_lines.len(), 1);
        assert_eq!(ghost_with_lines[0].session_file, "a.jsonl");
        assert_eq!(ghost_with_lines[0].cost_usd, 0.0);
        assert!(ghost_with_lines[0].request_id.is_none());
    }

    #[test]
    fn partial_overlap_preserves_non_canonical_timestamps_as_ghosts() {
        // Realistic partial-overlap scenario: parent file logs progressive
        // streaming chunks (cumulative usage), a sub-agent file logs only a
        // terminal snapshot with higher `max_total`. The canonical-stream
        // picker correctly chooses the sub-agent for billing (= highest
        // final snapshot), but we must NOT lose the earlier minutes of
        // activity the parent captured. They come back as zero-usage ghost
        // events so `active_minutes` and the minute-level timeline stay
        // correct.
        let dir = tmp_dir("partial_overlap");
        let mk = |ts: &str, out: u64| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","requestId":"r","message":{{"id":"m","model":"claude-opus-4-6","usage":{{"input_tokens":3,"output_tokens":{out},"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}}}}"#
            )
        };
        // Parent file: 3 progressive chunks, terminates at out=200.
        let a_lines = [
            mk("2026-04-01T12:00:00.100Z", 50),
            mk("2026-04-01T12:00:01.200Z", 120),
            mk("2026-04-01T12:00:02.300Z", 200),
        ];
        // Sub-agent file: 1 late chunk with the complete snapshot, out=300.
        let b_lines = [mk("2026-04-01T12:00:03.500Z", 300)];
        let pa = write_file(
            &dir,
            "a.jsonl",
            &a_lines.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        let pb = write_file(
            &dir,
            "b.jsonl",
            &b_lines.iter().map(String::as_str).collect::<Vec<_>>(),
        );

        let events = parse_session_files(&[pa, pb]);
        let _ = std::fs::remove_dir_all(&dir);

        // 1 canonical chunk (from b.jsonl) + 3 ghost chunks (from a.jsonl).
        assert_eq!(events.len(), 4, "4 events: 1 canonical + 3 ghosts");

        // Billing invariant: sum must equal the canonical snapshot — ghosts
        // contribute zero. Never the inflated 50+120+200+300 = 670.
        let sum_out: u64 = events.iter().map(|e| e.output_tokens).sum();
        assert_eq!(sum_out, 300, "billing stays at the canonical snapshot");

        // Activity preservation: the 3 parent timestamps survive as ghosts
        // in a.jsonl, and b.jsonl carries the canonical.
        let canonical: Vec<&Event> = events.iter().filter(|e| e.output_tokens > 0).collect();
        assert_eq!(canonical.len(), 1);
        assert_eq!(canonical[0].session_file, "b.jsonl");

        let ghosts: Vec<&Event> = events.iter().filter(|e| e.output_tokens == 0).collect();
        assert_eq!(ghosts.len(), 3);
        assert!(ghosts.iter().all(|g| g.session_file == "a.jsonl"));
        assert!(ghosts.iter().all(|g| g.request_id.is_none()));
    }

    #[test]
    fn independent_subagent_calls_preserved() {
        // A requestId that only exists in an agent file (no parent mirror).
        let dir = tmp_dir("indep_agent");
        let line = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.100Z","requestId":"r_indep","message":{"id":"m","model":"claude-opus-4-6","usage":{"input_tokens":5,"output_tokens":40,"cache_read_input_tokens":200,"cache_creation_input_tokens":0}}}"#;
        let path = write_file(&dir, "agent-solo.jsonl", &[line]);

        let events = parse_session_files(&[path]);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].output_tokens, 40);
        assert_eq!(events[0].cache_read_input_tokens, 200);
    }

    #[test]
    fn deltaized_tokens_sum_to_final_snapshot_across_streams() {
        // Two streams (parent + mirror), each with 3 cumulative chunks.
        // Total billable must equal ONE final snapshot (not two).
        let dir = tmp_dir("sum_invariant");
        let mk = |ts: &str, out: u64| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","requestId":"r","message":{{"id":"m","model":"claude-opus-4-6","usage":{{"input_tokens":3,"output_tokens":{out},"cache_read_input_tokens":500,"cache_creation_input_tokens":0}}}}}}"#
            )
        };
        let p1 = mk("2026-04-01T12:00:00.100Z", 10);
        let p2 = mk("2026-04-01T12:00:00.200Z", 50);
        let p3 = mk("2026-04-01T12:00:00.300Z", 120);
        let parent = write_file(&dir, "parent.jsonl", &[&p1, &p2, &p3]);
        let agent = write_file(&dir, "agent-echo.jsonl", &[&p1, &p2, &p3]);

        let events = parse_session_files(&[parent, agent]);
        let _ = std::fs::remove_dir_all(&dir);

        let sum_in: u64 = events.iter().map(|e| e.input_tokens).sum();
        let sum_out: u64 = events.iter().map(|e| e.output_tokens).sum();
        let sum_cr: u64 = events.iter().map(|e| e.cache_read_input_tokens).sum();
        assert_eq!(sum_in, 3, "sum(input) must equal final input_tokens");
        assert_eq!(sum_out, 120, "sum(output) must equal final output_tokens");
        assert_eq!(sum_cr, 500, "sum(cache_read) must equal final cache_read");
    }

    #[test]
    fn cost_from_tokens_includes_cache_creation_surcharge() {
        // Opus 4-6: input=$5/M, output=$25/M, cache_read=$0.5/M,
        // cache_creation = input × 1.25 = $6.25/M.
        let cost = cost_from_tokens("claude-opus-4-6", 0, 0, 0, 1_000_000);
        assert!(
            (cost - 6.25).abs() < 1e-9,
            "1M cache_creation tokens must cost $6.25 (= $5 × 1.25), got {cost}"
        );

        // Sanity: a full request with all four buckets.
        let cost = cost_from_tokens("claude-opus-4-6", 1000, 200, 500, 300);
        // fresh=500 × $5/M = $0.0025
        // cache_read=500 × $0.5/M = $0.00025
        // cache_creation=300 × $5/M × 1.25 = $0.001875
        // output=200 × $25/M = $0.005
        // sum = $0.009625
        assert!((cost - 0.009625).abs() < 1e-9);
    }

    #[test]
    fn dedups_user_events_mirrored_across_files_by_uuid() {
        // Same user-side patch line mirrored into parent + sub-agent logs
        // (identical `uuid`). Must be counted once for lines_added /
        // lines_accepted, otherwise efficiency metrics inflate.
        let dir = tmp_dir("user_uuid_dedup");
        let patch = r#"{"type":"user","timestamp":"2026-04-01T12:00:00.000Z","uuid":"u-1","toolUseResult":{"structuredPatch":[{"lines":["+a","+b","-c"]}]}}"#;
        let parent = write_file(&dir, "parent.jsonl", &[patch]);
        let agent = write_file(&dir, "agent-sub.jsonl", &[patch]);

        let events = parse_session_files(&[parent, agent]);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            events.len(),
            1,
            "user event must not be duplicated by mirror"
        );
        assert_eq!(events[0].lines_added, 2);
        assert_eq!(events[0].lines_deleted, 1);
        assert_eq!(events[0].lines_accepted, 3);
    }

    #[test]
    fn keeps_user_events_without_uuid_even_when_duplicated() {
        // Edge case: older Claude Code versions may not emit uuid on user
        // events. We must keep them all — under-counting is worse than a
        // rare over-count, and historical data shouldn't silently vanish.
        let dir = tmp_dir("user_no_uuid");
        let patch = r#"{"type":"user","timestamp":"2026-04-01T12:00:00.000Z","toolUseResult":{"structuredPatch":[{"lines":["+a"]}]}}"#;
        let a = write_file(&dir, "a.jsonl", &[patch]);
        let b = write_file(&dir, "b.jsonl", &[patch]);

        let events = parse_session_files(&[a, b]);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.line_uuid.is_none()));
    }

    #[test]
    fn keeps_user_events_without_request_id() {
        use std::io::Write;

        let dir = tmp_dir("user_events");
        let user = r#"{"type":"user","timestamp":"2026-04-01T12:00:00.000Z","toolUseResult":{"structuredPatch":[{"lines":["+a","-b"]}]}}"#;
        let path_a = dir.join("a.jsonl");
        writeln!(std::fs::File::create(&path_a).unwrap(), "{user}").unwrap();
        let path_b = dir.join("b.jsonl");
        writeln!(std::fs::File::create(&path_b).unwrap(), "{user}").unwrap();

        let events = parse_session_files(&[path_a, path_b]);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.request_id.is_none()));
    }

    // ------------------------------------------------------------------
    // message.id fallback when requestId is absent
    // ------------------------------------------------------------------

    #[test]
    fn dedups_assistant_chunks_by_message_id_when_request_id_absent() {
        // Proxied API calls (corporate Bedrock gateways, third-party LLM
        // relays) often strip `requestId` from the JSONL but preserve
        // `message.id`. Modern Claude Code writes the same final usage
        // snapshot on every content-block line — without a fallback dedup
        // key, two chunks would be summed and totals would inflate 2×.
        let dir = tmp_dir("msgid_fallback_same_file");
        let c1 = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.100Z","uuid":"u-1","message":{"id":"msg_abc","model":"claude-opus-4-6","content":[{"type":"thinking"}],"usage":{"input_tokens":3,"output_tokens":200,"cache_read_input_tokens":1000,"cache_creation_input_tokens":0}}}"#;
        let c2 = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.200Z","uuid":"u-2","message":{"id":"msg_abc","model":"claude-opus-4-6","content":[{"type":"text"}],"usage":{"input_tokens":3,"output_tokens":200,"cache_read_input_tokens":1000,"cache_creation_input_tokens":0}}}"#;
        let path = write_file(&dir, "s.jsonl", &[c1, c2]);

        let events = parse_session_files(&[path]);
        let _ = std::fs::remove_dir_all(&dir);

        // Sum must equal ONE snapshot, not two.
        let sum_in: u64 = events.iter().map(|e| e.input_tokens).sum();
        let sum_out: u64 = events.iter().map(|e| e.output_tokens).sum();
        let sum_cr: u64 = events.iter().map(|e| e.cache_read_input_tokens).sum();
        assert_eq!(sum_in, 3, "must not double-count identical input snapshot");
        assert_eq!(sum_out, 200, "must not double-count identical output snapshot");
        assert_eq!(sum_cr, 1000, "must not double-count identical cache_read snapshot");
    }

    #[test]
    fn mirror_dedup_uses_message_id_when_request_id_absent() {
        // Same flow as `deltaized_tokens_sum_to_final_snapshot_across_streams`
        // but stripped of `requestId` — this is the Bedrock-proxy case.
        // Three cumulative chunks per file × two mirror files. Totals must
        // still equal ONE final snapshot.
        let dir = tmp_dir("msgid_fallback_mirror");
        let mk = |ts: &str, out: u64, uid: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","uuid":"{uid}","message":{{"id":"msg_xyz","model":"claude-opus-4-6","usage":{{"input_tokens":3,"output_tokens":{out},"cache_read_input_tokens":500,"cache_creation_input_tokens":0}}}}}}"#
            )
        };
        let p1 = mk("2026-04-01T12:00:00.100Z", 10, "u1");
        let p2 = mk("2026-04-01T12:00:00.200Z", 50, "u2");
        let p3 = mk("2026-04-01T12:00:00.300Z", 120, "u3");
        let parent = write_file(&dir, "parent.jsonl", &[&p1, &p2, &p3]);
        let agent = write_file(&dir, "agent-echo.jsonl", &[&p1, &p2, &p3]);

        let events = parse_session_files(&[parent, agent]);
        let _ = std::fs::remove_dir_all(&dir);

        let sum_in: u64 = events.iter().map(|e| e.input_tokens).sum();
        let sum_out: u64 = events.iter().map(|e| e.output_tokens).sum();
        let sum_cr: u64 = events.iter().map(|e| e.cache_read_input_tokens).sum();
        assert_eq!(sum_in, 3, "sum(input) must equal final input_tokens, not 6× chunks");
        assert_eq!(sum_out, 120, "sum(output) must equal final output_tokens");
        assert_eq!(sum_cr, 500, "sum(cache_read) must equal final cache_read");
    }

    #[test]
    fn prefers_request_id_over_message_id_when_both_present() {
        // When both ids are present, requestId still wins. Two chunks share
        // the same requestId but differ on message.id (synthetic edge case;
        // proves the precedence). Should dedup as ONE group.
        let dir = tmp_dir("msgid_precedence");
        let c1 = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.100Z","requestId":"req_same","uuid":"u-1","message":{"id":"msg_a","model":"claude-opus-4-6","usage":{"input_tokens":3,"output_tokens":50,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}"#;
        let c2 = r#"{"type":"assistant","timestamp":"2026-04-01T12:00:00.200Z","requestId":"req_same","uuid":"u-2","message":{"id":"msg_b","model":"claude-opus-4-6","usage":{"input_tokens":3,"output_tokens":120,"cache_read_input_tokens":100,"cache_creation_input_tokens":0}}}"#;
        let path = write_file(&dir, "s.jsonl", &[c1, c2]);

        let events = parse_session_files(&[path]);
        let _ = std::fs::remove_dir_all(&dir);

        // Both chunks share requestId, so deltaize collapses them.
        // Sum of deltaized output = max snapshot = 120, not 50+120=170.
        let sum_out: u64 = events.iter().map(|e| e.output_tokens).sum();
        assert_eq!(sum_out, 120, "requestId grouping wins over message.id");
        assert_eq!(
            events[0].request_id.as_deref(),
            Some("req_same"),
            "request_id field carries the explicit requestId, not message.id"
        );
    }
}
