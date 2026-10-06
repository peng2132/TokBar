//! pi-agent adapter, ported from ccusage's pi adapter.
//!
//! Sessions are JSONL files under $PI_AGENT_DIR (or
//! ~/.pi/agent/sessions). Assistant `message` lines carry usage with
//! camelCase cache fields and an optional pre-computed `cost.total`.
//!
//! Two kinds of copies would otherwise be double counted:
//! - A forked session (header `parentSession`) starts by replaying the
//!   parent's history up to the fork. Like ccusage `PiReplayPlan`, the
//!   child's leading usage lines that match the parent's (active-branch)
//!   usage up to the fork timestamp are skipped.
//! - pi-subagents writes derived debug transcripts under
//!   `subagent-artifacts/`; they duplicate calls already recorded in the
//!   primary session files and are not collected at all (ccusage
//!   `is_subagent_artifact_transcript`).
//!
//! The dedup key is content-only (no file-derived project/session), so a
//! line copied or moved into another session file collapses onto the
//! original row. Deviation from ccusage, whose entry id includes the
//! file-derived session: two files holding a byte-identical usage line
//! (same millisecond timestamp, model and token counts) count once here.
//!
//! Deviation from ccusage: model names are stored without the "[pi] "
//! display prefix so LiteLLM pricing matching keeps working; the agent
//! column already attributes the usage.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::adapters::util;
use crate::types::UsageRecord;

pub const AGENT: &str = "pi";

pub fn data_dirs() -> Vec<PathBuf> {
    util::env_dirs("PI_AGENT_DIR", ".pi/agent/sessions")
}

pub fn collect_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in data_dirs() {
        util::walk_files(
            &dir,
            &|p| p.extension().is_some_and(|e| e == "jsonl") && !is_subagent_artifact(p),
            &mut files,
        );
    }
    files.sort();
    files
}

/// ccusage `is_subagent_artifact_transcript`.
fn is_subagent_artifact(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str().to_string_lossy() == "subagent-artifacts")
}

/// Path component right after "sessions" is the project name.
fn project_from_path(path: &Path) -> String {
    let mut take_next = false;
    for comp in path.components() {
        let name = comp.as_os_str().to_string_lossy();
        if take_next {
            return name.to_string();
        }
        if name == "sessions" {
            take_next = true;
        }
    }
    "unknown".to_string()
}

fn session_from_path(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .map(|stem| {
            stem.split_once('_')
                .map(|(_, rest)| rest.to_string())
                .unwrap_or(stem)
        })
        .unwrap_or_default()
}

/// First-line `{"type":"session", ...}` header.
#[derive(Debug, Clone)]
struct Header {
    parent_session: Option<PathBuf>,
    /// `parentSession` present but not a non-empty string.
    parent_malformed: bool,
    timestamp: Option<i64>,
}

fn parse_header(line: &[u8]) -> Option<Header> {
    let v: Value = serde_json::from_slice(line).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("session") {
        return None;
    }
    let parent = v.get("parentSession");
    let parent_str = parent.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    Some(Header {
        parent_session: parent_str.map(PathBuf::from),
        parent_malformed: parent.is_some() && parent_str.is_none(),
        timestamp: v.get("timestamp").and_then(util::ts_from_value),
    })
}

fn read_header(path: &Path) -> Option<Header> {
    parse_header(&util::first_line(path)?)
}

/// Everything ccusage compares to decide that a child usage line is a
/// replay of the parent's (`PiUsageSignature`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Signature {
    ts: i64,
    model: Option<String>,
    input: u64,
    output: u64,
    cache_write: u64,
    cache_read: u64,
    effective_total: u64,
    cost: String,
}

struct Usage {
    sig: Signature,
    entry_id: Option<String>,
    record: UsageRecord,
}

/// Tree-format link fields carried by every pi session entry; parsed
/// without materializing the (possibly large) message bodies.
#[derive(Deserialize)]
struct LinkRaw {
    #[serde(rename = "type", default)]
    kind: Option<Value>,
    #[serde(default)]
    id: Option<Value>,
    #[serde(rename = "parentId", default)]
    parent_id: Option<Value>,
}

