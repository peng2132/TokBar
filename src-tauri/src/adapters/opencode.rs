//! OpenCode adapter, ported from ccusage's opencode adapter.
//!
//! Data lives in $OPENCODE_DATA_DIR (or ~/.local/share/opencode): a SQLite
//! `opencode.db` holding the legacy `message` table and/or the v2
//! `session_message` table, plus pre-SQLite per-message JSON files under
//! `storage/message/`. All carry the same usage shape.
//!
//! Dedup is content-based, not by message id: `Session.fork` copies every
//! message into the new session with a fresh id but identical tokens,
//! cost and `time.created`, so an id key would count a forked history
//! twice. The same content key also collapses a message present in both
//! the legacy and v2 tables, or in both the DB and a JSON file.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::util;
use crate::types::UsageRecord;

pub const AGENT: &str = "opencode";

pub fn data_dirs() -> Vec<PathBuf> {
    util::env_dirs("OPENCODE_DATA_DIR", ".local/share/opencode")
}

/// ccusage `is_channel_db_name`: `opencode-<channel>.db`.
fn is_channel_db_name(name: &str) -> bool {
    name.strip_prefix("opencode-")
        .and_then(|rest| rest.strip_suffix(".db"))
        .is_some_and(|ch| ch.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
}

pub fn collect_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for base in data_dirs() {
        // Prefer opencode.db; fall back to the first opencode-*.db.
        let main_db = base.join("opencode.db");
        if main_db.is_file() {
            files.push(main_db);
        } else if let Ok(read) = std::fs::read_dir(&base) {
            let mut candidates: Vec<PathBuf> = read
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(is_channel_db_name)
                })
                .collect();
            candidates.sort();
            files.extend(candidates.into_iter().next());
        }
        let messages = base.join("storage").join("message");
        if messages.is_dir() {
            util::collect_with_ext(&messages, &["json"], &mut files);
        }
    }
    files
}

pub fn parse_file(path: &Path) -> Vec<UsageRecord> {
    if path.extension().is_some_and(|e| e == "db") {
        parse_db(path)
    } else {
        parse_json(path)
    }
}

/// Content identity shared by OpenCode-family adapters (OpenCode, Kilo):
/// everything a forked copy keeps verbatim, nothing it regenerates
/// (message id, session id, parent id).
#[allow(clippy::too_many_arguments)]
pub(super) fn content_key(
    prefix: &str,
    model: &str,
    created_ms: i64,
    input: u64,
    output: u64,
    reasoning: u64,
    cache_read: u64,
    cache_write: u64,
    cost: Option<f64>,
) -> String {
    format!(
        "{prefix}:{model}:{created_ms}:{input}:{output}:{reasoning}:{cache_read}:{cache_write}:{}",
        util::cost_key(cost)
    )
}

/// Push unless an identical message (same content key) was already
/// emitted from this file, keeping the first (the original, since forks
/// are written after their parent).
fn push_unique(records: &mut Vec<UsageRecord>, seen: &mut HashSet<String>, rec: UsageRecord) {
    if rec.dedup_key.as_ref().is_some_and(|k| seen.insert(k.clone())) {
        records.push(rec);
    }
}

fn parse_db(path: &Path) -> Vec<UsageRecord> {
    let Some(conn) = util::open_readonly_db(path) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    let mut seen = HashSet::new();
    // Message ids already counted from the legacy table: the v2 migration
    // keeps the id, so a v2 row with a known id is the same message even
    // if its payload was rewritten (ccusage `seen_message_ids`).
    let mut legacy_ids: HashSet<String> = HashSet::new();

    if util::table_exists(&conn, "message") {
        if let Ok(mut stmt) = conn.prepare("SELECT id, session_id, data FROM message") {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(0).unwrap_or(None),
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            }) {
                for (id, session, data) in rows.flatten() {
                    let Some(value) = data.and_then(|d| serde_json::from_str::<Value>(&d).ok())
                    else {
                        continue;
                    };
                    if let Some(rec) = record_from_value(&value, session.as_deref()) {
                        legacy_ids.extend(id);
                        push_unique(&mut records, &mut seen, rec);
                    }
                }
            }
        }
    }

    if util::table_exists(&conn, "session_message") {
        for (id, rec) in parse_session_messages(&conn) {
            if !legacy_ids.contains(&id) {
                push_unique(&mut records, &mut seen, rec);
            }
        }
    }
    records
}

