//! Kimi CLI / Kimi Code adapter, ported from ccusage's kimi adapter.
//!
//! Sessions live under $KIMI_DATA_DIR (or ~/.kimi and ~/.kimi-code) in
//! `sessions/`, one `wire.jsonl` per session (old layout
//! `sessions/<group>/<session>/wire.jsonl`) or per agent (Kimi Code layout
//! `sessions/<workspace>/<session>/agents/<agent>/wire.jsonl`).
//!
//! Two usage shapes:
//! - old: `message.type == "StatusUpdate"` with per-step
//!   `payload.token_usage`;
//! - Kimi Code: `type == "usage.record"` lines; only `usageScope: "turn"`
//!   counts, `"session"` records are cumulative totals.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::adapters::util;
use crate::types::UsageRecord;

pub const AGENT: &str = "kimi";

const DEFAULT_MODEL: &str = "kimi-for-coding";

/// `<root>/sessions` for every Kimi data root.
pub fn data_dirs() -> Vec<PathBuf> {
    let custom = std::env::var("KIMI_DATA_DIR")
        .ok()
        .filter(|raw| !raw.trim().is_empty());
    let roots: Vec<PathBuf> = match custom {
        Some(raw) => util::split_env_paths(&raw),
        None => dirs::home_dir()
            .map(|h| vec![h.join(".kimi"), h.join(".kimi-code")])
            .unwrap_or_default(),
    };
    let mut dirs: Vec<PathBuf> = roots
        .into_iter()
        .map(|b| b.join("sessions"))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

pub fn collect_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for sessions in data_dirs() {
        util::walk_files(&sessions, &|p| is_wire_file(&sessions, p), &mut files);
    }
    files.sort();
    files.dedup();
    files
}

/// ccusage `is_kimi_wire_file`: `wire.jsonl` exactly 3 (old layout) or 5
/// (agents layout) components below `sessions/`.
fn is_wire_file(sessions: &Path, path: &Path) -> bool {
    if path.file_name().is_none_or(|n| n != "wire.jsonl") {
        return false;
    }
    let Ok(rel) = path.strip_prefix(sessions) else {
        return false;
    };
    matches!(
        rel.components()
            .filter(|c| matches!(c, Component::Normal(_)))
            .count(),
        3 | 5
    )
}

