//! Gemini CLI adapter, ported from ccusage's gemini adapter.
//!
//! Data lives in $GEMINI_DATA_DIR (or ~/.gemini/tmp) as .json session
//! files and .jsonl streams. Token fields come under several aliases;
//! cached tokens may or may not be included in the reported totals, so
//! the input/cache split follows ccusage's overlap-subtraction rules.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::util;
use crate::types::UsageRecord;

pub const AGENT: &str = "gemini";

pub fn data_dirs() -> Vec<PathBuf> {
    util::env_dirs("GEMINI_DATA_DIR", ".gemini/tmp")
}

pub fn collect_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in data_dirs() {
        util::collect_with_ext(&dir, &["json", "jsonl"], &mut files);
    }
    files.sort();
    files.dedup();
    files
}

#[derive(Debug, Default, Clone, Copy)]
struct Tokens {
    input: u64,
    output: u64,
    cached: u64,
    thoughts: u64,
    tool: u64,
    total: u64,
}

impl Tokens {
    fn is_zero(&self) -> bool {
        self.input == 0 && self.output == 0 && self.cached == 0 && self.thoughts == 0
            && self.tool == 0 && self.total == 0
    }
}

fn parse_tokens(v: &Value) -> Option<Tokens> {
    let obj = v.as_object()?;
    let pick = |keys: &[&str]| -> u64 {
        keys.iter()
            .filter_map(|k| obj.get(*k))
            .map(util::as_u64)
            .find(|n| *n > 0)
            .unwrap_or(0)
    };
    let t = Tokens {
        input: pick(&["input", "prompt", "input_tokens", "prompt_tokens"]),
        output: pick(&["output", "candidates", "output_tokens", "candidates_tokens"]),
        cached: pick(&["cached", "cached_tokens"]),
        thoughts: pick(&["thoughts", "reasoning", "thoughts_tokens", "reasoning_tokens"]),
        tool: pick(&["tool", "tool_tokens"]),
        total: pick(&["total", "total_tokens"]),
    };
    (!t.is_zero()).then_some(t)
}

/// Split raw Gemini counts into (input without cache, cache read).
///
/// Gemini CLI writes `input` = promptTokenCount, which already includes
/// the cached prompt tokens, and `total` = input + output + thoughts +
/// tool. ccusage `normalize_session_input`: when that inclusive total
/// matches exactly (and the exclusive one, which would add `cached` on
/// top, does not), `cached` overlaps `input` and is subtracted. Aggregate
/// `stats` blocks always overlap (ccusage `subtract_cached_overlap_tokens`).
fn split_cached_input(t: &Tokens, always_subtract_cache: bool) -> (u64, u64) {
    let inclusive_total = t.input + t.output + t.thoughts + t.tool;
    let exclusive_total = inclusive_total + t.cached;
    let direct_overlap =
        t.cached > 0 && t.total == inclusive_total && t.total != exclusive_total;
    if always_subtract_cache || direct_overlap {
        (t.input.saturating_sub(t.cached.min(t.input)), t.cached)
    } else {
        (t.input, t.cached)
    }
}

/// Build a usage record from raw token counts. `always_subtract_cache`
/// matches ccusage: aggregate stats always treat `cached` as overlapping
/// input; direct events only when the totals indicate the overlap.
fn build_record(
    t: Tokens,
    always_subtract_cache: bool,
    model: &str,
    session_id: &str,
    ts: i64,
    dedup_key: Option<String>,
) -> Option<UsageRecord> {
    let (input_without_cache, cache_read) = split_cached_input(&t, always_subtract_cache);
    // Tool tokens count as input; thoughts (reasoning) bill as output.
    let input = input_without_cache + t.tool;
    let mut output = t.output + t.thoughts;
    // ccusage `apply_total_token_fallback`: tokens the reported total
    // covers beyond the known parts are billed as output (this also
    // handles events that only carry a total).
    let known = input + output + cache_read;
    output += t.total.saturating_sub(known);
    if input == 0 && output == 0 && cache_read == 0 {
        return None;
    }
    // Every row needs a stable key: fall back to the event content when
    // the source has no message id.
    let dedup_key = dedup_key.unwrap_or_else(|| {
        format!(
            "gemini:{session_id}:{ts}:{model}:{}:{}:{}:{}:{}:{}",
            t.input, t.output, t.cached, t.thoughts, t.tool, t.total
        )
    });
    Some(UsageRecord {
        agent: AGENT.to_string(),
        project: "Gemini".to_string(),
        session_id: session_id.to_string(),
        timestamp_ms: ts,
        model: model.to_string(),
        input_tokens: input,
        output_tokens: output,
        cache_creation_5m: 0,
        cache_creation_1h: 0,
        cache_read_tokens: cache_read,
        cost_usd: None,
        dedup_key: Some(dedup_key),
    })
}

