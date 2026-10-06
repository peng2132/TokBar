//! GitHub Copilot CLI adapter, ported from ccusage's copilot adapter.
//!
//! Two sources under ${COPILOT_HOME:-~/.copilot}:
//! - `session-state/<session-id>/events.jsonl`: written by default; each
//!   `session.shutdown` event carries cumulative per-model usage.
//! - OpenTelemetry JSONL under `otel/` (plus the file or directory named
//!   by $COPILOT_OTEL_FILE_EXPORTER_PATH). The same inference can appear
//!   as a chat span, an agent-summary span, an inference log, and an
//!   agent-turn log; sources are ranked and deduped per trace / response
//!   id so each call is counted once.
//!
//! When both exist for a session, session-state is authoritative (ccusage
//! `load_entries_inner`): OTel rows of a (session, model) at or before
//! its latest shutdown are dropped, later ones (a resumed session that
//! has not shut down yet) are kept.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::adapters::util;
use crate::types::UsageRecord;

pub const AGENT: &str = "copilot";

const SESSION_STATE_DIR: &str = "session-state";
const EVENTS_FILE: &str = "events.jsonl";

/// ${COPILOT_HOME:-~/.copilot}
fn copilot_root() -> Option<PathBuf> {
    std::env::var("COPILOT_HOME")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".copilot")))
}

fn exporter_path() -> Option<PathBuf> {
    std::env::var("COPILOT_OTEL_FILE_EXPORTER_PATH")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(root) = copilot_root() {
        for sub in ["otel", SESSION_STATE_DIR] {
            let p = root.join(sub);
            if p.is_dir() {
                dirs.push(p);
            }
        }
    }
    if let Some(p) = exporter_path() {
        if p.is_dir() {
            dirs.push(p);
        } else if p.is_file() {
            dirs.extend(p.parent().map(Path::to_path_buf));
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs
}

pub fn collect_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(root) = copilot_root() {
        util::collect_with_ext(&root.join("otel"), &["jsonl"], &mut files);
        files.extend(session_state_files(&root.join(SESSION_STATE_DIR)));
    }
    if let Some(p) = exporter_path() {
        if p.is_file() {
            files.push(p);
        } else if p.is_dir() {
            util::collect_with_ext(&p, &["jsonl"], &mut files);
        }
    }
    files.sort();
    files.dedup();
    files
}

/// `session-state/<session-id>/events.jsonl`, one level deep only, never
/// through symlinked session directories (ccusage `paths`).
fn session_state_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path().join(EVENTS_FILE))
        .filter(|p| p.is_file())
        .collect()
}

fn is_session_state_file(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == EVENTS_FILE)
        && path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|n| n == SESSION_STATE_DIR)
}

pub fn parse_file(path: &Path) -> Vec<UsageRecord> {
    if is_session_state_file(path) {
        parse_session_state_file(path)
    } else {
        let state_root = copilot_root().map(|r| r.join(SESSION_STATE_DIR));
        parse_otel_file(path, state_root.as_deref())
    }
}

/// ccusage `normalize_copilot_model`: internal long-context ids price
/// like their public model.
fn normalize_model(model: &str) -> String {
    let model = model.trim();
    model
        .strip_suffix("-1m-internal")
        .or_else(|| model.strip_suffix("-1m"))
        .unwrap_or(model)
        .to_string()
}

/// Non-negative integer from a JSON number or numeric string (ccusage
/// `number_value`).
fn number_value(v: Option<&Value>) -> Option<u64> {
    // Unclamped: also used for nanosecond timestamps. Token counts go
    // through `token_value`.
    match v? {
        Value::Number(n) => n.as_u64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite() && *f >= 0.0)
                .map(|f| f.trunc() as u64)
        }),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

/// A token count, clamped like every other adapter's (`util::MAX_TOKENS`)
/// so summing the parts cannot overflow.
fn token_value(v: Option<&Value>) -> Option<u64> {
    number_value(v).map(util::clamp_tokens)
}

