use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::adapters::util;
use crate::types::UsageRecord;

pub const AGENT: &str = "kimi";

// ---- Old Kimi CLI format (~/.kimi): "StatusUpdate" messages ----

#[derive(Debug, Deserialize)]
struct RawLine {
    /// Epoch seconds (float).
    timestamp: Option<f64>,
    message: Option<RawMessage>,
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: Option<RawPayload>,
}

#[derive(Debug, Deserialize)]
struct RawPayload {
    token_usage: Option<RawTokenUsage>,
    message_id: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawTokenUsage {
    #[serde(default)]
    input_other: u64,
    #[serde(default)]
    output: u64,
    #[serde(default)]
    input_cache_read: u64,
    #[serde(default)]
    input_cache_creation: u64,
}

// ---- New Kimi Code CLI format (~/.kimi-code): "usage.record" events ----

#[derive(Debug, Deserialize)]
struct RawUsageRecord {
    #[serde(rename = "type")]
    kind: Option<String>,
    model: Option<String>,
    usage: Option<RawTokenUsageV2>,
    #[serde(rename = "usageScope")]
    usage_scope: Option<String>,
    /// Epoch milliseconds.
    time: Option<i64>,
}

#[derive(Debug, Deserialize, Default)]
struct RawTokenUsageV2 {
    #[serde(default, rename = "inputOther")]
    input_other: u64,
    #[serde(default)]
    output: u64,
    #[serde(default, rename = "inputCacheRead")]
    input_cache_read: u64,
    #[serde(default, rename = "inputCacheCreation")]
    input_cache_creation: u64,
}

/// Kimi CLI sessions: $KIMI_DATA_DIR or ~/.kimi, usage in
/// sessions/{group}/{session}/wire.jsonl.
/// Kimi Code CLI sessions: ~/.kimi-code, usage in
/// sessions/{workspace}/{session}/agents/{agent}/wire.jsonl.
pub fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let old_base = std::env::var("KIMI_DATA_DIR")
        .map(PathBuf::from)
        .ok()
        .or_else(|| dirs::home_dir().map(|h| h.join(".kimi")));
    if let Some(base) = old_base {
        dirs.push(base.join("sessions"));
    }
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".kimi-code").join("sessions"));
    }
    dirs.sort();
    dirs.dedup();
    dirs.retain(|p| p.is_dir());
    dirs
}

pub fn collect_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in data_dirs() {
        collect_wire(&dir, &mut files);
    }
    files
}

fn collect_wire(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_wire(&path, files);
        } else if path.file_name().is_some_and(|n| n == "wire.jsonl") {
            files.push(path);
        }
    }
}

/// Where a wire.jsonl sits on disk: which session it belongs to and what
/// to show as project. Old layout: {group}/{session}/wire.jsonl; new
/// Kimi Code layout: {workspace}/{session}/agents/{agent}/wire.jsonl.
struct SourceInfo {
    session_id: String,
    /// Sub-agent dir name ("main", "agent-0", ...); empty for old layout.
    agent_name: String,
    project: String,
}

fn source_info(path: &Path) -> SourceInfo {
    let parent_name = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let agents_dir = path.parent().and_then(|p| p.parent());
    if agents_dir
        .and_then(|p| p.file_name())
        .is_some_and(|n| n == "agents")
    {
        // New Kimi Code layout.
        let session_dir = agents_dir.and_then(|p| p.parent());
        let session_id = session_dir
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let project = session_dir
            .and_then(|p| p.parent())
            .and_then(|w| w.file_name())
            .and_then(|n| n.to_str())
            .map(workspace_project)
            .unwrap_or_else(|| "Kimi Code".to_string());
        return SourceInfo {
            session_id,
            agent_name: parent_name,
            project,
        };
    }
    SourceInfo {
        session_id: parent_name,
        agent_name: String::new(),
        project: "Kimi CLI".to_string(),
    }
}

/// Workspace dir `wd_<name>_<12-hex-hash>` -> `<name>`.
fn workspace_project(dir_name: &str) -> String {
    let name = dir_name.strip_prefix("wd_").unwrap_or(dir_name);
    match name.rsplit_once('_') {
        Some((base, suffix))
            if suffix.len() == 12 && suffix.chars().all(|c| c.is_ascii_hexdigit()) =>
        {
            base.to_string()
        }
        _ => name.to_string(),
    }
}

