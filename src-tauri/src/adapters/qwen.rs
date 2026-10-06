//! Qwen Code adapter, ported from ccusage's qwen adapter.
//!
//! Chats live at $QWEN_DATA_DIR (or ~/.qwen) under
//! `projects/{project}/chats/*.jsonl`; assistant lines carry a Gemini
//! style `usageMetadata` block.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::util;
use crate::types::UsageRecord;

pub const AGENT: &str = "qwen";

pub fn data_dirs() -> Vec<PathBuf> {
    util::env_dirs("QWEN_DATA_DIR", ".qwen")
        .into_iter()
        .map(|b| b.join("projects"))
        .filter(|p| p.is_dir())
        .collect()
}

pub fn collect_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in data_dirs() {
        util::collect_with_ext(&root, &["jsonl"], &mut files);
    }
    // Keep only projects/{project}/chats/{file}.jsonl shaped paths.
    files.retain(|f| {
        f.parent()
            .and_then(|p| p.file_name())
            .is_some_and(|n| n == "chats")
    });
    files.sort();
    files
}

/// `projects/{project}/chats/{file}` -> project.
fn project_from_path(path: &Path) -> Option<String> {
    let chats = path.parent()?;
    let project = chats.parent()?;
    if project.parent()?.file_name()? == "projects" {
        Some(project.file_name()?.to_string_lossy().to_string())
    } else {
        None
    }
}

/// Normalize a qwen-code `usageMetadata` block into
/// (input without cache, billable output, cache read).
///
/// qwen-code converts every provider into Gemini's shape (see its
/// `converter.ts` / `buildAnthropicUsageMetadata`):
/// - `promptTokenCount` is the whole prompt *including* cached tokens
///   (on the Anthropic path it is input + cache read + cache write), and
///   `cachedContentTokenCount` is the cache-read part of it, so the cached
///   portion is subtracted from input instead of being billed twice.
/// - On the OpenAI path `candidatesTokenCount` is `completion_tokens`,
///   which already includes reasoning, and `thoughtsTokenCount` is that
///   reasoning subset; `totalTokenCount` = prompt + completion. On the
///   Gemini-native path candidates exclude thoughts and the total adds
///   them on top. Thoughts are therefore billed only for the part of the
///   total not already covered by prompt + candidates.
///
/// Deviation from ccusage, which bills prompt + cached + thoughts as-is.
fn split_usage(usage: &Value) -> Option<(u64, u64, u64)> {
    let prompt = util::get_u64(usage, "promptTokenCount");
    let candidates = util::get_u64(usage, "candidatesTokenCount");
    let thoughts = util::get_u64(usage, "thoughtsTokenCount");
    let cached = util::get_u64(usage, "cachedContentTokenCount");
    let total = util::get_u64(usage, "totalTokenCount");

    let input = prompt - cached.min(prompt);
    let reasoning_beyond_candidates =
        thoughts.min(total.saturating_sub(prompt.saturating_add(candidates)));
    let mut output = candidates + reasoning_beyond_candidates;
    if input == 0 && output == 0 {
        // Total-only blocks: qwen-code omits the prompt/candidates
        // breakdown when the provider reports only `total_tokens`.
        output = total.saturating_sub(cached);
    }
    (input > 0 || output > 0 || cached > 0).then_some((input, output, cached))
}

pub fn parse_file(path: &Path) -> Vec<UsageRecord> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let project = project_from_path(path).unwrap_or_else(|| "qwen".to_string());
    let file_stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let fallback_ts = util::mtime_ms(path);

    let mut records = Vec::new();
    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = value.get("usageMetadata") else {
            continue;
        };
        let Some((input, output, cache_read)) = split_usage(usage) else {
            continue;
        };
        let ts = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(util::parse_rfc3339_ms)
            .unwrap_or(fallback_ts);
        let model = util::get_str(&value, &["model"]).unwrap_or("unknown");
        let session_id = util::get_str(&value, &["sessionId"])
            .map(str::to_string)
            .unwrap_or_else(|| format!("{project}-{file_stem}"));
        let dedup_key = format!(
            "qwen:{session_id}:{ts}:{model}:{input}:{output}:{cache_read}"
        );
        records.push(UsageRecord {
            agent: AGENT.to_string(),
            project: project.clone(),
            session_id,
            timestamp_ms: ts,
            model: model.to_string(),
            input_tokens: input,
            output_tokens: output,
            cache_creation_5m: 0,
            cache_creation_1h: 0,
            cache_read_tokens: cache_read,
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

    #[test]
    fn openai_shape_subtracts_cache_and_keeps_reasoning_inside_completion() {
        // OpenAI path: prompt incl. cached, candidates = completion incl.
        // reasoning, total = prompt + completion.
        let usage = json!({
            "promptTokenCount": 1000,
            "candidatesTokenCount": 300,
            "thoughtsTokenCount": 120,
            "cachedContentTokenCount": 400,
            "totalTokenCount": 1300
        });
        assert_eq!(split_usage(&usage), Some((600, 300, 400)));
    }

    #[test]
    fn gemini_native_shape_adds_thoughts_on_top_of_candidates() {
        // Gemini path: candidates exclude thoughts, total adds them.
        let usage = json!({
            "promptTokenCount": 1000,
            "candidatesTokenCount": 300,
            "thoughtsTokenCount": 120,
            "cachedContentTokenCount": 400,
            "totalTokenCount": 1420
        });
        assert_eq!(split_usage(&usage), Some((600, 420, 400)));
    }

    #[test]
    fn anthropic_shape_prompt_includes_cache_read() {
        // buildAnthropicUsageMetadata: prompt = input + read + write.
        let usage = json!({
            "promptTokenCount": 50 + 800 + 100,
            "candidatesTokenCount": 40,
            "cachedContentTokenCount": 800,
            "totalTokenCount": 990
        });
        assert_eq!(split_usage(&usage), Some((150, 40, 800)));
    }

    #[test]
    fn cached_larger_than_prompt_never_underflows() {
        let usage = json!({"promptTokenCount": 10, "cachedContentTokenCount": 25, "candidatesTokenCount": 1});
        assert_eq!(split_usage(&usage), Some((0, 1, 25)));
    }

    #[test]
    fn total_only_and_empty_blocks() {
        assert_eq!(split_usage(&json!({"totalTokenCount": 77})), Some((0, 77, 0)));
        assert_eq!(split_usage(&json!({})), None);
    }
}