fn str_value(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------
// Session-state (`session.shutdown` snapshots)
// ---------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Shutdown {
    ts: i64,
    model: String,
    /// Uncached input: Copilot's `inputTokens` includes cache read and
    /// cache write (ccusage `uncached_session_input_tokens`).
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    dedup_key: String,
}

/// Every `session.shutdown` snapshot in one events file, in file order.
fn read_shutdowns(path: &Path, session_id: &str) -> Vec<Shutdown> {
    let mut out = Vec::new();
    util::for_each_line(path, |line| {
        if !util::contains_bytes(line, b"session.shutdown") {
            return;
        }
        let Ok(event) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        if str_value(event.get("type")) != Some("session.shutdown") {
            return;
        }
        let Some(ts) = str_value(event.get("timestamp")).and_then(util::parse_rfc3339_ms) else {
            return;
        };
        let Some(metrics) = event
            .get("data")
            .and_then(|d| d.get("modelMetrics"))
            .and_then(Value::as_object)
        else {
            return;
        };
        let event_id = str_value(event.get("id"));
        for (raw_model, m) in metrics {
            let model = normalize_model(raw_model);
            let Some(usage) = m.get("usage").filter(|u| u.is_object()) else {
                continue;
            };
            let n = |k: &str| token_value(usage.get(k)).unwrap_or(0);
            let (input_raw, output, cache_read, cache_write, reasoning) = (
                n("inputTokens"),
                n("outputTokens"),
                n("cacheReadTokens"),
                n("cacheWriteTokens"),
                n("reasoningTokens"),
            );
            let requests = m
                .get("requests")
                .and_then(|r| token_value(r.get("count")))
                .unwrap_or(0);
            if model.is_empty()
                || input_raw + output + cache_read + cache_write + reasoning + requests == 0
            {
                continue;
            }
            // ccusage `session_state_dedup_key`.
            let dedup_key = match event_id {
                Some(id) => format!("copilot:shutdown:{session_id}:{id}:{model}"),
                None => format!(
                    "copilot:shutdown:{session_id}:{ts}:{model}:{input_raw}:{output}:{cache_read}:{cache_write}:{reasoning}:{requests}"
                ),
            };
            out.push(Shutdown {
                ts,
                model,
                input: input_raw.saturating_sub(cache_read.saturating_add(cache_write)),
                output,
                cache_read,
                cache_write,
                dedup_key,
            });
        }
    });
    out
}

/// Turn cumulative per-model snapshots into interval usage (ccusage
/// `reconcile_session_state_entries`): a resumed session writes one
/// shutdown per resume, each holding the running total, so every later
/// snapshot subtracts its predecessor. Duplicate snapshots (same key)
/// keep the latest one.
fn reconcile_shutdowns(snapshots: Vec<Shutdown>) -> Vec<Shutdown> {
    let mut latest_by_key: HashMap<&str, usize> = HashMap::new();
    for (i, s) in snapshots.iter().enumerate() {
        match latest_by_key.get(s.dedup_key.as_str()) {
            Some(&prev) if snapshots[prev].ts > s.ts => {}
            _ => {
                latest_by_key.insert(&s.dedup_key, i);
            }
        }
    }
    let mut keep: Vec<usize> = latest_by_key.into_values().collect();
    keep.sort_unstable();

    let mut by_model: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for &i in &keep {
        by_model.entry(snapshots[i].model.as_str()).or_default().push(i);
    }
    let mut out = Vec::new();
    for (_, mut idxs) in by_model {
        idxs.sort_by_key(|&i| (snapshots[i].ts, i));
        let mut previous: Option<&Shutdown> = None;
        for i in idxs {
            let cur = &snapshots[i];
            let interval = match previous {
                None => cur.clone(),
                Some(base) => Shutdown {
                    input: cur.input.saturating_sub(base.input),
                    output: cur.output.saturating_sub(base.output),
                    cache_read: cur.cache_read.saturating_sub(base.cache_read),
                    cache_write: cur.cache_write.saturating_sub(base.cache_write),
                    ..cur.clone()
                },
            };
            previous = Some(cur);
            if interval.input + interval.output + interval.cache_read + interval.cache_write > 0 {
                out.push(interval);
            }
        }
    }
    out.sort_by(|a, b| (a.ts, &a.dedup_key).cmp(&(b.ts, &b.dedup_key)));
    out
}

