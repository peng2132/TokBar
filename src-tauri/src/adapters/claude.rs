use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::adapters::{jsonl, AppendParse};
use crate::types::UsageRecord;

pub const AGENT: &str = "claude-code";

/// Raw JSONL line schema, ported from ccusage types.rs.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawEntry {
    session_id: Option<String>,
    cwd: Option<String>,
    timestamp: Option<String>,
    message: Option<RawMessage>,
    #[serde(rename = "costUSD")]
    cost_usd: Option<f64>,
    request_id: Option<String>,
    is_api_error_message: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    usage: Option<RawUsage>,
    model: Option<String>,
    id: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    cache_creation: Option<RawCacheCreation>,
    /// "standard" | "fast" — fast/priority tier is billed at a multiplier.
    speed: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawCacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

/// Discover Claude Code data directories, in ccusage priority order:
/// 1. $CLAUDE_CONFIG_DIR (comma-separated)
/// 2. $XDG_CONFIG_HOME/claude or ~/.config/claude
/// 3. ~/.claude
pub fn data_dirs() -> Vec<PathBuf> {
    let mut dirs_out = Vec::new();
    if let Ok(env_paths) = std::env::var("CLAUDE_CONFIG_DIR") {
        for p in env_paths.split(',') {
            let p = p.trim();
            if p.is_empty() {
                continue;
            }
            let path = PathBuf::from(p);
            let projects = if path.ends_with("projects") {
                path
            } else {
                path.join("projects")
            };
            if projects.is_dir() {
                dirs_out.push(projects);
            }
        }
        if !dirs_out.is_empty() {
            return dirs_out;
        }
    }
    if let Some(home) = dirs::home_dir() {
        for base in [home.join(".config").join("claude"), home.join(".claude")] {
            let projects = base.join("projects");
            if projects.is_dir() {
                dirs_out.push(projects);
            }
        }
    }
    dirs_out
}

/// Recursively collect all .jsonl files under the projects dirs.
pub fn collect_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in data_dirs() {
        collect_jsonl(&dir, &mut files);
    }
    files
}

/// Recursive .jsonl walk that never follows symlinked directories (a
/// link back up the tree would loop and list every file many times).
pub(crate) fn collect_jsonl(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let path = entry.path();
        if ft.is_dir() {
            collect_jsonl(&path, files);
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            files.push(path);
        }
    }
}

/// Per-file parse state carried across incremental reads.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ParseState {
    /// Project name from the first `cwd` seen in the file.
    project: Option<String>,
}

/// Parse one JSONL file into normalized usage records.
/// Skips lines without usage data, synthetic models, and API error messages.
pub fn parse_file(path: &Path) -> Vec<UsageRecord> {
    parse_append(path, 0, None).records
}

/// Incremental parse from byte `offset` (see [`AppendParse`]).
pub fn parse_append(path: &Path, offset: u64, state: Option<&str>) -> AppendParse {
    let mut st: ParseState = state
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let file_session = session_id_from_path(path);
    let path_project = project_name_from_path(path);

    let mut records = Vec::new();
    let end = jsonl::scan_lines(path, offset, &["\"usage\""], |line, complete| {
        // A trailing line still being written is parsed for display but
        // its state is not kept: it is re-read once complete.
        let mut scratch;
        let st = if complete {
            &mut st
        } else {
            scratch = st.clone();
            &mut scratch
        };
        if let Some(rec) = parse_line(line, st, &file_session, &path_project) {
            records.push(rec);
        }
    });
    AppendParse {
        records,
        offset: end.unwrap_or(offset),
        state: serde_json::to_string(&st).ok(),
    }
}

fn parse_line(
    line: &str,
    st: &mut ParseState,
    file_session: &str,
    path_project: &str,
) -> Option<UsageRecord> {
    let raw = serde_json::from_str::<RawEntry>(line).ok()?;
    if st.project.is_none() {
        st.project = raw.cwd.as_deref().and_then(jsonl::project_from_cwd);
    }
    if raw.is_api_error_message == Some(true) {
        return None;
    }
    let message = raw.message?;
    let usage = message.usage?;
    let mut model = message.model.unwrap_or_default();
    if model.is_empty() || model == "<synthetic>" {
        return None;
    }
    // ccusage reports fast-tier usage as "<model>-fast" so the fast
    // multiplier applies and breakdowns separate the two tiers.
    if usage.speed.as_deref() == Some("fast") {
        model.push_str("-fast");
    }
    if usage.input_tokens == 0
        && usage.output_tokens == 0
        && usage.cache_creation_input_tokens == 0
        && usage.cache_read_input_tokens == 0
    {
        return None;
    }
    let ts = raw.timestamp.as_deref().and_then(parse_timestamp_ms)?;
    // cache_creation breakdown (5m/1h) supersedes the legacy total field.
    let (cache_5m, cache_1h) = match &usage.cache_creation {
        Some(b) => (b.ephemeral_5m_input_tokens, b.ephemeral_1h_input_tokens),
        None => (usage.cache_creation_input_tokens, 0),
    };
    // Dedup key = messageId:requestId, ccusage's uniqueness hash.
    let dedup_key = message
        .id
        .as_ref()
        .map(|mid| format!("{}:{}", mid, raw.request_id.as_deref().unwrap_or("")));

    Some(UsageRecord {
        agent: AGENT.to_string(),
        project: st.project.clone().unwrap_or_else(|| path_project.to_string()),
        session_id: raw.session_id.unwrap_or_else(|| file_session.to_string()),
        timestamp_ms: ts,
        model,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_creation_5m: cache_5m,
        cache_creation_1h: cache_1h,
        cache_read_tokens: usage.cache_read_input_tokens,
        cost_usd: raw.cost_usd,
        dedup_key,
    })
}