/// Session directory name for both layouts (ccusage `extract_session_id`).
fn session_from_path(path: &Path) -> String {
    let parent = path.parent();
    let in_agents = parent
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .is_some_and(|n| n == "agents");
    let session_dir = if in_agents {
        parent.and_then(Path::parent).and_then(Path::parent)
    } else {
        parent
    };
    session_dir
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

struct Tokens {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
}

impl Tokens {
    fn is_zero(&self) -> bool {
        self.input == 0 && self.output == 0 && self.cache_read == 0 && self.cache_creation == 0
    }
}

/// Kimi Code `usage.record` turn line -> (ts, model, tokens).
fn usage_record(value: &Value, fallback_ts: i64) -> Option<(i64, String, Tokens)> {
    if value.get("usageScope").and_then(Value::as_str) != Some("turn") {
        return None;
    }
    let usage = value.get("usage")?;
    let tokens = Tokens {
        input: util::get_u64(usage, "inputOther"),
        output: util::get_u64(usage, "output"),
        cache_read: util::get_u64(usage, "inputCacheRead"),
        cache_creation: util::get_u64(usage, "inputCacheCreation"),
    };
    if tokens.is_zero() {
        return None;
    }
    let ts = value
        .get("time")
        .and_then(Value::as_i64)
        .filter(|t| *t > 0)
        .unwrap_or(fallback_ts);
    let model = util::get_str(value, &["model"]).unwrap_or(DEFAULT_MODEL);
    let model = model.strip_prefix("kimi-code/").unwrap_or(model).to_string();
    Some((ts, model, tokens))
}

/// Old `StatusUpdate` line -> (ts, message id, tokens).
fn status_update(value: &Value, fallback_ts: i64) -> Option<(i64, String, Tokens)> {
    let message = value.get("message")?;
    if message.get("type").and_then(Value::as_str) != Some("StatusUpdate") {
        return None;
    }
    let payload = message.get("payload")?;
    let usage = payload.get("token_usage")?;
    let tokens = Tokens {
        input: util::get_u64(usage, "input_other"),
        output: util::get_u64(usage, "output"),
        cache_read: util::get_u64(usage, "input_cache_read"),
        cache_creation: util::get_u64(usage, "input_cache_creation"),
    };
    if tokens.is_zero() {
        return None;
    }
    // Epoch seconds as a float; the saturating cast cannot panic.
    let ts = value
        .get("timestamp")
        .and_then(Value::as_f64)
        .filter(|s| s.is_finite() && *s > 0.0)
        .map(|s| (s * 1000.0).trunc() as i64)
        .unwrap_or(fallback_ts);
    let message_id = util::get_str(payload, &["message_id"]).unwrap_or("").to_string();
    Some((ts, message_id, tokens))
}

fn record(session_id: &str, ts: i64, model: &str, t: &Tokens, dedup_key: String) -> UsageRecord {
    UsageRecord {
        agent: AGENT.to_string(),
        project: "Kimi CLI".to_string(),
        session_id: session_id.to_string(),
        timestamp_ms: ts,
        model: model.to_string(),
        input_tokens: t.input,
        output_tokens: t.output,
        cache_creation_5m: t.cache_creation,
        cache_creation_1h: 0,
        cache_read_tokens: t.cache_read,
        cost_usd: None,
        dedup_key: Some(dedup_key),
    }
}

/// Parse a Kimi wire.jsonl.
///
/// A file that carries Kimi Code `usage.record` turn lines is accounted
/// from those alone: they are the per-turn rollup of the same calls a
/// `StatusUpdate` reports per step, so counting both shapes would bill a
/// turn twice. Files without turn records keep the old `StatusUpdate`
/// accounting (ccusage behavior).
pub fn parse_file(path: &Path) -> Vec<UsageRecord> {
    let session_id = session_from_path(path);
    let fallback_ts = util::mtime_ms(path);
    let mut turn_records = Vec::new();
    let mut status_records = Vec::new();

    util::for_each_line(path, |line| {
        // Cheap pre-filter, same as ccusage.
        let is_new = util::contains_bytes(line, b"usage.record");
        if !is_new && !util::contains_bytes(line, b"\"token_usage\"") {
            return;
        }
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("usage.record") => {
                if let Some((ts, model, t)) = usage_record(&value, fallback_ts) {
                    let key = format!(
                        "kimi:{session_id}:turn:{ts}:{model}:{}:{}:{}:{}",
                        t.input, t.output, t.cache_creation, t.cache_read
                    );
                    turn_records.push(record(&session_id, ts, &model, &t, key));
                }
            }
            Some("metadata") => {}
            _ => {
                if let Some((ts, message_id, t)) = status_update(&value, fallback_ts) {
                    let key = format!(
                        "kimi:{session_id}:{message_id}:{ts}:{}:{}:{}:{}",
                        t.input, t.output, t.cache_creation, t.cache_read
                    );
                    status_records.push(record(&session_id, ts, DEFAULT_MODEL, &t, key));
                }
            }
        }
    });

    if turn_records.is_empty() {
        status_records
    } else {
        turn_records
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = r#"{"timestamp":1770983427.123,"message":{"type":"StatusUpdate","payload":{"token_usage":{"input_other":100,"output":50,"input_cache_read":10,"input_cache_creation":20},"message_id":"msg-1"}}}"#;
    const TURN: &str = r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":3064,"output":76,"inputCacheRead":14848,"inputCacheCreation":0},"usageScope":"turn","time":1782113184943}"#;
    const SESSION_TOTAL: &str = r#"{"type":"usage.record","model":"kimi-code/kimi-for-coding","usage":{"inputOther":5000,"output":200,"inputCacheRead":20000,"inputCacheCreation":100},"usageScope":"session","time":1782113185000}"#;

    fn write(root: &Path, rel: &str, lines: &[&str]) -> PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    #[test]
    fn old_status_update_format() {
        let root = util::test_dir("kimi-old");
        let path = write(
            &root,
            "sessions/group/session-a/wire.jsonl",
            &[r#"{"type":"metadata","protocol_version":"1.3"}"#, "not json", STATUS],
        );
        let records = parse_file(&path);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "session-a");
        assert_eq!(records[0].timestamp_ms, 1_770_983_427_123);
        assert_eq!((records[0].input_tokens, records[0].output_tokens), (100, 50));
        assert_eq!((records[0].cache_read_tokens, records[0].cache_creation_5m), (10, 20));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn kimi_code_usage_record_format_counts_turns_only() {
        let root = util::test_dir("kimi-new");
        let path = write(
            &root,
            "sessions/workspace/session-b/agents/agent-1/wire.jsonl",
            &[TURN, SESSION_TOTAL],
        );
        let records = parse_file(&path);
        assert_eq!(records.len(), 1, "session-scoped record must be skipped");
        assert_eq!(records[0].session_id, "session-b");
        assert_eq!(records[0].model, "kimi-for-coding");
        assert_eq!(records[0].timestamp_ms, 1_782_113_184_943);
        assert_eq!((records[0].input_tokens, records[0].output_tokens), (3064, 76));
        assert_eq!(records[0].cache_read_tokens, 14848);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn both_shapes_in_one_file_are_not_double_counted() {
        let root = util::test_dir("kimi-both");
        let path = write(
            &root,
            "sessions/workspace/session-c/agents/agent-1/wire.jsonl",
            &[STATUS, TURN],
        );
        let records = parse_file(&path);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_tokens, 3064);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn wire_file_layouts() {
        let s = Path::new("/k/sessions");
        assert!(is_wire_file(s, Path::new("/k/sessions/g/s/wire.jsonl")));
        assert!(is_wire_file(s, Path::new("/k/sessions/w/s/agents/a/wire.jsonl")));
        assert!(!is_wire_file(s, Path::new("/k/sessions/a/b/c/wire.jsonl")));
        assert!(!is_wire_file(s, Path::new("/k/sessions/g/s/other.jsonl")));
    }
}
