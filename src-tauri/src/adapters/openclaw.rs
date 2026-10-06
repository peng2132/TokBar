//! OpenClaw adapter, ported from ccusage's openclaw adapter.
//!
//! Two sources under each state dir ($OPENCLAW_DIR, or the legacy
//! ~/.openclaw / ~/.clawdbot / ~/.moltbot / ~/.moldbot homes):
//! - session transcripts: JSONL files (plus archived `.jsonl.deleted.*` /
//!   `.jsonl.reset.*` copies) anywhere below the root;
//! - per-agent SQLite stores `agents/<agentId>/agent/openclaw-agent.sqlite`
//!   whose `transcript_events.event_json` rows hold the same records.
//!
//! `model_change` / `model-snapshot` records update the active model;
//! assistant `message` records carry usage and an optional cost under
//! `message.usage.cost.total`.
//!
//! After `openclaw doctor --fix` migrates legacy JSONL into SQLite both
//! copies exist on disk. Both paths build the same content key (session,
//! timestamp, model, tokens; no cost, which SQLite may have corrected),
//! so the copies collapse onto one row. SQLite stores are collected first
//! so their provider-billed row is the one kept (ccusage prefers it too).
//!
//! Deviation from ccusage: model names are stored without the
//! "[openclaw] " display prefix so LiteLLM pricing matching keeps
//! working; the agent column already attributes the usage.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::util;
use crate::types::UsageRecord;

pub const AGENT: &str = "openclaw";

const AGENT_DB_FILE: &str = "openclaw-agent.sqlite";

pub fn data_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let custom = std::env::var("OPENCLAW_DIR")
        .ok()
        .filter(|raw| !raw.trim().is_empty());
    if let Some(raw) = custom {
        dirs.extend(util::split_env_paths(&raw));
    } else if let Some(home) = dirs::home_dir() {
        for legacy in [".openclaw", ".clawdbot", ".moltbot", ".moldbot"] {
            dirs.push(home.join(legacy));
        }
    }
    dirs.retain(|p| p.is_dir());
    dirs.sort();
    dirs.dedup();
    dirs
}

pub fn collect_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in data_dirs() {
        files.extend(agent_databases(&dir));
        let mut sessions = Vec::new();
        util::walk_files(
            &dir,
            &|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(is_session_file_name)
            },
            &mut sessions,
        );
        sessions.sort();
        files.extend(sessions);
    }
    files
}

/// ccusage `is_openclaw_session_file`.
fn is_session_file_name(name: &str) -> bool {
    let Some(index) = name.find(".jsonl") else {
        return false;
    };
    let suffix = &name[index..];
    suffix == ".jsonl" || suffix.starts_with(".jsonl.deleted.") || suffix.starts_with(".jsonl.reset.")
}

fn is_dir_no_symlink(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_dir())
}

/// `agents/*/agent/openclaw-agent.sqlite`, lexical traversal without
/// following symlinks (ccusage `collect_agent_databases`).
fn agent_databases(root: &Path) -> Vec<PathBuf> {
    let agents = root.join("agents");
    if !is_dir_no_symlink(&agents) {
        return Vec::new();
    }
    let Ok(read) = std::fs::read_dir(&agents) else {
        return Vec::new();
    };
    let mut dbs: Vec<PathBuf> = read
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path().join("agent"))
        .filter(|p| is_dir_no_symlink(p))
        .map(|p| p.join(AGENT_DB_FILE))
        .filter(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_file()))
        .collect();
    dbs.sort();
    dbs
}

pub fn parse_file(path: &Path) -> Vec<UsageRecord> {
    if path.file_name().is_some_and(|n| n == AGENT_DB_FILE) {
        parse_agent_db(path)
    } else {
        parse_jsonl(path)
    }
}

/// Model carried by a `model_change` / `model-snapshot` record, or `None`
/// for any other record. ccusage `model_change_source`: when the record
/// has a `data` object the model is read from it, else from the root.
fn model_change(value: &Value) -> Option<Option<String>> {
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    let is_change = kind == "model_change"
        || (kind == "custom"
            && value.get("customType").and_then(Value::as_str) == Some("model-snapshot"));
    if !is_change {
        return None;
    }
    let source = value.get("data").filter(|d| d.is_object()).unwrap_or(value);
    Some(util::get_str(source, &["modelId", "model"]).map(str::to_string))
}