pub fn parse_file(path: &Path) -> Vec<UsageRecord> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let file_stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let fallback_ts = util::mtime_ms(path);

    if path.extension().is_some_and(|e| e == "jsonl") {
        parse_jsonl(&content, &file_stem, fallback_ts)
    } else {
        let Ok(value) = serde_json::from_str::<Value>(&content) else {
            return Vec::new();
        };
        parse_session_value(&value, &file_stem, fallback_ts)
    }
}

/// Stateful JSONL stream: lines may update the sticky sessionId/model,
/// emit direct `type: "gemini"` events (deduped by id, last one wins),
/// or carry aggregate `stats` objects.
fn parse_jsonl(content: &str, file_stem: &str, fallback_ts: i64) -> Vec<UsageRecord> {
    let mut session: Option<String> = None;
    let mut model: Option<String> = None;
    let mut records: Vec<UsageRecord> = Vec::new();
    let mut by_id: HashMap<String, usize> = HashMap::new();

    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(s) = util::get_str(&value, &["sessionId", "session_id"]) {
            session = Some(s.to_string());
        }
        if let Some(m) = util::get_str(&value, &["model"]) {
            model = Some(m.to_string());
        }
        let sid = session.clone().unwrap_or_else(|| file_stem.to_string());

        if value.get("type").and_then(Value::as_str) == Some("gemini") {
            let Some(tokens) = value.get("tokens").and_then(parse_tokens) else {
                continue;
            };
            let ts = value
                .get("timestamp")
                .or_else(|| value.get("created_at"))
                .and_then(util::ts_from_value)
                .unwrap_or(fallback_ts);
            let m = util::get_str(&value, &["model"])
                .map(str::to_string)
                .or_else(|| model.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let id = util::get_str(&value, &["id"]).map(str::to_string);
            let dedup = id
                .as_ref()
                .map(|i| format!("gemini:{sid}:{i}"));
            let Some(rec) = build_record(tokens, false, &m, &sid, ts, dedup) else {
                continue;
            };
            // Last event per id wins, in the slot of its first occurrence.
            // (`HashMap::insert` here used to overwrite the stored slot
            // with `records.len()` on a duplicate without pushing, so a
            // third copy of an id indexed past the end and panicked.)
            match id {
                Some(i) => match by_id.entry(i) {
                    std::collections::hash_map::Entry::Occupied(slot) => {
                        records[*slot.get()] = rec;
                    }
                    std::collections::hash_map::Entry::Vacant(slot) => {
                        slot.insert(records.len());
                        records.push(rec);
                    }
                },
                None => records.push(rec),
            }
        } else if let Some(stats) = value.get("stats").or_else(|| {
            value.get("result").and_then(|r| r.get("stats"))
        }) {
            let ts = value
                .get("timestamp")
                .and_then(util::ts_from_value)
                .unwrap_or(fallback_ts);
            records.extend(parse_stats(stats, model.as_deref(), &sid, ts));
        }
    }
    records
}