struct Link {
    id: Option<String>,
    parent_id: Option<String>,
}

enum ReplayPath {
    /// v1 sessions predate entry links: physical order is the history.
    Linear,
    /// Usage indices on the active branch (root -> leaf).
    Active(Vec<usize>),
    /// Links are inconsistent; never suppress anything against it.
    Invalid,
}

struct Session {
    header: Option<Header>,
    usage: Vec<Usage>,
    replay: ReplayPath,
}

fn non_empty_str(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Read one session file. Entry links (needed only for replay matching)
/// are collected when `with_links` is set.
fn read_session(path: &Path, with_links: bool) -> Option<Session> {
    let project = project_from_path(path);
    let session_id = session_from_path(path);
    let mut header = None;
    let mut links: Vec<Link> = Vec::new();
    let mut usage: Vec<Usage> = Vec::new();
    let mut first = true;
    let opened = util::for_each_line(path, |line| {
        if first {
            first = false;
            header = parse_header(line);
        }
        if with_links {
            if let Ok(raw) = serde_json::from_slice::<LinkRaw>(line) {
                if raw.kind.as_ref().and_then(Value::as_str) != Some("session") {
                    links.push(Link {
                        id: non_empty_str(raw.id.as_ref()),
                        parent_id: non_empty_str(raw.parent_id.as_ref()),
                    });
                }
            }
        }
        // Cheap pre-filter, same as ccusage.
        if !util::contains_bytes(line, b"\"usage\"") || !util::contains_bytes(line, b"\"message\"")
        {
            return;
        }
        if let Ok(value) = serde_json::from_slice::<Value>(line) {
            usage.extend(usage_from_line(&value, &project, &session_id));
        }
    });
    if !opened {
        return None;
    }
    let replay = if with_links {
        replay_path(&links, &usage)
    } else {
        ReplayPath::Linear
    };
    Some(Session {
        header,
        usage,
        replay,
    })
}

fn usage_from_line(value: &Value, project: &str, session_id: &str) -> Option<Usage> {
    if let Some(kind) = value.get("type").and_then(Value::as_str) {
        if kind != "message" {
            return None;
        }
    }
    let message = value.get("message")?;
    if message.get("role").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let usage = message.get("usage")?;
    let ts = value.get("timestamp").and_then(util::ts_from_value)?;

    let input = util::get_u64(usage, "input");
    let mut output = util::get_u64(usage, "output");
    let cache_read = util::get_u64(usage, "cacheRead");
    let cache_write = util::get_u64(usage, "cacheWrite");
    let total = util::get_u64(usage, "totalTokens");
    if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
        if total == 0 {
            return None;
        }
        output = total;
    }
    let raw_model = util::get_str(message, &["model"]);
    let model = raw_model.unwrap_or("unknown");
    // pi writes the cost under `message.usage.cost.total`.
    let cost = usage
        .get("cost")
        .and_then(|c| c.get("total"))
        .and_then(Value::as_f64)
        .filter(|c| c.is_finite() && *c > 0.0);
    let dedup_key = format!(
        "pi:{ts}:{model}:{input}:{output}:{cache_write}:{cache_read}:{total}"
    );
    Some(Usage {
        sig: Signature {
            ts,
            model: raw_model.map(str::to_string),
            input,
            output,
            cache_write,
            cache_read,
            effective_total: total.max(input + output + cache_write + cache_read),
            cost: util::cost_key(cost),
        },
        entry_id: non_empty_str(value.get("id")),
        record: UsageRecord {
            agent: AGENT.to_string(),
            project: project.to_string(),
            session_id: session_id.to_string(),
            timestamp_ms: ts,
            model: model.to_string(),
            input_tokens: input,
            output_tokens: output,
            cache_creation_5m: cache_write,
            cache_creation_1h: 0,
            cache_read_tokens: cache_read,
            cost_usd: cost,
            dedup_key: Some(dedup_key),
        },
    })
}