/// OpenCode v2 `session_message` rows (ccusage `load_entries_from_database`
/// v2 branch). `data` holds the assistant payload with the model under
/// `model.{id|modelID, providerID}`; `time_created` and `seq` are optional
/// columns depending on the schema version.
fn parse_session_messages(conn: &rusqlite::Connection) -> Vec<(String, UsageRecord)> {
    let columns = util::table_columns(conn, "session_message");
    let has = |c: &str| columns.iter().any(|x| x == c);
    if !["id", "session_id", "type", "data"].iter().all(|c| has(c)) {
        return Vec::new();
    }
    let sql = format!(
        "SELECT id, session_id, type, data, {}, {} FROM session_message",
        if has("time_created") { "time_created" } else { "NULL" },
        if has("seq") { "seq" } else { "NULL" },
    );
    let fork_cutoffs = fork_copy_cutoffs(conn);
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |row| {
        let created = row
            .get::<_, Option<i64>>(4)
            .or_else(|_| row.get::<_, Option<f64>>(4).map(|f| f.map(|f| f as i64)))
            .unwrap_or(None);
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            created,
            row.get::<_, Option<i64>>(5).unwrap_or(None),
        ))
    }) else {
        return Vec::new();
    };

    let mut records = Vec::new();
    for (id, session, kind, data, created, seq) in rows.flatten() {
        if kind.as_deref() != Some("assistant") {
            continue;
        }
        let (Some(id), Some(session)) = (id, session) else {
            continue;
        };
        // Copied parent history of a fork: the content key would usually
        // collapse it too, but the copy's `time_created` column may be the
        // fork time, so drop it by position like ccusage does.
        if let (Some(cutoff), Some(seq)) = (fork_cutoffs.get(&session), seq) {
            if seq <= *cutoff {
                continue;
            }
        }
        let Some(mut value) = data.and_then(|d| serde_json::from_str::<Value>(&d).ok()) else {
            continue;
        };
        // ccusage `OpenCodeV2Message::into_legacy_message`.
        if let Some(obj) = value.as_object_mut() {
            let model_ref = obj.get("model").filter(|m| m.is_object()).cloned();
            if let Some(m) = &model_ref {
                if let Some(model_id) = util::get_str(m, &["id", "modelID"]) {
                    obj.insert("modelID".into(), Value::String(model_id.to_string()));
                }
                if let Some(provider) = util::get_str(m, &["providerID"]) {
                    obj.insert("providerID".into(), Value::String(provider.to_string()));
                }
            }
            let has_created = obj
                .get("time")
                .and_then(|t| t.get("created"))
                .and_then(Value::as_i64)
                .is_some_and(|n| n > 0);
            if let (false, Some(created)) = (has_created, created.filter(|n| *n > 0)) {
                obj.insert("time".into(), serde_json::json!({ "created": created }));
            }
        }
        if let Some(rec) = record_from_value(&value, Some(&session)) {
            records.push((id, rec));
        }
    }
    records
}

/// Largest copied `seq` per fork session (ccusage `fork_copy_cutoffs`):
/// `session_v2` links a fork to its parent and boundary message; rows of
/// the fork at or below the cutoff are the copied parent history.
fn fork_copy_cutoffs(conn: &rusqlite::Connection) -> HashMap<String, i64> {
    let mut cutoffs = HashMap::new();
    if !util::table_exists(conn, "session_v2") {
        return cutoffs;
    }
    let v2_cols = util::table_columns(conn, "session_v2");
    let msg_cols = util::table_columns(conn, "session_message");
    let has = |cols: &[String], names: &[&str]| names.iter().all(|n| cols.iter().any(|c| c == n));
    if !has(&v2_cols, &["id", "fork_session_id", "fork_boundary"])
        || !has(&msg_cols, &["id", "session_id", "seq"])
    {
        return cutoffs;
    }
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, fork_session_id, fork_boundary FROM session_v2
         WHERE fork_session_id IS NOT NULL AND fork_boundary IS NOT NULL",
    ) else {
        return cutoffs;
    };
    let Ok(rows) = stmt.query_map([], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    }) else {
        return cutoffs;
    };
    for (fork_id, parent_id, boundary) in rows.flatten() {
        let (Some(fork_id), Some(parent_id), Some(boundary)) = (fork_id, parent_id, boundary)
        else {
            continue;
        };
        if fork_id.is_empty() || parent_id.is_empty() {
            continue;
        }
        if let Some(cutoff) = fork_copy_cutoff(conn, &parent_id, &boundary) {
            cutoffs.insert(fork_id, cutoff);
        }
    }
    cutoffs
}