fn session_id_from_state_path(path: &Path) -> Option<String> {
    path.parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().trim().to_string())
        .filter(|n| !n.is_empty())
}

fn parse_session_state_file(path: &Path) -> Vec<UsageRecord> {
    let Some(session_id) = session_id_from_state_path(path) else {
        return Vec::new();
    };
    reconcile_shutdowns(read_shutdowns(path, &session_id))
        .into_iter()
        .map(|s| UsageRecord {
            agent: AGENT.to_string(),
            project: "GitHub Copilot CLI".to_string(),
            session_id: session_id.clone(),
            timestamp_ms: s.ts,
            model: s.model,
            input_tokens: s.input,
            output_tokens: s.output,
            cache_creation_5m: s.cache_write,
            cache_creation_1h: 0,
            cache_read_tokens: s.cache_read,
            cost_usd: None,
            dedup_key: Some(s.dedup_key),
        })
        .collect()
}

/// Latest shutdown timestamp per model for one session, read from its
/// session-state file. Used to drop OTel rows the snapshot already covers.
fn latest_shutdowns(state_root: &Path, session_id: &str) -> HashMap<String, i64> {
    let mut latest = HashMap::new();
    // The session id comes from telemetry attributes: only accept a plain
    // single path component so it can never point outside session-state.
    let safe = !session_id.is_empty()
        && session_id != "."
        && session_id != ".."
        && !session_id.contains(['/', '\\']);
    if !safe {
        return latest;
    }
    let path = state_root.join(session_id).join(EVENTS_FILE);
    if !path.is_file() {
        return latest;
    }
    for s in read_shutdowns(&path, session_id) {
        let e = latest.entry(s.model).or_insert(s.ts);
        if s.ts > *e {
            *e = s.ts;
        }
    }
    latest
}

// ---------------------------------------------------------------------
// OpenTelemetry export
// ---------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    AgentTurnLog,
    InferenceLog,
    AgentSummarySpan,
    ChatSpan,
}

/// Per-trace context filled from every attributed record of the trace
/// (ccusage `collect_trace_contexts`): first model wins, the session id
/// with the highest attribute priority wins.
#[derive(Default)]
struct TraceContext {
    model: Option<String>,
    session: Option<String>,
    session_priority: u8,
}

/// A usage candidate extracted in the single streaming pass; model and
/// session may still need the trace context, which is only complete once
/// the whole file has been read.
struct Candidate {
    source: Source,
    line_idx: usize,
    trace_id: Option<String>,
    span_id: Option<String>,
    response_id: Option<String>,
    turn_index: Option<u64>,
    model: Option<String>,
    session: Option<String>,
    ts: i64,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
}

const MODEL_ATTRS: &[&str] = &["gen_ai.response.model", "gen_ai.request.model"];
const SESSION_ATTRS: &[(&str, u8)] = &[
    ("gen_ai.conversation.id", 3),
    ("copilot_chat.session_id", 3),
    ("copilot_chat.chat_session_id", 3),
    ("session.id", 3),
    ("github.copilot.interaction_id", 2),
    ("gen_ai.response.id", 1),
];

fn attr_str<'a>(attrs: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    str_value(attrs.get(key))
}

fn attr_num(attrs: &Map<String, Value>, key: &str) -> u64 {
    token_value(attrs.get(key)).unwrap_or(0)
}

fn attr_num_first(attrs: &Map<String, Value>, keys: &[&str]) -> u64 {
    keys.iter().map(|k| attr_num(attrs, k)).find(|n| *n > 0).unwrap_or(0)
}