/// ccusage `replay_usage_path`: pi writes the current leaf last; walking
/// its parents excludes abandoned sibling branches that remain earlier
/// in the same physical file.
fn replay_path(links: &[Link], usage: &[Usage]) -> ReplayPath {
    if links.iter().all(|l| l.id.is_none() && l.parent_id.is_none()) {
        return ReplayPath::Linear;
    }
    let mut parents: HashMap<&str, Option<&str>> = HashMap::new();
    let mut leaf = None;
    for link in links {
        // A partially linked session cannot identify an active branch.
        let Some(id) = link.id.as_deref() else {
            return ReplayPath::Invalid;
        };
        if parents.insert(id, link.parent_id.as_deref()).is_some() {
            return ReplayPath::Invalid;
        }
        leaf = Some(id);
    }
    let Some(mut entry) = leaf else {
        return ReplayPath::Linear;
    };
    if parents.values().flatten().any(|p| !parents.contains_key(p)) || has_cycle(&parents) {
        return ReplayPath::Invalid;
    }
    let usage_by_id: HashMap<&str, usize> = usage
        .iter()
        .enumerate()
        .filter_map(|(i, u)| u.entry_id.as_deref().map(|id| (id, i)))
        .collect();
    let mut active = Vec::new();
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(entry) {
            return ReplayPath::Invalid;
        }
        if let Some(&i) = usage_by_id.get(entry) {
            active.push(i);
        }
        match parents.get(entry) {
            None => return ReplayPath::Invalid,
            Some(None) => break,
            Some(Some(parent)) => entry = parent,
        }
    }
    active.reverse();
    ReplayPath::Active(active)
}

fn has_cycle(parents: &HashMap<&str, Option<&str>>) -> bool {
    let mut validated: HashSet<&str> = HashSet::new();
    for &start in parents.keys() {
        if validated.contains(start) {
            continue;
        }
        let mut path = HashSet::new();
        let mut entry = start;
        while !validated.contains(entry) {
            if !path.insert(entry) {
                return true;
            }
            match parents.get(entry) {
                None => return true,
                Some(None) => break,
                Some(Some(parent)) => entry = parent,
            }
        }
        validated.extend(path);
    }
    false
}

/// Number of leading child usage lines that replay the parent's history
/// (ccusage `PiSessionData::matching_replay_prefix`).
fn matching_replay_prefix(parent: &Session, child: &Session, fork_ts: i64) -> Option<usize> {
    if matches!(child.replay, ReplayPath::Invalid) {
        return None;
    }
    let parent_sigs: Vec<&Signature> = match &parent.replay {
        ReplayPath::Linear => parent.usage.iter().map(|u| &u.sig).collect(),
        ReplayPath::Active(idx) => idx
            .iter()
            .filter_map(|&i| parent.usage.get(i))
            .map(|u| &u.sig)
            .collect(),
        ReplayPath::Invalid => return None,
    };
    Some(
        child
            .usage
            .iter()
            .map(|u| &u.sig)
            .zip(parent_sigs.into_iter().take_while(|s| s.ts <= fork_ts))
            .take_while(|(c, p)| c == p)
            .count(),
    )
}

/// Lexical normalization (no symlink resolution), ccusage `normalize_path`.
fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `parentSession` is absolute or relative to the child's directory.
fn resolve_parent(parent: &Path, child: &Path) -> Option<PathBuf> {
    let candidate = if parent.is_absolute() {
        normalize_path(parent)
    } else {
        normalize_path(&child.parent()?.join(parent))
    };
    (candidate.is_file() && !is_subagent_artifact(&candidate)).then_some(candidate)
}

fn header_is_valid(h: &Header) -> bool {
    h.timestamp.is_some() && !h.parent_malformed
}