/// One assistant usage record.
fn record_from_event(
    value: &Value,
    session_id: &str,
    current_model: Option<&str>,
    fallback_ts: i64,
) -> Option<UsageRecord> {
    if value.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    let message = value.get("message")?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let usage = message.get("usage")?;
    let input = util::get_u64(usage, "input");
    let mut output = util::get_u64(usage, "output");
    let cache_read = util::get_u64(usage, "cacheRead");
    let cache_write = util::get_u64(usage, "cacheWrite");
    if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
        let total = util::get_u64(usage, "totalTokens");
        if total == 0 {
            return None;
        }
        output = total;
    }
    let ts = message
        .get("timestamp")
        .or_else(|| value.get("timestamp"))
        .and_then(util::ts_from_value)
        .unwrap_or(fallback_ts);
    let model = util::get_str(message, &["modelId", "model"])
        .or(current_model)
        .unwrap_or("unknown")
        .to_string();
    // OpenClaw (like pi) writes `message.usage.cost.total`; older
    // records kept it at `message.cost.total`.
    let cost = usage
        .get("cost")
        .or_else(|| message.get("cost"))
        .and_then(|c| c.get("total"))
        .and_then(Value::as_f64)
        .filter(|c| c.is_finite() && *c > 0.0);
    let dedup_key = format!(
        "openclaw:{session_id}:{ts}:{model}:{input}:{output}:{cache_write}:{cache_read}"
    );
    Some(UsageRecord {
        agent: AGENT.to_string(),
        project: "OpenClaw".to_string(),
        session_id: session_id.to_string(),
        timestamp_ms: ts,
        model,
        input_tokens: input,
        output_tokens: output,
        cache_creation_5m: cache_write,
        cache_creation_1h: 0,
        cache_read_tokens: cache_read,
        cost_usd: cost,
        dedup_key: Some(dedup_key),
    })
}

/// ccusage `extract_session_id`: the file name up to the first ".jsonl".
fn session_from_path(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    match name.find(".jsonl") {
        Some(0) | None => name,
        Some(i) => name[..i].to_string(),
    }
}

fn parse_jsonl(path: &Path) -> Vec<UsageRecord> {
    let session_id = session_from_path(path);
    let fallback_ts = util::mtime_ms(path);
    let mut current_model: Option<String> = None;
    let mut records = Vec::new();
    util::for_each_line(path, |line| {
        if !util::contains_bytes(line, b"model_change")
            && !util::contains_bytes(line, b"model-snapshot")
            && !util::contains_bytes(line, b"\"usage\"")
        {
            return;
        }
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        if let Some(change) = model_change(&value) {
            if let Some(m) = change {
                current_model = Some(m);
            }
            return;
        }
        records.extend(record_from_event(
            &value,
            &session_id,
            current_model.as_deref(),
            fallback_ts,
        ));
    });
    records
}