/// Single-file JSON session: `messages` array, a bare direct event, or
/// an aggregate `stats` object, in that order of precedence. Like
/// ccusage `parse_json_file`, a file that has `messages` (or is itself a
/// direct event) returns early: its `stats` block summarizes the same
/// calls and counting it too would double the session.
fn parse_session_value(value: &Value, file_stem: &str, fallback_ts: i64) -> Vec<UsageRecord> {
    let session_id = util::get_str(value, &["sessionId", "session_id"])
        .map(str::to_string)
        .unwrap_or_else(|| file_stem.to_string());
    let session_ts = util::get_str(value, &["startTime", "lastUpdated"])
        .and_then(util::parse_rfc3339_ms)
        .unwrap_or(fallback_ts);

    if let Some(messages) = value.get("messages").and_then(Value::as_array) {
        let mut records = Vec::new();
        for (i, msg) in messages.iter().enumerate() {
            if msg.get("type").and_then(Value::as_str) != Some("gemini") {
                continue;
            }
            let Some(tokens) = msg.get("tokens").and_then(parse_tokens) else {
                continue;
            };
            let ts = msg
                .get("timestamp")
                .or_else(|| msg.get("created_at"))
                .and_then(util::ts_from_value)
                .unwrap_or(session_ts);
            let model = util::get_str(msg, &["model"]).unwrap_or("unknown");
            let dedup = util::get_str(msg, &["id"])
                .map(|id| format!("gemini:{session_id}:{id}"))
                .or_else(|| Some(format!("gemini:{session_id}:msg{i}")));
            records.extend(build_record(tokens, false, model, &session_id, ts, dedup));
        }
        return records;
    }
    if value.get("type").and_then(Value::as_str) == Some("gemini") {
        let Some(tokens) = value.get("tokens").and_then(parse_tokens) else {
            return Vec::new();
        };
        let ts = value
            .get("timestamp")
            .or_else(|| value.get("created_at"))
            .and_then(util::ts_from_value)
            .unwrap_or(session_ts);
        let model = util::get_str(value, &["model"]).unwrap_or("unknown");
        let dedup = util::get_str(value, &["id"]).map(|id| format!("gemini:{session_id}:{id}"));
        return build_record(tokens, false, model, &session_id, ts, dedup)
            .into_iter()
            .collect();
    }
    match value
        .get("stats")
        .or_else(|| value.get("result").and_then(|r| r.get("stats")))
    {
        Some(stats) => {
            let ts = value
                .get("timestamp")
                .and_then(util::ts_from_value)
                .unwrap_or(session_ts);
            let model_hint = util::get_str(value, &["model"]);
            parse_stats(stats, model_hint, &session_id, ts)
        }
        None => Vec::new(),
    }
}

