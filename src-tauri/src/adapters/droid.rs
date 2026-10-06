//! Droid (Factory) adapter, ported from ccusage's droid adapter.
//!
//! Each session under $DROID_SESSIONS_DIR (or ~/.factory/sessions)
//! keeps a `{session}.settings.json` with cumulative `tokenUsage`
//! totals. Snapshots grow over time; the per-session dedup key plus
//! TokBar's "more tokens wins" conflict rule keeps the latest one.

use std::io::BufRead;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::util;
use crate::types::UsageRecord;

pub const AGENT: &str = "droid";

pub fn data_dirs() -> Vec<PathBuf> {
    util::env_dirs("DROID_SESSIONS_DIR", ".factory/sessions")
}

pub fn collect_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in data_dirs() {
        util::walk_files(
            &dir,
            &|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".settings.json"))
            },
            &mut files,
        );
    }
    files.sort();
    files
}

/// `custom:` prefix and `[...]` suffix stripped, lowercased, dots and
/// runs of separators collapsed to single dashes.
fn normalize_model(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    if let Some(rest) = s.strip_prefix("custom:") {
        s = rest.to_string();
    }
    if let Some(open) = s.find('[') {
        s.truncate(open);
    }
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.trim().to_lowercase().chars() {
        let mapped = if ch == '.' || ch.is_whitespace() || ch == '-' { '-' } else { ch };
        if mapped == '-' {
            if !prev_dash && !out.is_empty() {
                out.push('-');
            }
            prev_dash = true;
        } else {
            out.push(mapped);
            prev_dash = false;
        }
    }
    out.trim_end_matches('-').to_string()
}

fn default_model_for_provider(provider: &str) -> &'static str {
    let p = provider.trim().to_lowercase().replace('-', "_");
    match p.as_str() {
        "" | "claude" | "anthropic" => "claude-unknown",
        "openai" => "gpt-unknown",
        "google" | "google_ai" | "gemini" | "vertex" | "vertex_ai" => "gemini-unknown",
        "xai" | "x_ai" | "grok" => "grok-unknown",
        _ => "unknown",
    }
}

/// Fallback: scan the sibling `{session}.jsonl` for a "Model: ..." line
/// (first 500 lines, ccusage behavior).
fn model_from_sibling_jsonl(settings_path: &Path) -> Option<String> {
    let name = settings_path.file_name()?.to_str()?;
    let prefix = name.strip_suffix(".settings.json")?;
    let jsonl = settings_path.with_file_name(format!("{prefix}.jsonl"));
    // Read lazily: the transcript can be large and only its head is
    // scanned, so stream lines instead of loading the whole file.
    let file = std::fs::File::open(jsonl).ok()?;
    std::io::BufReader::new(file)
        .lines()
        .take(500)
        .map_while(Result::ok)
        .find_map(|line| model_from_line(&line))
}

fn model_from_line(line: &str) -> Option<String> {
    let pos = line.find("Model: ")?;
    // "Model: " is ASCII, so `pos + 7` is a char boundary.
    let rest = line.get(pos + 7..)?;
    let model = rest
        .split(['"', '\\', ','])
        .next()
        .unwrap_or("")
        .trim();
    let normalized = normalize_model(model);
    (!normalized.is_empty()).then_some(normalized)
}

pub fn parse_file(path: &Path) -> Vec<UsageRecord> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };
    let Some(usage) = value.get("tokenUsage") else {
        return Vec::new();
    };
    let input = util::get_u64(usage, "inputTokens");
    let mut output = util::get_u64(usage, "outputTokens");
    let cache_creation = util::get_u64(usage, "cacheCreationTokens");
    let cache_read = util::get_u64(usage, "cacheReadTokens");
    // Thinking tokens bill as output (ccusage behavior).
    output += util::get_u64(usage, "thinkingTokens");
    if input == 0 && output == 0 && cache_creation == 0 && cache_read == 0 {
        let total = util::get_u64(usage, "totalTokens");
        if total == 0 {
            return Vec::new();
        }
        output = total;
    }

    let provider = util::get_str(&value, &["providerLock"]).unwrap_or("");
    let model = util::get_str(&value, &["model"])
        .map(normalize_model)
        .filter(|m| !m.is_empty())
        .or_else(|| model_from_sibling_jsonl(path))
        .unwrap_or_else(|| default_model_for_provider(provider).to_string());

    let ts = util::get_str(&value, &["providerLockTimestamp"])
        .and_then(util::parse_rfc3339_ms)
        .unwrap_or_else(|| util::mtime_ms(path));
    let session_id = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".settings.json"))
        .unwrap_or("unknown")
        .to_string();

    vec![UsageRecord {
        agent: AGENT.to_string(),
        project: "Droid".to_string(),
        session_id: session_id.clone(),
        timestamp_ms: ts,
        model,
        input_tokens: input,
        output_tokens: output,
        cache_creation_5m: cache_creation,
        cache_creation_1h: 0,
        cache_read_tokens: cache_read,
        cost_usd: None,
        // Cumulative session snapshot: one row per session, latest wins.
        dedup_key: Some(format!("droid:{session_id}")),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_model_is_read_from_the_first_500_lines_only() {
        let root = util::test_dir("droid");
        let settings = root.join("session-b.settings.json");
        std::fs::write(&settings, r#"{"tokenUsage":{"inputTokens":1,"outputTokens":2}}"#).unwrap();

        let mut lines: Vec<String> = (0..2).map(|i| format!("{{\"n\":{i}}}")).collect();
        lines.push(r#"{"content":"Model: Claude Opus 4.5 Thinking [Anthropic]"}"#.to_string());
        std::fs::write(root.join("session-b.jsonl"), lines.join("\n")).unwrap();
        assert_eq!(
            model_from_sibling_jsonl(&settings).as_deref(),
            Some("claude-opus-4-5-thinking")
        );
        let records = parse_file(&settings);
        assert_eq!(records[0].model, "claude-opus-4-5-thinking");

        let mut late: Vec<String> = (0..500).map(|i| format!("{{\"n\":{i}}}")).collect();
        late.push(r#"{"content":"Model: gpt-5"}"#.to_string());
        std::fs::write(root.join("session-b.jsonl"), late.join("\n")).unwrap();
        assert_eq!(model_from_sibling_jsonl(&settings), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn model_line_parsing_handles_non_ascii() {
        assert_eq!(model_from_line(r#"{"c":"Model: gpt-5.1, more"}"#).as_deref(), Some("gpt-5-1"));
        assert_eq!(model_from_line("日本語 Model: 模型"), Some("模型".to_string()));
        assert_eq!(model_from_line("no model here"), None);
        assert_eq!(model_from_line("Model: "), None);
    }
}