/// Per-agent SQLite store (ccusage `read_agent_database`). Rows arrive in
/// (session, seq) order; the tracked model resets on session boundaries
/// exactly like the per-file state of the JSONL path.
fn parse_agent_db(path: &Path) -> Vec<UsageRecord> {
    let Some(conn) = util::open_readonly_db(path) else {
        return Vec::new();
    };
    if !util::table_exists(&conn, "transcript_events") {
        return Vec::new();
    }
    let Ok(mut stmt) = conn.prepare(
        "SELECT session_id, event_json, created_at FROM transcript_events
         ORDER BY session_id ASC, seq ASC",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| {
        let created = row
            .get::<_, Option<i64>>(2)
            .or_else(|_| row.get::<_, Option<f64>>(2).map(|f| f.map(|f| f.trunc() as i64)))
            .unwrap_or(None);
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            created,
        ))
    }) else {
        return Vec::new();
    };

    let mut current_session: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut records = Vec::new();
    for (session, event_json, created) in rows.flatten() {
        let (Some(session), Some(event_json)) = (session, event_json) else {
            continue;
        };
        if current_session.as_deref() != Some(session.as_str()) {
            current_session = Some(session.clone());
            current_model = None;
        }
        // A single malformed row must not drop the session around it.
        let Ok(value) = serde_json::from_str::<Value>(&event_json) else {
            continue;
        };
        if let Some(change) = model_change(&value) {
            if let Some(m) = change {
                current_model = Some(m);
            }
            continue;
        }
        let fallback_ts = created.filter(|c| *c > 0).unwrap_or(0);
        records.extend(record_from_event(
            &value,
            &session,
            current_model.as_deref(),
            fallback_ts,
        ));
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_cost_from_usage_and_legacy_message_paths() {
        let current = json!({"type": "message", "message": {
            "role": "assistant", "timestamp": 1_769_753_935_279_i64,
            "usage": {"input": 1660, "output": 55, "cacheRead": 108_928, "cost": {"total": 0.02}}
        }});
        let r = record_from_event(&current, "abc", Some("gpt-5.2"), 0).unwrap();
        assert_eq!(r.cost_usd, Some(0.02));
        assert_eq!(r.model, "gpt-5.2");
        assert_eq!(r.cache_read_tokens, 108_928);
        assert_eq!(r.timestamp_ms, 1_769_753_935_279);

        let legacy = json!({"type": "message", "message": {
            "role": "assistant", "model": "gpt-5.2", "timestamp": 1_769_753_935_279_i64,
            "usage": {"input": 1, "output": 1}, "cost": {"total": 0.5}
        }});
        assert_eq!(record_from_event(&legacy, "abc", None, 0).unwrap().cost_usd, Some(0.5));
    }

    #[test]
    fn model_change_reads_data_block_first() {
        assert_eq!(
            model_change(&json!({"type": "model_change", "provider": "openai", "modelId": "gpt-5.2"})),
            Some(Some("gpt-5.2".to_string()))
        );
        assert_eq!(
            model_change(&json!({"type": "custom", "customType": "model-snapshot",
                                  "data": {"modelId": "deepseek-v4"}})),
            Some(Some("deepseek-v4".to_string()))
        );
        assert_eq!(model_change(&json!({"type": "message"})), None);
    }

    #[test]
    fn session_file_names() {
        assert!(is_session_file_name("a.jsonl"));
        assert!(is_session_file_name("a.jsonl.deleted.1700000000000"));
        assert!(is_session_file_name("a.jsonl.reset.2026-03-20T06-34-44.520Z"));
        assert!(!is_session_file_name("a.json"));
        assert!(!is_session_file_name("a.jsonl.tmp"));
    }

    #[test]
    fn sqlite_store_matches_migrated_jsonl_copy() {
        let root = util::test_dir("openclaw");
        let db_path = root.join("agents/main/agent").join(AGENT_DB_FILE);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute_batch(
            "CREATE TABLE transcript_events (session_id TEXT NOT NULL, seq INTEGER NOT NULL,
             event_json TEXT NOT NULL, created_at INTEGER NOT NULL, PRIMARY KEY (session_id, seq))",
        )
        .unwrap();
        let events = [
            ("session", 0, r#"{"id":"e1","type":"model_change","modelId":"gpt-5.2","provider":"openai"}"#),
            ("session", 1, r#"{"id":"e2","type":"message","message":{"role":"assistant","usage":{"input":10,"output":20,"cost":{"total":0.51}},"timestamp":1769753935279}}"#),
            ("session", 2, "not json"),
            ("other", 0, r#"{"id":"e3","type":"message","message":{"role":"assistant","usage":{"input":1,"output":2}}}"#),
        ];
        for (session, seq, event) in events {
            db.execute(
                "INSERT INTO transcript_events VALUES (?1, ?2, ?3, 1769753935000)",
                rusqlite::params![session, seq, event],
            )
            .unwrap();
        }
        drop(db);
        let jsonl = root.join("agents/main/sessions/session.jsonl");
        std::fs::create_dir_all(jsonl.parent().unwrap()).unwrap();
        std::fs::write(
            &jsonl,
            [
                r#"{"type":"model_change","modelId":"gpt-5.2"}"#,
                r#"{"type":"message","message":{"role":"assistant","usage":{"input":10,"output":20,"cost":{"total":0.50}},"timestamp":1769753935279}}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        let from_db = parse_file(&db_path);
        assert_eq!(from_db.len(), 2);
        // `other` session: model state reset, created_at as fallback ts.
        assert_eq!(from_db[0].session_id, "other");
        assert_eq!(from_db[0].model, "unknown");
        assert_eq!(from_db[0].timestamp_ms, 1_769_753_935_000);
        assert_eq!(from_db[1].model, "gpt-5.2");
        assert_eq!(from_db[1].cost_usd, Some(0.51));

        let from_jsonl = parse_file(&jsonl);
        assert_eq!(from_jsonl.len(), 1);
        assert_eq!(from_jsonl[0].dedup_key, from_db[1].dedup_key);

        assert_eq!(agent_databases(&root), vec![db_path]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