/// ccusage `has_valid_lineage`: every session on the way to the root must
/// have a well-formed header and a resolvable parent, without cycles.
/// Returns the resolved parent path when the child's lineage is valid.
fn valid_parent(child_path: &Path, child_header: &Header) -> Option<PathBuf> {
    if !header_is_valid(child_header) {
        return None;
    }
    let parent = resolve_parent(child_header.parent_session.as_deref()?, child_path)?;
    let mut visited: HashSet<PathBuf> = HashSet::new();
    visited.insert(normalize_path(child_path));
    let mut current = parent.clone();
    loop {
        if !visited.insert(current.clone()) {
            return None; // self-reference or cycle
        }
        let header = read_header(&current)?;
        if !header_is_valid(&header) {
            return None;
        }
        match header.parent_session {
            None => return Some(parent),
            Some(p) => current = resolve_parent(&p, &current)?,
        }
    }
}

pub fn parse_file(path: &Path) -> Vec<UsageRecord> {
    if is_subagent_artifact(path) {
        return Vec::new();
    }
    let forked = read_header(path).is_some_and(|h| h.parent_session.is_some());
    let Some(child) = read_session(path, forked) else {
        return Vec::new();
    };
    let skip = child
        .header
        .as_ref()
        .and_then(|h| Some((valid_parent(path, h)?, h.timestamp?)))
        .and_then(|(parent_path, fork_ts)| {
            let parent = read_session(&parent_path, true)?;
            matching_replay_prefix(&parent, &child, fork_ts)
        })
        .unwrap_or(0);
    child
        .usage
        .into_iter()
        .skip(skip)
        .map(|u| u.record)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn session_line(id: &str, ts: &str, parent: Option<&Path>) -> String {
        let mut line = json!({"type": "session", "id": id, "timestamp": ts});
        if let Some(parent) = parent {
            line["parentSession"] = json!(parent.to_string_lossy());
        }
        line.to_string()
    }

    fn usage_line(ts: &str, input: u64, output: u64, cache_read: u64, cache_write: u64) -> String {
        json!({
            "type": "message",
            "timestamp": ts,
            "message": {
                "role": "assistant",
                "model": "gpt-5",
                "usage": {
                    "input": input, "output": output,
                    "cacheRead": cache_read, "cacheWrite": cache_write,
                    "totalTokens": input + output + cache_read + cache_write,
                    "cost": {"total": 0.01}
                }
            }
        })
        .to_string()
    }

    fn linked_usage_line(id: &str, parent: Option<&str>, ts: &str, input: u64) -> String {
        let mut line: Value = serde_json::from_str(&usage_line(ts, input, 10, 20, 3)).unwrap();
        line["id"] = json!(id);
        line["parentId"] = parent.map_or(Value::Null, |p| json!(p));
        line.to_string()
    }

    fn write(root: &Path, rel: &str, lines: &[String]) -> PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    fn inputs(records: &[UsageRecord]) -> Vec<u64> {
        records.iter().map(|r| r.input_tokens).collect()
    }

    #[test]
    fn skips_replayed_parent_prefix_but_keeps_child_usage() {
        let root = util::test_dir("pi-fork");
        let parent = write(
            &root,
            "sessions/project-a/root.jsonl",
            &[
                session_line("root", "2026-01-01T00:00:00.000Z", None),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
                usage_line("2026-01-03T00:00:00.000Z", 200, 20, 30, 4),
                usage_line("2026-01-04T00:00:00.000Z", 50, 5, 6, 1),
            ],
        );
        let child_a = write(
            &root,
            "sessions/project-a/child-a.jsonl",
            &[
                session_line("child-a", "2026-01-03T00:00:00.000Z", Some(&parent)),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
                usage_line("2026-01-03T00:00:00.000Z", 200, 20, 30, 4),
                usage_line("2026-01-04T00:00:00.000Z", 50, 5, 6, 1),
            ],
        );
        let child_b = write(
            &root,
            "sessions/project-a/child-b.jsonl",
            &[
                session_line("child-b", "2026-01-03T00:00:00.000Z", Some(&parent)),
                usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3),
                usage_line("2026-01-03T00:00:00.000Z", 200, 20, 30, 999),
                usage_line("2026-01-03T01:00:00.000Z", 70, 7, 8, 2),
            ],
        );

        assert_eq!(inputs(&parse_file(&parent)), vec![100, 200, 50]);
        // Only usage at or before the fork timestamp can be a replay.
        assert_eq!(inputs(&parse_file(&child_a)), vec![50]);
        let b = parse_file(&child_b);
        assert_eq!(inputs(&b), vec![200, 70]);
        assert_eq!(b[0].cache_creation_5m, 999);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn skips_the_copied_active_parent_branch_after_an_abandoned_sibling() {
        let root = util::test_dir("pi-branch");
        let parent = write(
            &root,
            "sessions/project-a/root.jsonl",
            &[
                session_line("root", "2026-01-01T00:00:00.000Z", None),
                linked_usage_line("a", None, "2026-01-02T10:00:00.000Z", 100),
                linked_usage_line("x", Some("a"), "2026-01-02T11:00:00.000Z", 200),
                linked_usage_line("y", Some("a"), "2026-01-02T12:00:00.000Z", 300),
                linked_usage_line("z", Some("y"), "2026-01-02T13:00:00.000Z", 400),
            ],
        );
        let child = write(
            &root,
            "sessions/project-a/child.jsonl",
            &[
                session_line("child", "2026-01-03T00:00:00.000Z", Some(&parent)),
                linked_usage_line("copy-a", None, "2026-01-02T10:00:00.000Z", 100),
                linked_usage_line("copy-y", Some("copy-a"), "2026-01-02T12:00:00.000Z", 300),
                linked_usage_line("copy-z", Some("copy-y"), "2026-01-02T13:00:00.000Z", 400),
                linked_usage_line("child-only", Some("copy-z"), "2026-01-03T01:00:00.000Z", 500),
            ],
        );
        assert_eq!(inputs(&parse_file(&child)), vec![500]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fails_open_for_missing_self_referential_and_cyclic_parents() {
        let root = util::test_dir("pi-cycle");
        let line = usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3);
        let self_path = root.join("sessions/p/self.jsonl");
        write(
            &root,
            "sessions/p/self.jsonl",
            &[session_line("self", "2026-01-03T00:00:00.000Z", Some(&self_path)), line.clone()],
        );
        let a = root.join("sessions/p/cycle-a.jsonl");
        let b = root.join("sessions/p/cycle-b.jsonl");
        write(
            &root,
            "sessions/p/cycle-a.jsonl",
            &[session_line("a", "2026-01-03T00:00:00.000Z", Some(&b)), line.clone()],
        );
        write(
            &root,
            "sessions/p/cycle-b.jsonl",
            &[session_line("b", "2026-01-03T00:00:00.000Z", Some(&a)), line.clone()],
        );
        let missing = write(
            &root,
            "sessions/p/orphan.jsonl",
            &[
                session_line("o", "2026-01-03T00:00:00.000Z", Some(&root.join("nope.jsonl"))),
                line.clone(),
            ],
        );
        for p in [&self_path, &a, &b, &missing] {
            assert_eq!(parse_file(p).len(), 1, "{}", p.display());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn subagent_artifacts_are_excluded() {
        assert!(is_subagent_artifact(Path::new(
            "sessions/project-a/root/subagent-artifacts/run1_agent_0_transcript.jsonl"
        )));
        assert!(!is_subagent_artifact(Path::new(
            "sessions/project-a/root/run-1/child.jsonl"
        )));
        let root = util::test_dir("pi-artifacts");
        let artifact = write(
            &root,
            "sessions/project-a/root/subagent-artifacts/run1_agent_0_transcript.jsonl",
            &[usage_line("2026-01-02T10:00:00.000Z", 1, 1, 0, 0)],
        );
        assert!(parse_file(&artifact).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dedup_key_ignores_file_location_and_cost_comes_from_usage() {
        let line: Value =
            serde_json::from_str(&usage_line("2026-01-02T10:00:00.000Z", 100, 10, 20, 3)).unwrap();
        let a = usage_from_line(&line, "project-a", "s1").unwrap().record;
        let b = usage_from_line(&line, "project-b", "s2").unwrap().record;
        assert_eq!(a.dedup_key, b.dedup_key);
        assert_eq!(a.cost_usd, Some(0.01));
    }
}
