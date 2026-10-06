use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::adapters::claude::{collect_jsonl, parse_timestamp_ms};
use crate::adapters::{jsonl, AppendParse};
use crate::types::UsageRecord;

pub const AGENT: &str = "codex";

#[derive(Debug, Deserialize)]
struct RawLine {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: Option<RawPayload>,
}

#[derive(Debug, Deserialize)]
struct RawPayload {
    #[serde(rename = "type")]
    kind: Option<String>,
    info: Option<RawInfo>,
    model: Option<String>,
    model_name: Option<String>,
    /// Working directory, on `session_meta` and `turn_context` lines.
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawInfo {
    model: Option<String>,
    last_token_usage: Option<RawTokenUsage>,
    /// Cumulative session totals; older Codex versions only write this,
    /// so per-turn usage is derived as the delta from the previous event.
    total_token_usage: Option<RawTokenUsage>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, Default, PartialEq, Eq)]
struct RawTokenUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    reasoning_output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

/// Codex usage dirs: `sessions/` plus `archived_sessions/` (ccusage
/// scans both) under every Codex home.
///
/// Homes come from $CODEX_HOME (comma-separated, ccusage parity) when
/// set. Otherwise ~/.codex plus any sibling `~/.codex{-_.}*` home that
/// has a `sessions/` dir — multi-account setups launch the second
/// Codex with CODEX_HOME pointing at such a clone, and a GUI app never
/// sees that per-shell variable.
pub fn data_dirs() -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Ok(raw) = std::env::var("CODEX_HOME") {
        for part in raw.split(',') {
            let part = part.trim();
            if !part.is_empty() {
                bases.push(PathBuf::from(part));
            }
        }
    } else if let Some(home) = dirs::home_dir() {
        bases.push(home.join(".codex"));
        if let Ok(read) = std::fs::read_dir(&home) {
            for entry in read.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let is_clone = name
                    .strip_prefix(".codex")
                    .is_some_and(|rest| rest.starts_with(['-', '_', '.']));
                if is_clone && path.join("sessions").is_dir() {
                    bases.push(path);
                }
            }
        }
    }
    bases.sort();
    bases.dedup();
    bases
        .iter()
        .flat_map(|base| [base.join("sessions"), base.join("archived_sessions")])
        .filter(|p| p.is_dir())
        .collect()
}

pub fn collect_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in data_dirs() {
        collect_jsonl(&dir, &mut files);
    }
    files
}

/// Codex does not record the service tier per event; ccusage detects it
/// from `service_tier = "fast" | "priority"` in the Codex config.toml and
/// bills usage at the fast multiplier when set. Each Codex home has its
/// own config, so the home a log file lives under decides; the config is
/// read at parse time, so toggling it applies to usage scanned from then
/// on.
fn fast_tier_for(log: &Path) -> bool {
    let home = log
        .ancestors()
        .find(|a| {
            a.file_name()
                .is_some_and(|n| n == "sessions" || n == "archived_sessions")
        })
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .or_else(|| {
            std::env::var("CODEX_HOME")
                .ok()
                .and_then(|v| v.split(',').next().map(|s| PathBuf::from(s.trim())))
        })
        .or_else(|| dirs::home_dir().map(|h| h.join(".codex")));
    let Some(config) = home.map(|b| b.join("config.toml")) else {
        return false;
    };
    let Ok(content) = std::fs::read_to_string(config) else {
        return false;
    };
    config_sets_fast_tier(&content)
}

fn config_sets_fast_tier(content: &str) -> bool {
    content
        .lines()
        // Only top-level keys: a `service_tier` under a [profiles.x] table
        // applies to that profile, not to every session.
        .take_while(|line| !line.trim_start().starts_with('['))
        .any(|line| {
            let setting = line.split('#').next().unwrap_or_default().trim();
            let Some((key, value)) = setting.split_once('=') else {
                return false;
            };
            key.trim() == "service_tier"
                && matches!(value.trim().trim_matches(['"', '\'']), "fast" | "priority")
        })
}

/// Per-file parse state carried across incremental reads.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ParseState {
    /// Session's current model (token_count events usually omit it).
    model: Option<String>,
    /// Last cumulative totals seen.
    previous_totals: Option<RawTokenUsage>,
    /// Project name from the session's working directory.
    project: Option<String>,
}

/// Parse a Codex session JSONL: "event_msg" lines whose payload is a
/// "token_count" event carrying last_token_usage (per-turn deltas).
/// Identical events across session files are deduplicated by content key,
/// matching ccusage's codex adapter.
pub fn parse_file(path: &Path) -> Vec<UsageRecord> {
    parse_append(path, 0, None).records
}

/// Incremental parse from byte `offset` (see [`AppendParse`]).
pub fn parse_append(path: &Path, offset: u64, state: Option<&str>) -> AppendParse {
    let mut st: ParseState = state
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let session_id = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let fast = fast_tier_for(path);

    let mut records = Vec::new();
    // Only these lines carry usage, the model, or the working directory;
    // everything else (tool output, images) is skipped unparsed.
    let needles = [
        "\"token_count\"",
        "\"turn_context\"",
        "\"session_meta\"",
        "\"model\"",
        "\"model_name\"",
    ];
    let end = jsonl::scan_lines(path, offset, &needles, |line, complete| {
        let mut scratch;
        let st = if complete {
            &mut st
        } else {
            scratch = st.clone();
            &mut scratch
        };
        if let Some(rec) = parse_line(line, st, &session_id, fast) {
            records.push(rec);
        }
    });
    AppendParse {
        records,
        offset: end.unwrap_or(offset),
        state: serde_json::to_string(&st).ok(),
    }
}