/// Parse a Kimi wire.jsonl. Both generations are handled per line:
/// old "StatusUpdate" messages carry payload.token_usage; new Kimi Code
/// "usage.record" events carry per-step usage plus the real model name.
pub fn parse_file(path: &Path) -> Vec<UsageRecord> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let src = source_info(path);
    let fallback_ts = util::mtime_ms(path);

    let mut records = Vec::new();
    for line in content.lines() {
        // Cheap pre-filters, same as ccusage.
        if line.contains("\"usage.record\"") {
            if let Some(rec) = parse_usage_record_line(line, &src, fallback_ts) {
                records.push(rec);
            }
        } else if line.contains("\"StatusUpdate\"") && line.contains("\"token_usage\"") {
            if let Some(rec) = parse_status_update_line(line, &src, fallback_ts) {
                records.push(rec);
            }
        }
    }
    records
}

/// New Kimi Code format: one `usage.record` per step. `usageScope` is
/// "turn" for per-step usage; the "session"-scoped record is the
/// cumulative total and must be skipped to avoid double counting.
fn parse_usage_record_line(line: &str, src: &SourceInfo, fallback_ts: i64) -> Option<UsageRecord> {
    let raw: RawUsageRecord = serde_json::from_str(line).ok()?;
    if raw.kind.as_deref() != Some("usage.record") {
        return None;
    }
    if raw.usage_scope.as_deref() != Some("turn") {
        return None;
    }
    let usage = raw.usage?;
    if usage.input_other == 0
        && usage.output == 0
        && usage.input_cache_read == 0
        && usage.input_cache_creation == 0
    {
        return None;
    }
    let ts = raw
        .time
        .filter(|t| *t > 0)
        .map(util::smart_unit_ms)
        .unwrap_or(fallback_ts);
    let dedup_key = format!(
        "kimi:{}:{}:{}:{}:{}:{}:{}",
        src.session_id,
        src.agent_name,
        ts,
        usage.input_other,
        usage.output,
        usage.input_cache_creation,
        usage.input_cache_read
    );
    Some(UsageRecord {
        agent: AGENT.to_string(),
        project: src.project.clone(),
        session_id: src.session_id.clone(),
        timestamp_ms: ts,
        // The wire carries the real model; fall back to the CLI's
        // current default when it is absent.
        model: raw
            .model
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "kimi-code/k3".to_string()),
        input_tokens: usage.input_other,
        output_tokens: usage.output,
        cache_creation_5m: usage.input_cache_creation,
        cache_creation_1h: 0,
        cache_read_tokens: usage.input_cache_read,
        cost_usd: None,
        dedup_key: Some(dedup_key),
    })
}