fn model_attr(attrs: &Map<String, Value>) -> Option<String> {
    MODEL_ATTRS
        .iter()
        .find_map(|k| attr_str(attrs, k))
        .map(normalize_model)
}

fn best_session_attr(attrs: &Map<String, Value>) -> Option<(String, u8)> {
    SESSION_ATTRS
        .iter()
        .filter_map(|(k, p)| attr_str(attrs, k).map(|s| (s.to_string(), *p)))
        .max_by_key(|(_, p)| *p)
}

fn trace_or_span_id(value: &Value, key: &str) -> Option<String> {
    str_value(value.get(key))
        .or_else(|| str_value(value.get("spanContext").and_then(|c| c.get(key))))
        .map(str::to_string)
}

/// ccusage `is_span_record`: an explicit string `type` decides; otherwise
/// a named record with span-shaped fields.
fn is_span_record(value: &Value) -> bool {
    if let Some(t) = value.get("type").and_then(Value::as_str) {
        return t == "span";
    }
    str_value(value.get("name")).is_some()
        && (str_value(value.get("spanId")).is_some()
            || str_value(value.get("traceId")).is_some()
            || ["startTime", "endTime", "duration", "kind"]
                .iter()
                .any(|k| value.get(*k).is_some()))
}

fn classify(value: &Value, attrs: &Map<String, Value>) -> Option<Source> {
    let op = attr_str(attrs, "gen_ai.operation.name").unwrap_or("");
    let name = str_value(value.get("name")).unwrap_or("");
    if is_span_record(value) {
        if op == "chat" || name.starts_with("chat ") {
            return Some(Source::ChatSpan);
        }
        if op == "invoke_agent" || name.starts_with("invoke_agent ") {
            return Some(Source::AgentSummarySpan);
        }
        return None;
    }
    let event = attr_str(attrs, "event.name").unwrap_or("");
    let body = str_value(value.get("body"))
        .or_else(|| str_value(value.get("_body")))
        .unwrap_or("");
    if event == "gen_ai.client.inference.operation.details" || body.starts_with("GenAI inference:")
    {
        return Some(Source::InferenceLog);
    }
    if event == "copilot_chat.agent.turn" || body.starts_with("copilot_chat.agent.turn") {
        return Some(Source::AgentTurnLog);
    }
    None
}

/// [seconds, nanos] hrtime arrays, smart-unit integers, or nothing.
fn record_timestamp(value: &Value) -> Option<i64> {
    for key in ["endTime", "startTime", "hrTime", "_hrTime", "time"] {
        if let Some(arr) = value.get(key).and_then(Value::as_array) {
            if let (Some(secs), Some(nanos)) =
                (number_value(arr.first()).filter(|s| *s > 0), number_value(arr.get(1)))
            {
                // checked math: a garbage seconds value must not panic.
                if let Some(ms) = secs
                    .checked_mul(1000)
                    .and_then(|ms| ms.checked_add(nanos / 1_000_000))
                {
                    return Some(ms.min(i64::MAX as u64) as i64);
                }
            }
        }
    }
    for key in ["timestamp", "observedTimestamp"] {
        if let Some(n) = number_value(value.get(key)).filter(|n| *n > 0) {
            return Some(util::smart_unit_ms(n.min(i64::MAX as u64) as i64));
        }
    }
    number_value(value.get("timeUnixNano"))
        .filter(|n| *n > 0)
        .map(|n| (n / 1_000_000).min(i64::MAX as u64) as i64)
}