/// Aggregate stats: per-model token blocks under `stats.models`, else a
/// single block attributed to `model_hint`.
fn parse_stats(
    stats: &Value,
    model_hint: Option<&str>,
    session_id: &str,
    ts: i64,
) -> Vec<UsageRecord> {
    let mut records = Vec::new();
    if let Some(models) = stats.get("models").and_then(Value::as_object) {
        for (model, block) in models {
            let tokens = block
                .get("tokens")
                .and_then(parse_tokens)
                .or_else(|| parse_tokens(block));
            if let Some(t) = tokens {
                let dedup = format!("gemini:{session_id}:stats:{model}:{ts}");
                records.extend(build_record(t, true, model, session_id, ts, Some(dedup)));
            }
        }
    } else if let Some(t) = stats.get("tokens").and_then(parse_tokens).or_else(|| parse_tokens(stats)) {
        let model = model_hint.unwrap_or("unknown");
        let dedup = format!("gemini:{session_id}:stats:{model}:{ts}");
        records.extend(build_record(t, true, model, session_id, ts, Some(dedup)));
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tokens(v: Value) -> Tokens {
        parse_tokens(&v).expect("tokens")
    }

    #[test]
    fn subtracts_cached_when_total_includes_it_in_input() {
        // Gemini CLI shape: input = promptTokenCount (incl. cached),
        // total = input + output + thoughts + tool.
        let t = tokens(json!({"input": 100, "output": 20, "cached": 30, "thoughts": 5, "total": 125}));
        let r = build_record(t, false, "gemini-2.5-pro", "s", 1, None).unwrap();
        assert_eq!(r.input_tokens, 70);
        assert_eq!(r.cache_read_tokens, 30);
        assert_eq!(r.output_tokens, 25);
        assert_eq!(r.total_tokens(), 125);
    }

    #[test]
    fn keeps_input_when_cached_is_reported_separately() {
        // Exclusive shape: total counts cached on top of input.
        let t = tokens(json!({"input": 100, "output": 20, "cached": 30, "thoughts": 5, "total": 155}));
        let r = build_record(t, false, "gemini-2.5-pro", "s", 1, None).unwrap();
        assert_eq!(r.input_tokens, 100);
        assert_eq!(r.cache_read_tokens, 30);
        assert_eq!(r.output_tokens, 25);
    }

    #[test]
    fn aggregate_stats_always_subtract_cached() {
        let t = tokens(json!({"prompt": 100, "candidates": 20, "cached": 30}));
        let r = build_record(t, true, "gemini-2.5-pro", "s", 1, Some("k".into())).unwrap();
        assert_eq!(r.input_tokens, 70);
        assert_eq!(r.cache_read_tokens, 30);
    }

    #[test]
    fn total_only_events_bill_total_as_output_and_get_a_key() {
        let t = tokens(json!({"total": 654}));
        let r = build_record(t, false, "gemini-2.5-pro", "s", 7, None).unwrap();
        assert_eq!(r.output_tokens, 654);
        assert_eq!(r.input_tokens, 0);
        assert!(r.dedup_key.is_some());
    }

    #[test]
    fn json_messages_win_over_stats() {
        let v = json!({
            "sessionId": "sess",
            "startTime": "2026-01-01T00:00:00Z",
            "messages": [
                {"type": "user", "id": "u1"},
                {"type": "gemini", "id": "m1", "model": "gemini-2.5-pro",
                 "tokens": {"input": 10, "output": 5, "total": 15}}
            ],
            "stats": {"models": {"gemini-2.5-pro": {"tokens": {"prompt": 10, "candidates": 5}}}}
        });
        let records = parse_session_value(&v, "stem", 0);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].dedup_key.as_deref(), Some("gemini:sess:m1"));
        assert_eq!(records[0].input_tokens, 10);
    }

    #[test]
    fn json_stats_used_when_no_messages() {
        let v = json!({
            "sessionId": "sess",
            "stats": {"models": {"gemini-2.5-pro": {"tokens": {"prompt": 10, "candidates": 5, "cached": 4}}}}
        });
        let records = parse_session_value(&v, "stem", 42);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_tokens, 6);
        assert_eq!(records[0].cache_read_tokens, 4);
    }

    #[test]
    fn jsonl_direct_events_without_id_get_content_keys() {
        let content = [
            r#"{"sessionId":"s1","model":"gemini-2.5-flash"}"#,
            r#"{"type":"gemini","timestamp":"2026-01-01T00:00:00Z","tokens":{"input":10,"output":2,"total":12}}"#,
            r#"{"type":"gemini","timestamp":"2026-01-01T00:01:00Z","tokens":{"input":11,"output":2,"total":13}}"#,
        ]
        .join("\n");
        let records = parse_jsonl(&content, "stem", 0);
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|r| r.dedup_key.is_some()));
        assert_ne!(records[0].dedup_key, records[1].dedup_key);
        assert_eq!(records[0].model, "gemini-2.5-flash");
    }

    #[test]
    fn repeated_event_ids_keep_the_last_copy_without_panicking() {
        let ev = |input: u64| {
            format!(
                r#"{{"type":"gemini","id":"m1","timestamp":"2026-01-01T00:00:00Z","model":"g","tokens":{{"input":{input},"output":1}}}}"#
            )
        };
        let content = [ev(1), ev(2), ev(3), ev(4)].join("\n");
        let records = parse_jsonl(&content, "stem", 0);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_tokens, 4);
    }
}