/// Old Kimi CLI format: "StatusUpdate" messages carry per-step
/// token_usage (ccusage kimi adapter behavior).
fn parse_status_update_line(line: &str, src: &SourceInfo, fallback_ts: i64) -> Option<UsageRecord> {
    let raw: RawLine = serde_json::from_str(line).ok()?;
    let message = raw.message?;
    if message.kind.as_deref() != Some("StatusUpdate") {
        return None;
    }
    let payload = message.payload?;
    let usage = payload.token_usage?;
    if usage.input_other == 0
        && usage.output == 0
        && usage.input_cache_read == 0
        && usage.input_cache_creation == 0
    {
        return None;
    }
    let ts = raw
        .timestamp
        .filter(|s| s.is_finite())
        .map(|s| (s * 1000.0) as i64)
        .unwrap_or(fallback_ts);
    let message_id = payload.message_id.unwrap_or_default();
    let dedup_key = format!(
        "kimi:{}:{}:{}:{}:{}:{}:{}",
        src.session_id,
        message_id,
        ts,
        usage.input_other,
        usage.output,
        usage.input_cache_creation,
        usage.input_cache_read
    );
    Some(UsageRecord {
        agent: AGENT.to_string(),
        project: src.project.clone(),
        session_id: src.session_id.clone(),
        timestamp_ms: ts,
        model: "kimi-for-coding".to_string(),
        input_tokens: usage.input_other,
        output_tokens: usage.output,
        cache_creation_5m: usage.input_cache_creation,
        cache_creation_1h: 0,
        cache_read_tokens: usage.input_cache_read,
        cost_usd: None,
        dedup_key: Some(dedup_key),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wire(dir: &Path, rel: &str, content: &str) -> PathBuf {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tokbar-kimi-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_kimi_code_turn_usage_records() {
        let tmp = temp_dir("v2");
        let wire = write_wire(
            &tmp,
            "wd_myproj_0123456789ab/session_abc/agents/main/wire.jsonl",
            concat!(
                // step.end events also carry usage, but are not usage.record lines.
                "{\"type\":\"context.append_loop_event\",\"event\":{\"type\":\"step.end\",\"usage\":{\"inputOther\":1,\"output\":2,\"inputCacheRead\":3,\"inputCacheCreation\":4}},\"time\":1786118461000}\n",
                "{\"type\":\"usage.record\",\"model\":\"kimi-code/k3\",\"usage\":{\"inputOther\":100,\"output\":20,\"inputCacheRead\":50,\"inputCacheCreation\":5},\"usageScope\":\"turn\",\"time\":1786118461472}\n",
                // session-scoped cumulative record: must be skipped.
                "{\"type\":\"usage.record\",\"model\":\"kimi-code/k3\",\"usage\":{\"inputOther\":999,\"output\":999,\"inputCacheRead\":999,\"inputCacheCreation\":9},\"usageScope\":\"session\",\"time\":1786118462000}\n",
                // all-zero usage: skipped.
                "{\"type\":\"usage.record\",\"model\":\"kimi-code/k3\",\"usage\":{\"inputOther\":0,\"output\":0,\"inputCacheRead\":0,\"inputCacheCreation\":0},\"usageScope\":\"turn\",\"time\":1786118463000}\n",
            ),
        );
        let records = parse_file(&wire);
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.agent, "kimi");
        assert_eq!(r.project, "myproj");
        assert_eq!(r.session_id, "session_abc");
        assert_eq!(r.model, "kimi-code/k3");
        assert_eq!(r.timestamp_ms, 1786118461472);
        assert_eq!(r.input_tokens, 100);
        assert_eq!(r.output_tokens, 20);
        assert_eq!(r.cache_read_tokens, 50);
        assert_eq!(r.cache_creation_5m, 5);
        assert_eq!(r.cache_creation_1h, 0);
        assert!(r.cost_usd.is_none());
        assert!(r.dedup_key.is_some());
    }

    #[test]
    fn subagent_wire_keeps_session_but_gets_distinct_dedup_key() {
        let tmp = temp_dir("v2-sub");
        let line = "{\"type\":\"usage.record\",\"model\":\"kimi-code/k3\",\"usage\":{\"inputOther\":10,\"output\":1,\"inputCacheRead\":0,\"inputCacheCreation\":0},\"usageScope\":\"turn\",\"time\":1786118461472}\n";
        let main_wire = write_wire(
            &tmp,
            "wd_proj_0123456789ab/session_s1/agents/main/wire.jsonl",
            line,
        );
        let sub_wire = write_wire(
            &tmp,
            "wd_proj_0123456789ab/session_s1/agents/agent-0/wire.jsonl",
            line,
        );
        let main = parse_file(&main_wire);
        let sub = parse_file(&sub_wire);
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(main.len(), 1);
        assert_eq!(sub.len(), 1);
        assert_eq!(main[0].session_id, sub[0].session_id);
        assert_ne!(main[0].dedup_key, sub[0].dedup_key);
    }

    #[test]
    fn workspace_project_strips_prefix_and_hash() {
        assert_eq!(workspace_project("wd_tokbar_e3e675a00df6"), "tokbar");
        assert_eq!(workspace_project("wd_.kimi-code_5ba22589248d"), ".kimi-code");
        assert_eq!(
            workspace_project("wd_miss.sleinvoice_0ca0130642ce"),
            "miss.sleinvoice"
        );
        // No 12-hex suffix: keep the name as-is.
        assert_eq!(workspace_project("wd_plain"), "plain");
        assert_eq!(workspace_project("other"), "other");
    }

    #[test]
    fn parses_old_status_update_lines() {
        let tmp = temp_dir("v1");
        let wire = write_wire(
            &tmp,
            "group1/session-old/wire.jsonl",
            concat!(
                "{\"timestamp\":1786118461.472,\"message\":{\"type\":\"StatusUpdate\",\"payload\":{\"token_usage\":{\"input_other\":100,\"output\":20,\"input_cache_read\":50,\"input_cache_creation\":5},\"message_id\":\"msg-1\"}}}\n",
                "{\"timestamp\":1786118462.0,\"message\":{\"type\":\"StatusUpdate\",\"payload\":{\"token_usage\":{\"input_other\":0,\"output\":0,\"input_cache_read\":0,\"input_cache_creation\":0}}}}\n",
            ),
        );
        let records = parse_file(&wire);
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.project, "Kimi CLI");
        assert_eq!(r.session_id, "session-old");
        assert_eq!(r.model, "kimi-for-coding");
        assert_eq!(r.timestamp_ms, 1786118461472);
        assert_eq!(r.input_tokens, 100);
        assert!(r.dedup_key.as_ref().is_some_and(|k| k.contains("msg-1")));
    }
}