/// Token math for one OTel record. OTel GenAI `output_tokens` already
/// includes reasoning, so `reasoning` is *not* added on top (ccusage
/// `does_not_double_count_reasoning_tokens`); only tokens the reported
/// total covers beyond input + output + cache are billed as extra output
/// (ccusage `apply_total_token_fallback`). Input includes cache reads in
/// Copilot telemetry, so they are split out.
/// Returns (input, output, cache_read, cache_creation).
fn otel_tokens(attrs: &Map<String, Value>) -> (u64, u64, u64, u64) {
    let input_raw = attr_num(attrs, "gen_ai.usage.input_tokens");
    let output = attr_num(attrs, "gen_ai.usage.output_tokens");
    let cache_read = attr_num(attrs, "gen_ai.usage.cache_read.input_tokens");
    let cache_creation = attr_num_first(
        attrs,
        &[
            "gen_ai.usage.cache_write.input_tokens",
            "gen_ai.usage.cache_creation.input_tokens",
        ],
    );
    let total = attr_num_first(
        attrs,
        &["gen_ai.usage.total_tokens", "gen_ai.usage.total.token_count"],
    );
    let input = input_raw - cache_read.min(input_raw);
    let known = input + output + cache_read + cache_creation;
    (input, output + total.saturating_sub(known), cache_read, cache_creation)
}