/// The parent's boundary `seq` for a `through` fork, or the parent's last
/// `seq` below it for a `before` fork; `None` (keep every row) when the
/// boundary cannot be resolved.
fn fork_copy_cutoff(conn: &rusqlite::Connection, parent_id: &str, boundary: &str) -> Option<i64> {
    let boundary: Value = serde_json::from_str(boundary).ok()?;
    let kind = boundary.get("type")?.as_str()?;
    let message_id = boundary.get("messageID")?.as_str()?;
    let boundary_seq: i64 = conn
        .query_row(
            "SELECT seq FROM session_message WHERE session_id = ?1 AND id = ?2 LIMIT 1",
            [parent_id, message_id],
            |row| row.get(0),
        )
        .ok()?;
    match kind {
        "through" => Some(boundary_seq),
        // `seq` is sparse, so `boundary_seq - 1` could swallow fork rows.
        "before" => conn
            .query_row(
                "SELECT MAX(seq) FROM session_message WHERE session_id = ?1 AND seq < ?2",
                rusqlite::params![parent_id, boundary_seq],
                |row| row.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten(),
        _ => None,
    }
}

fn parse_json(path: &Path) -> Vec<UsageRecord> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };
    record_from_value(&value, None).into_iter().collect()
}

fn record_from_value(value: &Value, session_hint: Option<&str>) -> Option<UsageRecord> {
    let tokens = value.get("tokens")?;
    let model = normalize_model(util::get_str(value, &["modelID"])?);
    let input = util::get_u64(tokens, "input");
    let raw_output = util::get_u64(tokens, "output");
    let reasoning = util::get_u64(tokens, "reasoning");
    // OpenCode reports reasoning separately; ccusage bills it as output.
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

    let timestamp_ms = value
        .get("time")
        .and_then(|t| t.get("created"))
        .and_then(Value::as_i64)
        .filter(|n| *n > 0)
        .map(util::smart_unit_ms)?;
    let session_id = session_hint
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| util::get_str(value, &["sessionID"]).map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string());
    let cost = value
        .get("cost")
        .and_then(Value::as_f64)
        .filter(|c| c.is_finite() && *c > 0.0);
    let dedup_key = content_key(
        "opencode",
        &model,
        timestamp_ms,
        input,
        raw_output,
        reasoning,
        cache_read,
        cache_write,
        cost,
    );

    Some(UsageRecord {
        agent: AGENT.to_string(),
        project: "OpenCode".to_string(),
        session_id,
        timestamp_ms,
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

/// ccusage's opencode model normalization: alias fix-ups plus
/// `claude-{family}-{major}.{minor}` -> `claude-{family}-{major}-{minor}`.
fn normalize_model(model: &str) -> String {
    match model {
        "gemini-3-pro-high" => return "gemini-3-pro-preview".to_string(),
        "k2p6" => return "kimi-k2.6".to_string(),
        _ => {}
    }
    if let Some(rest) = model.strip_prefix("claude-") {
        let bytes = rest.as_bytes();
        if bytes.len() >= 3 {
            let n = bytes.len();
            if bytes[n - 2] == b'.'
                && bytes[n - 1].is_ascii_digit()
                && bytes[n - 3].is_ascii_digit()
            {
                // The three bytes checked above are ASCII, so these
                // offsets are char boundaries.
                let mut fixed = model.to_string();
                fixed.replace_range(model.len() - 2..model.len() - 1, "-");
                return fixed;
            }
        }
    }
    model.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assistant(input: u64, cost: f64, created: i64) -> Value {
        json!({
            "role": "assistant",
            "modelID": "claude-sonnet-4.5",
            "providerID": "anthropic",
            "time": {"created": created},
            "tokens": {"input": input, "output": 10, "reasoning": 2, "cache": {"read": 5, "write": 1}},
            "cost": cost
        })
    }

    #[test]
    fn forked_copy_gets_the_same_key() {
        // Session.fork: same payload, fresh message + session ids.
        let mut original = assistant(100, 0.01, 1_767_312_000_000);
        original["id"] = json!("msg_parent");
        original["sessionID"] = json!("ses_parent");
        let mut copy = original.clone();
        copy["id"] = json!("msg_fork");
        copy["sessionID"] = json!("ses_fork");
        copy["parentID"] = json!("msg_fork_user");

        let a = record_from_value(&original, None).unwrap();
        let b = record_from_value(&copy, None).unwrap();
        assert_eq!(a.dedup_key, b.dedup_key);
        assert_ne!(a.session_id, b.session_id);
        assert_eq!(a.model, "claude-sonnet-4-5");
        assert_eq!(a.output_tokens, 12);

        // A genuinely different turn does not collide.
        let other = record_from_value(&assistant(101, 0.01, 1_767_312_000_000), None).unwrap();
        assert_ne!(a.dedup_key, other.dedup_key);
    }

    fn create_db(path: &Path) -> rusqlite::Connection {
        let db = rusqlite::Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, data TEXT);
             CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT,
                 seq INTEGER NOT NULL DEFAULT 0, time_created INTEGER, data TEXT);
             CREATE TABLE session_v2 (id TEXT PRIMARY KEY, fork_session_id TEXT, fork_boundary TEXT);",
        )
        .unwrap();
        db
    }

    fn v2_payload(input: u64) -> String {
        json!({
            "model": {"id": "gpt-5", "providerID": "openai"},
            "tokens": {"input": input, "output": 1, "cache": {"read": 0, "write": 0}},
            "cost": 0.5
        })
        .to_string()
    }

    #[test]
    fn reads_legacy_and_v2_tables_and_dedups_forks() {
        let root = util::test_dir("opencode-db");
        let path = root.join("opencode.db");
        let db = create_db(&path);
        // Legacy table: one original message plus its fork copy.
        let original = assistant(100, 0.01, 1_767_312_000_000).to_string();
        db.execute(
            "INSERT INTO message VALUES ('m1', 'ses_a', ?1), ('m1-fork', 'ses_fork_legacy', ?1)",
            [&original],
        )
        .unwrap();
        // v2: parent with three turns; a `through` fork at the 2nd copies
        // seq 0..=1 (new ids, fork-time time_created) and adds its own.
        for (id, seq, input) in [("p0", 0, 10), ("p1", 1, 20), ("p2", 2, 30)] {
            db.execute(
                "INSERT INTO session_message (id, session_id, type, seq, time_created, data)
                 VALUES (?1, 'ses_parent', 'assistant', ?2, ?3, ?4)",
                rusqlite::params![id, seq, 1_767_312_100_000_i64 + seq, v2_payload(input)],
            )
            .unwrap();
        }
        for (id, seq, input) in [("f0", 0, 10), ("f1", 1, 20), ("f-own", 2, 99)] {
            db.execute(
                "INSERT INTO session_message (id, session_id, type, seq, time_created, data)
                 VALUES (?1, 'ses_fork', 'assistant', ?2, ?3, ?4)",
                rusqlite::params![id, seq, 1_767_399_999_000_i64 + seq, v2_payload(input)],
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, data)
             VALUES ('u1', 'ses_parent', 'user', 3, 1, '{}')",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO session_v2 VALUES ('ses_fork', 'ses_parent', ?1)",
            [json!({"type": "through", "messageID": "p1"}).to_string()],
        )
        .unwrap();
        drop(db);

        let records = parse_file(&path);
        let inputs: Vec<u64> = records.iter().map(|r| r.input_tokens).collect();
        assert_eq!(inputs, vec![100, 10, 20, 30, 99]);
        assert!(records.iter().all(|r| r.dedup_key.is_some()));
        let v2 = records.iter().find(|r| r.input_tokens == 10).unwrap();
        assert_eq!(v2.model, "gpt-5");
        assert_eq!(v2.session_id, "ses_parent");
        assert_eq!(v2.timestamp_ms, 1_767_312_100_000);
        assert_eq!(v2.cost_usd, Some(0.5));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deduplicates_legacy_and_v2_rows_by_message_id() {
        // ccusage test of the same name: the v2 copy of a migrated message
        // keeps its id, even when its payload differs.
        let root = util::test_dir("opencode-ids");
        let path = root.join("opencode.db");
        let db = create_db(&path);
        db.execute(
            "INSERT INTO message VALUES ('msg-shared', 'legacy-session', ?1)",
            [r#"{"providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":120,"output":60},"cost":0.03}"#],
        )
        .unwrap();
        db.execute(
            "INSERT INTO session_message (id, session_id, type, time_created, data)
             VALUES ('msg-shared', 'v2-session', 'assistant', 1767312000000, ?1)",
            [r#"{"model":{"id":"gpt-test","providerID":"openai"},"time":{"created":1767312000000},"tokens":{"input":999,"output":999},"cost":9.99}"#],
        )
        .unwrap();
        drop(db);
        let records = parse_file(&path);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "legacy-session");
        assert_eq!(records[0].input_tokens, 120);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn channel_db_names() {
        assert!(is_channel_db_name("opencode-beta.db"));
        assert!(is_channel_db_name("opencode-dev_2.db"));
        assert!(!is_channel_db_name("opencode-../x.db"));
        assert!(!is_channel_db_name("opencode.db"));
    }
}
