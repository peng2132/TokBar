//! Amp adapter, ported from ccusage's amp adapter.
//!
//! Threads are JSON files under $AMP_DATA_DIR (or ~/.local/share/amp)
//! `threads/`. Newer threads carry a `usageLedger.events` array (cache
//! tokens joined from `messages` via `toMessageId`); older ones only
//! have assistant `messages[].usage` blocks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::util;
use crate::types::UsageRecord;

pub const AGENT: &str = "amp";

pub fn data_dirs() -> Vec<PathBuf> {
    util::env_dirs("AMP_DATA_DIR", ".local/share/amp")
        .into_iter()
        .map(|b| b.join("threads"))
        .filter(|p| p.is_dir())
        .collect()
}

pub fn collect_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in data_dirs() {
        util::collect_with_ext(&dir, &["json"], &mut files);
    }
    files.sort();
    files
}

pub fn parse_file(path: &Path) -> Vec<UsageRecord> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };
    let thread_id = util::get_str(&value, &["id"])
        .map(str::to_string)
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        });

    if let Some(events) = value
        .get("usageLedger")
        .and_then(|l| l.get("events"))
        .and_then(Value::as_array)
    {
        parse_ledger(&value, events, &thread_id)
    } else {
        parse_messages(&value, &thread_id)
    }
}

/// Cache tokens live on assistant messages; the ledger references them
/// through `toMessageId`.
fn cache_by_message(value: &Value) -> HashMap<i64, (u64, u64)> {
    let mut map = HashMap::new();
    let Some(messages) = value.get("messages").and_then(Value::as_array) else {
        return map;
    };
    for msg in messages {
        if msg.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(id) = msg.get("messageId").and_then(Value::as_i64) else {
            continue;
        };
        let Some(usage) = msg.get("usage") else { continue };
        map.insert(
            id,
            (
                util::get_u64(usage, "cacheCreationInputTokens"),
                util::get_u64(usage, "cacheReadInputTokens"),
            ),
        );
    }
    map
}

fn parse_ledger(value: &Value, events: &[Value], thread_id: &str) -> Vec<UsageRecord> {
    let cache_map = cache_by_message(value);
    let mut records = Vec::new();
    for event in events {
        let Some(ts) = util::get_str(event, &["timestamp"]).and_then(util::parse_rfc3339_ms) else {
            continue;
        };
        let Some(model) = util::get_str(event, &["model"]) else {
            continue;
        };
        let tokens = event.get("tokens").cloned().unwrap_or(Value::Null);
        let input = util::get_u64(&tokens, "input");
        let mut output = util::get_u64(&tokens, "output");
        let (cache_creation, cache_read) = event
            .get("toMessageId")
            .and_then(Value::as_i64)
            .and_then(|id| cache_map.get(&id).copied())
            .unwrap_or((0, 0));
        if input == 0 && output == 0 && cache_creation == 0 && cache_read == 0 {
            let total = util::get_u64(&tokens, "total");
            if total == 0 {
                continue;
            }
            output = total;
        }
        records.push(record(
            thread_id,
            source_id(event, "id"),
            ts,
            model,
            input,
            output,
            cache_creation,
            cache_read,
        ));
    }
    records
}

fn parse_messages(value: &Value, thread_id: &str) -> Vec<UsageRecord> {
    let Some(messages) = value.get("messages").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    for msg in messages {
        if msg.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = msg.get("usage") else { continue };
        let ts_str = util::get_str(usage, &["timestamp"])
            .or_else(|| util::get_str(msg, &["timestamp"]));
        let Some(ts) = ts_str.and_then(util::parse_rfc3339_ms) else {
            continue;
        };
        let Some(model) =
            util::get_str(usage, &["model"]).or_else(|| util::get_str(msg, &["model"]))
        else {
            continue;
        };
        let input = util::get_u64(usage, "inputTokens");
        let mut output = util::get_u64(usage, "outputTokens");
        let cache_creation = util::get_u64(usage, "cacheCreationInputTokens");
        let cache_read = util::get_u64(usage, "cacheReadInputTokens");
        if input == 0 && output == 0 && cache_creation == 0 && cache_read == 0 {
            let total = util::get_u64(usage, "totalTokens");
            if total == 0 {
                continue;
            }
            output = total;
        }
        records.push(record(
            thread_id,
            source_id(msg, "messageId"),
            ts,
            model,
            input,
            output,
            cache_creation,
            cache_read,
        ));
    }
    records
}

/// Ledger event `id` or message `messageId` (number or string).
fn source_id(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn record(
    thread_id: &str,
    source_id: Option<String>,
    ts: i64,
    model: &str,
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
) -> UsageRecord {
    // Every row needs a stable key now that keyless rows are no longer
    // collapsed: the thread id comes from the file content, plus the
    // event/message id when present, plus the usage itself. A thread
    // file that is copied or renamed therefore maps onto the same rows.
    let dedup_key = format!(
        "amp:{thread_id}:{}:{ts}:{model}:{input}:{output}:{cache_creation}:{cache_read}",
        source_id.as_deref().unwrap_or("")
    );
    UsageRecord {
        agent: AGENT.to_string(),
        project: "Amp".to_string(),
        session_id: thread_id.to_string(),
        timestamp_ms: ts,
        model: model.to_string(),
        input_tokens: input,
        output_tokens: output,
        cache_creation_5m: cache_creation,
        cache_creation_1h: 0,
        cache_read_tokens: cache_read,
        cost_usd: None,
        dedup_key: Some(dedup_key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ledger_and_message_records_get_stable_keys() {
        let ledger = json!({
            "id": "T-thread-a",
            "messages": [{"role": "assistant", "messageId": 7,
                          "usage": {"cacheCreationInputTokens": 3, "cacheReadInputTokens": 4}}],
            "usageLedger": {"events": [
                {"id": "event-a", "timestamp": "2026-01-02T00:00:00.000Z", "model": "gpt-5",
                 "tokens": {"input": 10, "output": 5}, "toMessageId": 7},
                {"id": "event-b", "timestamp": "2026-01-02T00:00:00.000Z", "model": "gpt-5",
                 "tokens": {"input": 10, "output": 5}, "toMessageId": 7}
            ]}
        });
        let events = ledger["usageLedger"]["events"].as_array().unwrap();
        let records = parse_ledger(&ledger, events, "T-thread-a");
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|r| r.dedup_key.is_some()));
        // Same content, different ledger ids: both calls are kept.
        assert_ne!(records[0].dedup_key, records[1].dedup_key);
        assert_eq!((records[0].cache_creation_5m, records[0].cache_read_tokens), (3, 4));

        let legacy = json!({"messages": [{"role": "assistant", "messageId": 1,
            "usage": {"model": "claude-sonnet-4", "timestamp": "2026-01-02T00:00:00Z",
                      "inputTokens": 1, "outputTokens": 2}}]});
        let a = parse_messages(&legacy, "t1");
        let b = parse_messages(&legacy, "t1");
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].dedup_key, b[0].dedup_key);
    }
}