fn parse_otel_file(path: &Path, state_root: Option<&Path>) -> Vec<UsageRecord> {
    let fallback_ts = util::mtime_ms(path);
    let mut traces: HashMap<String, TraceContext> = HashMap::new();
    let mut candidates: Vec<Candidate> = Vec::new();

    // Single streaming pass: no whole-file buffer and no per-line clone
    // of `attributes`; only the small extracted candidates are kept.
    let mut idx = 0usize;
    util::for_each_line(path, |line| {
        let line_idx = idx;
        idx += 1;
        // Every usable record carries `attributes` (ccusage prefilter).
        if !util::contains_bytes(line, b"\"attributes\"") {
            return;
        }
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        let Some(attrs) = value.get("attributes").and_then(Value::as_object) else {
            return;
        };
        let trace_id = trace_or_span_id(&value, "traceId");
        if let Some(t) = &trace_id {
            let ctx = traces.entry(t.clone()).or_default();
            if ctx.model.is_none() {
                ctx.model = model_attr(attrs);
            }
            if let Some((s, p)) = best_session_attr(attrs) {
                if p > ctx.session_priority {
                    ctx.session = Some(s);
                    ctx.session_priority = p;
                }
            }
        }
        let Some(source) = classify(&value, attrs) else {
            return;
        };
        let (input, output, cache_read, cache_creation) = otel_tokens(attrs);
        if input + output + cache_read + cache_creation == 0 {
            return;
        }
        candidates.push(Candidate {
            source,
            line_idx,
            span_id: trace_or_span_id(&value, "spanId"),
            response_id: attr_str(attrs, "gen_ai.response.id").map(str::to_string),
            turn_index: number_value(attrs.get("turn.index"))
                .or_else(|| number_value(attrs.get("copilot_chat.turn.index"))),
            model: model_attr(attrs),
            session: best_session_attr(attrs).map(|(s, _)| s),
            ts: record_timestamp(&value).unwrap_or(fallback_ts),
            trace_id,
            input,
            output,
            cache_read,
            cache_creation,
        });
    });

    // Cross-source dedup: a lower-priority source is dropped when a
    // higher-priority source already covers its trace or response id.
    let ids_of = |src: Source| -> (HashSet<&str>, HashSet<&str>) {
        let mut tr = HashSet::new();
        let mut resp = HashSet::new();
        for c in candidates.iter().filter(|c| c.source == src) {
            tr.extend(c.trace_id.as_deref());
            resp.extend(c.response_id.as_deref());
        }
        (tr, resp)
    };
    let chat = ids_of(Source::ChatSpan);
    let inference = ids_of(Source::InferenceLog);
    let turn = ids_of(Source::AgentTurnLog);
    let covered = |c: &Candidate, (tr, resp): &(HashSet<&str>, HashSet<&str>)| {
        c.trace_id.as_deref().is_some_and(|t| tr.contains(t))
            || c.response_id.as_deref().is_some_and(|r| resp.contains(r))
    };
    let emit: Vec<bool> = candidates
        .iter()
        .map(|c| match c.source {
            Source::ChatSpan => true,
            Source::InferenceLog => !covered(c, &chat),
            Source::AgentTurnLog => !covered(c, &chat) && !covered(c, &inference),
            Source::AgentSummarySpan => {
                !covered(c, &chat) && !covered(c, &inference) && !covered(c, &turn)
            }
        })
        .collect();

    let mut shutdown_cache: HashMap<String, HashMap<String, i64>> = HashMap::new();
    let mut seen_keys = HashSet::new();
    let mut records = Vec::new();
    for (c, emit) in candidates.into_iter().zip(emit) {
        if !emit {
            continue;
        }
        let ctx = c.trace_id.as_ref().and_then(|t| traces.get(t));
        let model = c
            .model
            .or_else(|| ctx.and_then(|x| x.model.clone()))
            .unwrap_or_else(|| "unknown".to_string());
        let session_id = c
            .session
            .or_else(|| ctx.and_then(|x| x.session.clone()))
            .or_else(|| c.trace_id.clone())
            .unwrap_or_else(|| "unknown-session".to_string());

        if let Some(root) = state_root {
            let shutdowns = shutdown_cache
                .entry(session_id.clone())
                .or_insert_with(|| latest_shutdowns(root, &session_id));
            if shutdowns.get(&model).is_some_and(|shutdown_ts| c.ts <= *shutdown_ts) {
                continue;
            }
        }

        let (ts, idx) = (c.ts, c.line_idx);
        let dedup_key = match (c.source, &c.trace_id, &c.span_id) {
            (Source::AgentTurnLog, Some(t), _) => {
                let turn = c.turn_index.map_or_else(|| format!("idx-{idx}"), |n| n.to_string());
                format!("copilot:agent-turn:{t}:{turn}")
            }
            (Source::AgentTurnLog, None, _) => {
                let turn = c.turn_index.map_or_else(|| format!("idx-{idx}"), |n| n.to_string());
                format!("copilot:agent-turn:{session_id}:{turn}:{idx}")
            }
            (Source::InferenceLog, Some(t), Some(s)) => format!("copilot:log:{t}:{s}"),
            (Source::InferenceLog, _, _) => format!("copilot:log:{session_id}:{ts}:{idx}"),
            (_, Some(t), Some(s)) => format!("copilot:{t}:{s}"),
            _ => format!("copilot:span:{session_id}:{ts}:{idx}"),
        };
        if !seen_keys.insert(dedup_key.clone()) {
            continue;
        }
        records.push(UsageRecord {
            agent: AGENT.to_string(),
            project: "GitHub Copilot CLI".to_string(),
            session_id,
            timestamp_ms: ts,
            model,
            input_tokens: c.input,
            output_tokens: c.output,
            cache_creation_5m: c.cache_creation,
            cache_creation_1h: 0,
            cache_read_tokens: c.cache_read,
            cost_usd: None,
            dedup_key: Some(dedup_key),
        });
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn attrs(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn does_not_double_count_reasoning_tokens() {
        // ccusage loader test of the same name: reasoning is a subset of
        // output_tokens, so it must not be billed again.
        let a = attrs(json!({
            "gen_ai.usage.input_tokens": 100,
            "gen_ai.usage.output_tokens": 50,
            "gen_ai.usage.cache_read.input_tokens": 10,
            "gen_ai.usage.cache_creation.input_tokens": 20,
            "gen_ai.usage.reasoning.output_tokens": 5,
        }));
        assert_eq!(otel_tokens(&a), (90, 50, 10, 20));
    }

    #[test]
    fn bills_reasoning_only_when_total_exceeds_known_parts() {
        // ccusage `includes_separate_otel_reasoning_tokens_in_total_and_cost`.
        let a = attrs(json!({
            "gen_ai.usage.input_tokens": 100,
            "gen_ai.usage.output_tokens": 50,
            "gen_ai.usage.cache_read.input_tokens": 10,
            "gen_ai.usage.cache_creation.input_tokens": 20,
            "gen_ai.usage.reasoning.output_tokens": 5,
            "gen_ai.usage.total_tokens": 175,
        }));
        assert_eq!(otel_tokens(&a), (90, 55, 10, 20));
    }

    #[test]
    fn total_only_spans_bill_total_as_output() {
        let a = attrs(json!({
            "gen_ai.usage.total_tokens": 567,
            "gen_ai.usage.reasoning_tokens": 5,
        }));
        assert_eq!(otel_tokens(&a), (0, 567, 0, 0));
    }

    #[test]
    fn normalizes_internal_model_ids() {
        assert_eq!(normalize_model(" claude-opus-4.7-1m-internal "), "claude-opus-4.7");
        assert_eq!(normalize_model("claude-opus-4.6-1m"), "claude-opus-4.6");
        assert_eq!(normalize_model("gpt-5.4"), "gpt-5.4");
    }

    #[test]
    fn hrtime_overflow_does_not_panic() {
        let v = json!({"endTime": [u64::MAX, 5]});
        let _ = record_timestamp(&v);
        let v = json!({"endTime": [1_775_934_264_u64, 967_317_833_u64]});
        assert_eq!(record_timestamp(&v), Some(1_775_934_264_967));
    }

    fn shutdown(id: &str, ts: &str, model: &str, usage: Value) -> String {
        json!({
            "type": "session.shutdown",
            "id": id,
            "timestamp": ts,
            "data": {"modelMetrics": {model: {"usage": usage, "requests": {"count": 1}}}}
        })
        .to_string()
    }

    #[test]
    fn session_state_splits_cumulative_shutdowns_into_intervals() {
        let root = util::test_dir("copilot-state");
        let file = root.join(SESSION_STATE_DIR).join("session-1").join(EVENTS_FILE);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let lines = [
            r#"{"type":"user.message","data":{"content":"hi"}}"#.to_string(),
            "not json".to_string(),
            shutdown(
                "shutdown-old",
                "2026-01-02T01:20:00.000Z",
                "test-model-1m",
                json!({"inputTokens": 100, "outputTokens": 50, "cacheReadTokens": 10, "cacheWriteTokens": 20}),
            ),
            // Duplicate of the first snapshot: must not count twice.
            shutdown(
                "shutdown-old",
                "2026-01-02T01:20:00.000Z",
                "test-model-1m",
                json!({"inputTokens": 100, "outputTokens": 50, "cacheReadTokens": 10, "cacheWriteTokens": 20}),
            ),
            shutdown(
                "shutdown-new",
                "2026-01-03T01:20:00.000Z",
                "test-model-1m",
                json!({"inputTokens": 200, "outputTokens": 80, "cacheReadTokens": 20, "cacheWriteTokens": 30}),
            ),
        ];
        std::fs::write(&file, lines.join("\n")).unwrap();
        assert!(is_session_state_file(&file));

        let records = parse_file(&file);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].model, "test-model");
        assert_eq!(records[0].session_id, "session-1");
        assert_eq!((records[0].input_tokens, records[0].output_tokens), (70, 50));
        assert_eq!((records[0].cache_read_tokens, records[0].cache_creation_5m), (10, 20));
        assert_eq!(
            records[0].dedup_key.as_deref(),
            Some("copilot:shutdown:session-1:shutdown-old:test-model")
        );
        // 200-20-30=150 cumulative uncached input, minus 70 already seen.
        assert_eq!((records[1].input_tokens, records[1].output_tokens), (80, 30));
        assert_eq!((records[1].cache_read_tokens, records[1].cache_creation_5m), (10, 10));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn otel_rows_covered_by_a_shutdown_are_dropped() {
        let root = util::test_dir("copilot-otel");
        let state_root = root.join(SESSION_STATE_DIR);
        let state = state_root.join("session-1").join(EVENTS_FILE);
        std::fs::create_dir_all(state.parent().unwrap()).unwrap();
        std::fs::write(
            &state,
            shutdown(
                "shutdown-1",
                "2026-04-15T09:52:27.352Z",
                "test-model",
                json!({"inputTokens": 10, "outputTokens": 20}),
            ),
        )
        .unwrap();
        let span = |trace: &str, end_secs: u64, model: &str, session: &str, input: u64| {
            json!({
                "type": "span",
                "traceId": trace,
                "spanId": format!("{trace}-span"),
                "name": format!("chat {model}"),
                "endTime": [end_secs, 0],
                "attributes": {
                    "gen_ai.operation.name": "chat",
                    "gen_ai.response.model": model,
                    "gen_ai.conversation.id": session,
                    "gen_ai.usage.input_tokens": input,
                    "gen_ai.usage.output_tokens": 1
                }
            })
            .to_string()
        };
        // 2026-04-15T09:52:27Z == 1776246747.
        let otel = root.join("otel.jsonl");
        std::fs::write(
            &otel,
            [
                json!({"type": "metric", "name": "gen_ai.client.token.usage"}).to_string(),
                span("covered", 1_776_246_700, "test-model", "session-1", 100),
                span("after-shutdown", 1_776_246_800, "test-model", "session-1", 7),
                span("other-model", 1_776_246_700, "other-model", "session-1", 3),
                span("other-session", 1_776_246_700, "test-model", "session-2", 5),
            ]
            .join("\n"),
        )
        .unwrap();

        let records = parse_otel_file(&otel, Some(&state_root));
        let inputs: Vec<(String, String, u64)> = records
            .iter()
            .map(|r| (r.session_id.clone(), r.model.clone(), r.input_tokens))
            .collect();
        assert_eq!(
            inputs,
            vec![
                ("session-1".to_string(), "test-model".to_string(), 7),
                ("session-1".to_string(), "other-model".to_string(), 3),
                ("session-2".to_string(), "test-model".to_string(), 5),
            ]
        );
        // Without session-state every span counts.
        assert_eq!(parse_otel_file(&otel, None).len(), 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn suppresses_lower_priority_records_for_same_response() {
        let root = util::test_dir("copilot-dedup");
        let otel = root.join("otel.jsonl");
        std::fs::write(
            &otel,
            [
                json!({
                    "type": "span", "traceId": "t", "spanId": "agent-1",
                    "name": "invoke_agent GitHub Copilot Chat",
                    "attributes": {
                        "gen_ai.operation.name": "invoke_agent",
                        "gen_ai.response.model": "gpt-5.4-mini",
                        "gen_ai.conversation.id": "conv",
                        "gen_ai.response.id": "resp",
                        "gen_ai.usage.input_tokens": 100,
                        "gen_ai.usage.output_tokens": 30
                    }
                })
                .to_string(),
                json!({
                    "hrTime": [1_775_934_263_u64, 0_u64],
                    "attributes": {
                        "event.name": "gen_ai.client.inference.operation.details",
                        "gen_ai.response.model": "gpt-5.4-mini",
                        "gen_ai.response.id": "resp",
                        "gen_ai.usage.input_tokens": 80,
                        "gen_ai.usage.output_tokens": 20
                    },
                    "_body": "GenAI inference: gpt-5.4-mini"
                })
                .to_string(),
                json!({
                    "type": "span", "traceId": "t", "spanId": "chat-1",
                    "name": "chat gpt-5.4-mini",
                    "attributes": {
                        "gen_ai.operation.name": "chat",
                        "gen_ai.response.model": "gpt-5.4-mini",
                        "gen_ai.conversation.id": "conv",
                        "gen_ai.response.id": "resp",
                        "gen_ai.usage.input_tokens": 60,
                        "gen_ai.usage.output_tokens": 10
                    }
                })
                .to_string(),
            ]
            .join("\n"),
        )
        .unwrap();
        let records = parse_otel_file(&otel, None);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_tokens, 60);
        assert_eq!(records[0].dedup_key.as_deref(), Some("copilot:t:chat-1"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
