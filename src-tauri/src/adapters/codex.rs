use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::adapters::claude::{collect_jsonl, parse_timestamp_ms};
use crate::adapters::{jsonl, AppendParse, RequestPricing};
use crate::performance::{output_rate, PerformanceSample};
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
    turn_id: Option<String>,
    item: Option<RawItem>,
    started_at_ms: Option<i64>,
    completed_at_ms: Option<i64>,
    content_token_count: Option<u64>,
    duration_ms: Option<i64>,
    time_to_first_token_ms: Option<i64>,
    response_id: Option<String>,
    usage: Option<RawTokenUsage>,
}

#[derive(Debug, Deserialize)]
struct RawItem {
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawInfo {
    model: Option<String>,
    response_id: Option<String>,
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

/// Per-file parse state carried across incremental reads.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ParseState {
    /// Session's current model (token_count events usually omit it).
    model: Option<String>,
    /// Last cumulative totals seen.
    previous_totals: Option<RawTokenUsage>,
    /// Project name from the session's working directory.
    project: Option<String>,
    timing_turn: Option<TimingTurn>,
    pending_response: Option<PendingResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingResponse {
    id: String,
    usage: RawTokenUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimingTurn {
    id: String,
    model: String,
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
    parse_from(path, offset, state, offset)
}

/// Read timing and response IDs without re-emitting usage for the scanned prefix.
pub(crate) fn backfill_metadata(path: &Path, usage_from: u64) -> AppendParse {
    parse_from(path, 0, None, usage_from)
}

fn parse_from(path: &Path, offset: u64, state: Option<&str>, usage_from: u64) -> AppendParse {
    let mut st: ParseState = state
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let session_id = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut records = Vec::new();
    let mut performance = Vec::new();
    let mut request_pricing = Vec::new();
    // Only these lines carry usage, timing, or session metadata;
    // everything else (tool output, images) is skipped unparsed.
    let needles = [
        "\"token_count\"",
        "\"token_usage_record\"",
        "\"turn_context\"",
        "\"session_meta\"",
        "\"model\"",
        "\"model_name\"",
        "\"content_token_count\"",
        "\"task_complete\"",
    ];
    let end = jsonl::scan_lines(path, offset, &needles, |line, complete, start| {
        let mut scratch;
        let st = if complete {
            &mut st
        } else {
            scratch = st.clone();
            &mut scratch
        };
        if let Some(rec) = parse_line(
            line,
            st,
            &session_id,
            complete.then_some(&mut performance),
            &mut request_pricing,
        )
        .filter(|_| start >= usage_from)
        {
            records.push(rec);
        }
    });
    AppendParse {
        records,
        performance,
        request_pricing,
        offset: end.unwrap_or(offset),
        state: serde_json::to_string(&st).ok(),
    }
}

fn parse_line(
    line: &str,
    st: &mut ParseState,
    session_id: &str,
    performance: Option<&mut Vec<PerformanceSample>>,
    request_pricing: &mut Vec<RequestPricing>,
) -> Option<UsageRecord> {
    let raw = serde_json::from_str::<RawLine>(line).ok()?;
    let payload = raw.payload?;
    if raw.kind.as_deref() == Some("token_usage_record") {
        st.pending_response = payload.response_id.filter(|id| !id.trim().is_empty())
            .zip(payload.usage)
            .map(|(id, usage)| PendingResponse { id, usage });
        return None;
    }
    if let Some(performance) = performance {
        performance.extend(read_performance(
            &payload,
            raw.kind.as_deref(),
            raw.timestamp.as_deref(),
            st,
        ));
    }
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
    // Codex records the upstream response just before its token_count. A
    // repeated totals notification above must not consume a pending response.
    let pending = st.pending_response.take();
    let response_id = info.response_id.filter(|id| !id.trim().is_empty()).or_else(|| {
        pending.filter(|p| p.usage == usage).map(|p| p.id)
    });
    if usage.input_tokens == 0
        && usage.cached_input_tokens == 0
        && usage.output_tokens == 0
        && usage.reasoning_output_tokens == 0
    {
        return None;
    }
    let ts = raw.timestamp.as_deref().and_then(parse_timestamp_ms)?;
    let model = st.model.clone().unwrap_or_else(|| "gpt-5".to_string());
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
    if let Some(response_id) = response_id {
        request_pricing.push(RequestPricing::CodexResponse {
            usage_key: dedup_key.clone(),
            response_id,
        });
    }
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

fn read_performance(
    payload: &RawPayload,
    kind: Option<&str>,
    timestamp: Option<&str>,
    st: &mut ParseState,
) -> Option<PerformanceSample> {
    let id = payload.turn_id.as_deref().filter(|id| !id.is_empty())?;
    if kind == Some("turn_context") {
        st.timing_turn = payload
            .model
            .as_deref()
            .filter(|model| !model.is_empty())
            .map(|model| TimingTurn {
                id: id.to_string(),
                model: model.to_string(),
            });
        return None;
    }
    if kind != Some("event_msg") {
        return None;
    }
    let turn = st.timing_turn.as_ref().filter(|turn| turn.id == id)?;
    let (key, timestamp_ms, output_tps, first_token_ms) = match payload.kind.as_deref()? {
        "item_completed" => {
            // An assistant text item has its own stream window and token count.
            // Tool execution and hidden reasoning aren't part of this sample.
            let item = payload.item.as_ref()?;
            if item.kind.as_deref() != Some("AgentMessage") {
                return None;
            }
            let started = payload.started_at_ms.filter(|ms| *ms > 0)?;
            let end = payload.completed_at_ms?;
            let window = end.checked_sub(started)?;
            if window < 1000 {
                return None;
            }
            let rate = output_rate(payload.content_token_count?, window)?;
            (
                format!("item:{id}:{}", item.id.as_deref()?),
                end,
                Some(rate),
                None,
            )
        }
        "task_complete" => {
            let first = payload.time_to_first_token_ms?;
            if !(1..=payload.duration_ms?).contains(&first) {
                return None;
            }
            // This is the wait at the start of the turn, never a timer for
            // each model call that might have happened during the turn.
            (
                format!("turn:{id}"),
                parse_timestamp_ms(timestamp?)?,
                None,
                Some(first),
            )
        }
        _ => return None,
    };
    Some(PerformanceSample {
        key,
        timestamp_ms,
        model: turn.model.clone(),
        provider: String::new(),
        output_tps,
        first_token_ms,
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
        let mut links = Vec::new();
        let meta = r#"{"type":"session_meta","payload":{"cwd":"/Users/a/work/chaogu"}}"#;
        let ctx = r#"{"type":"turn_context","payload":{"model":"gpt-6.1-sol","cwd":"/Users/a/work/chaogu"}}"#;
        assert!(parse_line(meta, &mut st, "s", None, &mut links).is_none());
        assert!(parse_line(ctx, &mut st, "s", None, &mut links).is_none());
        let a = token_line("2026-09-30T10:00:00Z", (100, 40, 5), (100, 40, 5));
        let dup = token_line("2026-09-30T10:00:01Z", (100, 40, 5), (100, 40, 5));
        let b = token_line("2026-09-30T10:01:00Z", (50, 10, 2), (150, 50, 7));
        let first = parse_line(&a, &mut st, "s", None, &mut links).unwrap();
        assert_eq!(first.model, "gpt-6.1-sol");
        assert_eq!(first.project, "chaogu");
        assert_eq!((first.input_tokens, first.cache_read_tokens), (60, 40));
        assert!(parse_line(&dup, &mut st, "s", None, &mut links).is_none());
        let second = parse_line(&b, &mut st, "s", None, &mut links).unwrap();
        assert_eq!((second.input_tokens, second.output_tokens), (40, 2));
    }

    #[test]
    fn records_item_speed_and_turn_ttft_across_appends_without_changing_usage() {
        use std::io::Write;

        let path = crate::adapters::util::test_dir("codex-timing").join("session.jsonl");
        let context = r#"{"type":"turn_context","payload":{"turn_id":"t","model":"gpt-test"}}"#;
        let usage = token_line("2026-11-24T12:00:00Z", (500, 250, 120), (500, 250, 120));
        std::fs::write(&path, format!("{context}\n{usage}\n")).unwrap();
        let initial = parse_append(&path, 0, None);
        assert_eq!(initial.records.len(), 1);
        assert!(initial.performance.is_empty());

        let message = r#"{"type":"event_msg","payload":{"type":"item_completed","turn_id":"t",
            "item":{"type":"AgentMessage","id":"msg-1"},"started_at_ms":1795521600000,
            "completed_at_ms":1795521603000,"content_token_count":120}}"#.replace('\n', "");
        let done = r#"{"type":"event_msg","timestamp":"2026-11-24T12:01:00Z",
            "payload":{"type":"task_complete","turn_id":"t","duration_ms":60000,
            "time_to_first_token_ms":700}}"#.replace('\n', "");
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            file,
            "{}",
            message.replace("\"turn_id\":\"t\"", "\"turn_id\":\"other\"")
        )
        .unwrap();
        writeln!(file, "{message}").unwrap();
        write!(file, "{done}").unwrap();

        let append = parse_append(&path, initial.offset, initial.state.as_deref());
        assert!(append.records.is_empty());
        assert_eq!(append.performance.len(), 1);
        assert_eq!(append.performance[0].model, "gpt-test");
        assert_eq!(append.performance[0].output_tps, Some(40.0));

        writeln!(file).unwrap();
        let end = parse_append(&path, append.offset, append.state.as_deref());
        assert_eq!(end.performance.len(), 1);
        assert_eq!(end.performance[0].first_token_ms, Some(700));
        assert_eq!(end.performance[0].output_tps, None);

        let new_usage = token_line("2026-11-24T12:02:00Z", (100, 10, 20), (600, 260, 140));
        writeln!(file, "{new_usage}").unwrap();
        let backfill = backfill_metadata(&path, initial.offset);
        assert_eq!(backfill.performance.len(), 2);
        assert_eq!(backfill.records.len(), 1);
        assert_eq!(backfill.records[0].output_tokens, 20);
        assert!(backfill.offset > end.offset);
    }

    #[test]
    fn links_the_next_matching_usage_to_a_response_across_appends() {
        use std::io::Write;

        let path = crate::adapters::util::test_dir("codex-response-link").join("session.jsonl");
        let context = r#"{"type":"turn_context","payload":{"model":"gpt-6.2-sol"}}"#;
        let response = r#"{"type":"token_usage_record","payload":{"response_id":"resp_1",
            "usage":{"input_tokens":50,"cached_input_tokens":10,"output_tokens":2}}}"#.replace('\n', "");
        let first = token_line("2026-11-24T12:00:00Z", (100, 40, 5), (100, 40, 5));
        std::fs::write(&path, format!("{context}\n{first}\n{response}\n")).unwrap();
        let initial = parse_append(&path, 0, None);
        assert_eq!(initial.records.len(), 1);
        assert!(initial.request_pricing.is_empty());

        let usage = token_line("2026-11-24T12:01:00Z", (50, 10, 2), (150, 50, 7));
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{first}\n{usage}").unwrap();
        let append = parse_append(&path, initial.offset, initial.state.as_deref());
        assert_eq!(append.records.len(), 1);
        assert!(matches!(append.request_pricing.as_slice(),
            [RequestPricing::CodexResponse { usage_key, response_id }]
            if response_id == "resp_1" && Some(usage_key) == append.records[0].dedup_key.as_ref()));
        let backfill = backfill_metadata(&path, append.offset);
        assert!(backfill.records.is_empty());
        assert_eq!(backfill.request_pricing.len(), 1);
    }
}