fn parse_line(line: &str, st: &mut ParseState, session_id: &str, fast: bool) -> Option<UsageRecord> {
    let raw = serde_json::from_str::<RawLine>(line).ok()?;
    let payload = raw.payload?;
    if let Some(m) = payload.model.as_ref().or(payload.model_name.as_ref()) {
        if !m.is_empty() {
            st.model = Some(m.clone());
        }
    }
    if st.project.is_none() {
        st.project = payload.cwd.as_deref().and_then(jsonl::project_from_cwd);
    }
    if raw.kind.as_deref() != Some("event_msg") || payload.kind.as_deref() != Some("token_count") {
        return None;
    }
    let info = payload.info?;
    if let Some(m) = info.model.filter(|m| !m.is_empty()) {
        st.model = Some(m);
    }
    let totals = info.total_token_usage;
    // Codex re-emits token_count with unchanged cumulative totals (e.g.
    // on rate-limit updates), repeating last_token_usage; no new tokens
    // were used, so counting it would double the turn.
    if totals.is_some() && totals == st.previous_totals {
        return None;
    }
    let usage = info
        .last_token_usage
        .or_else(|| totals.map(|t| subtract_usage(&t, st.previous_totals.as_ref())));
    if totals.is_some() {
        st.previous_totals = totals;
    }
    let usage = usage?;
    if usage.input_tokens == 0
        && usage.cached_input_tokens == 0
        && usage.output_tokens == 0
        && usage.reasoning_output_tokens == 0
    {
        return None;
    }
    let ts = raw.timestamp.as_deref().and_then(parse_timestamp_ms)?;
    let mut model = st.model.clone().unwrap_or_else(|| "gpt-5".to_string());
    if fast {
        model.push_str("-fast");
    }
    // Codex reports input inclusive of cached reads; the billable
    // fresh input is input - cached (ccusage non_cached_input_tokens).
    let cached = usage.cached_input_tokens.min(usage.input_tokens);
    let fresh_input = usage.input_tokens - cached;
    // Content-based dedup key: identical events in different session
    // files (e.g. resumed sessions) count once.
    let dedup_key = format!(
        "codex:{}:{}:{}:{}:{}:{}",
        ts, model, usage.input_tokens, cached, usage.output_tokens, usage.reasoning_output_tokens
    );
    Some(UsageRecord {
        agent: AGENT.to_string(),
        project: st.project.clone().unwrap_or_else(|| "Codex CLI".to_string()),
        session_id: session_id.to_string(),
        timestamp_ms: ts,
        model,
        input_tokens: fresh_input,
        output_tokens: usage.output_tokens,
        cache_creation_5m: 0,
        cache_creation_1h: 0,
        cache_read_tokens: cached,
        cost_usd: None,
        dedup_key: Some(dedup_key),
    })
}

/// Per-turn usage as the delta between cumulative totals (ccusage
/// subtract_codex_raw_usage).
fn subtract_usage(total: &RawTokenUsage, previous: Option<&RawTokenUsage>) -> RawTokenUsage {
    let Some(prev) = previous else { return *total };
    RawTokenUsage {
        input_tokens: total.input_tokens.saturating_sub(prev.input_tokens),
        cached_input_tokens: total
            .cached_input_tokens
            .saturating_sub(prev.cached_input_tokens),
        output_tokens: total.output_tokens.saturating_sub(prev.output_tokens),
        reasoning_output_tokens: total
            .reasoning_output_tokens
            .saturating_sub(prev.reasoning_output_tokens),
        total_tokens: total.total_tokens.saturating_sub(prev.total_tokens),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_line(ts: &str, last: (u64, u64, u64), total: (u64, u64, u64)) -> String {
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{}}},"total_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{}}}}}}}}}"#,
            last.0, last.1, last.2, total.0, total.1, total.2
        )
    }

    #[test]
    fn repeated_token_count_with_same_totals_counts_once() {
        let mut st = ParseState::default();
        let meta = r#"{"type":"session_meta","payload":{"cwd":"/Users/a/work/chaogu"}}"#;
        let ctx = r#"{"type":"turn_context","payload":{"model":"gpt-6.1-sol","cwd":"/Users/a/work/chaogu"}}"#;
        assert!(parse_line(meta, &mut st, "s", false).is_none());
        assert!(parse_line(ctx, &mut st, "s", false).is_none());
        let a = token_line("2026-09-30T10:00:00Z", (100, 40, 5), (100, 40, 5));
        let dup = token_line("2026-09-30T10:00:01Z", (100, 40, 5), (100, 40, 5));
        let b = token_line("2026-09-30T10:01:00Z", (50, 10, 2), (150, 50, 7));
        let first = parse_line(&a, &mut st, "s", true).unwrap();
        assert_eq!(first.model, "gpt-6.1-sol-fast");
        assert_eq!(first.project, "chaogu");
        assert_eq!((first.input_tokens, first.cache_read_tokens), (60, 40));
        assert!(parse_line(&dup, &mut st, "s", true).is_none());
        let second = parse_line(&b, &mut st, "s", true).unwrap();
        assert_eq!((second.input_tokens, second.output_tokens), (40, 2));
    }

    #[test]
    fn fast_tier_only_from_top_level_config() {
        assert!(config_sets_fast_tier("model = \"x\"\nservice_tier = \"priority\"\n"));
        assert!(config_sets_fast_tier("service_tier = 'fast' # comment"));
        assert!(!config_sets_fast_tier("[profiles.quick]\nservice_tier = \"fast\"\n"));
        assert!(!config_sets_fast_tier("service_tier = \"flex\""));
    }
}