/// Subagent transcripts live at `<project>/<session-id>/subagents/*.jsonl`
/// and belong to the parent session.
fn session_id_from_path(path: &Path) -> String {
    let parent = path.parent();
    if parent.and_then(|p| p.file_name()).is_some_and(|n| n == "subagents") {
        if let Some(session) = parent.and_then(|p| p.parent()).and_then(|p| p.file_name()) {
            return session.to_string_lossy().to_string();
        }
    }
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

pub fn parse_timestamp_ms(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// Fallback project name (logs without `cwd`) from the encoded project
/// directory, e.g. "-Users-alice-Documents-MyProject-TokBar" ->
/// "MyProject-TokBar". Simplified port of ccusage project_names.rs.
fn project_name_from_path(path: &Path) -> String {
    // The encoded project dir is the direct child of `projects/`; subagent
    // logs sit two levels deeper (`<session>/subagents/`).
    let project_dir = path
        .ancestors()
        .skip(1)
        .find(|a| {
            a.parent()
                .and_then(|p| p.file_name())
                .is_some_and(|n| n == "projects")
        })
        .or_else(|| path.parent());
    let Some(dir_name) = project_dir
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
    else {
        return "Unknown Project".to_string();
    };
    if dir_name.is_empty() || dir_name == "unknown" {
        return "Unknown Project".to_string();
    }
    let segments: Vec<&str> = dir_name.split('-').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return dir_name;
    }
    // Hex/UUID-looking paths: keep the last two segments.
    let hexish = segments.len() >= 5
        && segments
            .iter()
            .all(|s| s.chars().all(|c| c.is_ascii_hexdigit()));
    if hexish {
        return segments[segments.len() - 2..].join("-");
    }
    // Path-encoded names: take what follows the username for /Users/<u>/ or /home/<u>/.
    let lower: Vec<String> = segments.iter().map(|s| s.to_lowercase()).collect();
    if let Some(idx) = lower.iter().position(|s| s == "users" || s == "home") {
        if segments.len() > idx + 2 {
            let tail = &segments[idx + 2..];
            // Drop common parent folders to keep the meaningful tail.
            let skip = ["documents", "projects", "code", "src", "dev", "work", "repos"];
            let meaningful: Vec<&str> = tail
                .iter()
                .copied()
                .skip_while(|s| skip.contains(&s.to_lowercase().as_str()))
                .collect();
            if !meaningful.is_empty() {
                return meaningful.join("-");
            }
            return tail.join("-");
        }
    }
    dir_name.trim_start_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagent_logs_belong_to_parent_project_and_session() {
        let sub = Path::new(
            "/h/.claude/projects/-Users-a-Documents-MyProject-TokBar/0b7c-1/subagents/agent-a1.jsonl",
        );
        assert_eq!(project_name_from_path(sub), "MyProject-TokBar");
        assert_eq!(session_id_from_path(sub), "0b7c-1");
        let top = Path::new("/h/.claude/projects/-Users-a-Documents-MyProject-TokBar/0b7c-1.jsonl");
        assert_eq!(project_name_from_path(top), "MyProject-TokBar");
        assert_eq!(session_id_from_path(top), "0b7c-1");
    }

    #[test]
    fn project_comes_from_cwd() {
        let mut st = ParseState::default();
        let line = r#"{"cwd":"/Users/a/Documents/独立开发/chaogu","sessionId":"s1","timestamp":"2026-09-20T10:00:00Z","requestId":"r1","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":5,"output_tokens":7,"cache_read_input_tokens":100,"speed":"standard"}}}"#;
        let rec = parse_line(line, &mut st, "file", "fallback").unwrap();
        assert_eq!(rec.project, "chaogu");
        assert_eq!(rec.session_id, "s1");
        assert_eq!(rec.model, "claude-opus-5-5");
        assert_eq!(rec.dedup_key.as_deref(), Some("m1:r1"));
    }
}
