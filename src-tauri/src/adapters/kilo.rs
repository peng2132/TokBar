//! Kilo adapter, ported from ccusage's kilo adapter.
//!
//! Kilo stores messages in a SQLite `kilo.db` under $KILO_DATA_DIR (or
//! ~/.local/share/kilo) with the same `message(id, session_id, data)`
//! shape as OpenCode; `data` JSON carries tokens, model and cost.
//!
//! Kilo is an OpenCode fork and inherits `Session.fork`, which copies
//! messages with fresh ids but identical content, so rows are deduped by
//! content (see `opencode::content_key`) rather than by message id.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::{opencode, util};
use crate::types::UsageRecord;

pub const AGENT: &str = "kilo";

pub fn data_dirs() -> Vec<PathBuf> {
    util::env_dirs("KILO_DATA_DIR", ".local/share/kilo")
}

pub fn collect_files() -> Vec<PathBuf> {
    data_dirs()
        .into_iter()
        .map(|d| d.join("kilo.db"))
        .filter(|p| p.is_file())
        .collect()
}

pub fn parse_file(path: &Path) -> Vec<UsageRecord> {
    let Some(conn) = util::open_readonly_db(path) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT session_id, data FROM message") else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
        ))
    }) else {
        return Vec::new();
    };

    let mut records = Vec::new();
    let mut seen = HashSet::new();
    for (row_session, data) in rows.flatten() {
        let Some(value) = data.and_then(|d| serde_json::from_str::<Value>(&d).ok()) else {
            continue;
        };
        let Some(rec) = record_from_value(&value, row_session.as_deref()) else {
            continue;
        };
        // Keep the first copy (the original precedes its fork copies).
        if rec.dedup_key.as_ref().is_some_and(|k| seen.insert(k.clone())) {
            records.push(rec);
        }
    }
    records
}

fn record_from_value(value: &Value, row_session: Option<&str>) -> Option<UsageRecord> {
    if value.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let tokens = value.get("tokens")?;
    let model = util::get_str(value, &["modelID"])?;
    let input = util::get_u64(tokens, "input");
    let raw_output = util::get_u64(tokens, "output");
    let reasoning = util::get_u64(tokens, "reasoning");
    // Reasoning bills as output (ccusage behavior).
    let mut output = raw_output + reasoning;
    let cache = tokens.get("cache");
    let cache_read = cache.map(|c| util::get_u64(c, "read")).unwrap_or(0);
    let cache_write = cache.map(|c| util::get_u64(c, "write")).unwrap_or(0);
    if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
        let total = util::get_u64(tokens, "total");
        if total == 0 {
            return None;
        }
        output = total;
    }
    let ts = value
        .get("time")
        .and_then(|t| t.get("created"))
        .and_then(Value::as_i64)
        .filter(|n| *n > 0)
        .map(util::smart_unit_ms)?;
    let session_id = row_session
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| util::get_str(value, &["session_id", "sessionID"]).map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string());
    let cost = value
        .get("cost")
        .and_then(Value::as_f64)
        .filter(|c| c.is_finite() && *c > 0.0);
    let dedup_key = opencode::content_key(
        "kilo", model, ts, input, raw_output, reasoning, cache_read, cache_write, cost,
    );
    Some(UsageRecord {
        agent: AGENT.to_string(),
        project: "Kilo".to_string(),
        session_id,
        timestamp_ms: ts,
        model: model.to_string(),
        input_tokens: input,
        output_tokens: output,
        cache_creation_5m: cache_write,
        cache_creation_1h: 0,
        cache_read_tokens: cache_read,
        cost_usd: cost,
        dedup_key: Some(dedup_key),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn forked_messages_share_a_key_and_are_counted_once() {
        let root = util::test_dir("kilo");
        let path = root.join("kilo.db");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE message (id TEXT, session_id TEXT, data TEXT)")
            .unwrap();
        let payload = |id: &str, session: &str, input: u64| {
            json!({
                "id": id, "sessionID": session, "role": "assistant",
                "providerID": "anthropic", "modelID": "claude-sonnet-4-20250514",
                "time": {"created": 1_767_312_000_000_i64},
                "tokens": {"input": input, "output": 50, "reasoning": 5, "cache": {"read": 10, "write": 20}},
                "cost": 0.02
            })
            .to_string()
        };
        for (row, session, data) in [
            ("r1", "ses_a", payload("msg-1", "ses_a", 100)),
            ("r2", "ses_fork", payload("msg-1-copy", "ses_fork", 100)),
            ("r3", "ses_a", payload("msg-2", "ses_a", 101)),
            ("r4", "ses_a", json!({"role": "user", "modelID": "x"}).to_string()),
        ] {
            db.execute(
                "INSERT INTO message (id, session_id, data) VALUES (?1, ?2, ?3)",
                [row, session, data.as_str()],
            )
            .unwrap();
        }
        drop(db);

        let records = parse_file(&path);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].session_id, "ses_a");
        assert_eq!(records[0].output_tokens, 55);
        assert_eq!(records[0].cost_usd, Some(0.02));
        assert_ne!(records[0].dedup_key, records[1].dedup_key);
        let _ = std::fs::remove_dir_all(&root);
    }
}
