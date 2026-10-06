//! Codebuff adapter, ported from ccusage's codebuff adapter.
//!
//! Chats live under ~/.config/manicode{,-dev,-staging}/projects (or
//! $CODEBUFF_DATA_DIR) as `{project}/chats/{chat-id}/chat-messages.json`
//! arrays. Usage hides in up to three metadata layers which are merged
//! field-by-field, earlier layers winning.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapters::util;
use crate::types::UsageRecord;

pub const AGENT: &str = "codebuff";

static NULL: Value = Value::Null;

pub fn data_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(raw) = std::env::var("CODEBUFF_DATA_DIR") {
        for p in util::split_env_paths(&raw) {
            if p.file_name().is_some_and(|n| n == "projects") {
                dirs.push(p);
            } else {
                dirs.push(p.join("projects"));
            }
        }
    } else if let Some(home) = dirs::home_dir() {
        for channel in ["manicode", "manicode-dev", "manicode-staging"] {
            dirs.push(home.join(".config").join(channel).join("projects"));
        }
    }
    dirs.retain(|p| p.is_dir());
    dirs.sort();
    dirs.dedup();
    dirs
}

pub fn collect_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in data_dirs() {
        util::walk_files(
            &root,
            &|p| p.file_name().is_some_and(|n| n == "chat-messages.json"),
            &mut files,
        );
    }
    files.sort();
    files.dedup();
    files
}

fn is_assistant(msg: &Value) -> bool {
    let role = util::get_str(msg, &["variant"]).or_else(|| util::get_str(msg, &["role"]));
    matches!(role, Some("ai" | "agent" | "assistant"))
}

#[derive(Default, Clone, Copy)]
struct Usage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
    total: u64,
}

fn parse_usage_object(v: &Value) -> Usage {
    let pick = |keys: &[&str]| -> u64 {
        keys.iter()
            .filter_map(|k| {
                // Support one level of dotted nesting (promptTokensDetails.cachedTokens).
                let mut cur = v;
                for part in k.split('.') {
                    cur = cur.get(part)?;
                }
                Some(util::as_u64(cur))
            })
            .max()
            .unwrap_or(0)
    };
    let input = pick(&["inputTokens", "input_tokens", "promptTokens", "prompt_tokens"]);
    // Anthropic-shape cache reads are disjoint from input; OpenAI-shape
    // `prompt_tokens_details.cached_tokens` is a *subset* of
    // `prompt_tokens`, so when the cache count comes from there it is
    // subtracted from input to avoid billing those tokens twice.
    // Deviation from ccusage, which keeps input as reported.
    let anthropic_cache_read = pick(&["cacheReadInputTokens", "cache_read_input_tokens"]);
    let openai_cached = pick(&[
        "promptTokensDetails.cachedTokens",
        "prompt_tokens_details.cached_tokens",
    ]);
    let cache_read = anthropic_cache_read.max(openai_cached);
    let input = if openai_cached > 0 && openai_cached >= anthropic_cache_read {
        input - openai_cached.min(input)
    } else {
        input
    };
    Usage {
        input,
        output: pick(&[
            "outputTokens",
            "output_tokens",
            "completionTokens",
            "completion_tokens",
        ]),
        cache_read,
        cache_creation: pick(&[
            "cacheCreationInputTokens",
            "cache_creation_input_tokens",
            "cacheCreationTokens",
            "cache_creation_tokens",
            "cachedTokensCreated",
            "cached_tokens_created",
        ]),
        total: pick(&["totalTokens", "total_tokens", "total"]),
    }
}

fn merge(base: Usage, fallback: Usage) -> Usage {
    Usage {
        input: if base.input > 0 { base.input } else { fallback.input },
        output: if base.output > 0 { base.output } else { fallback.output },
        cache_read: if base.cache_read > 0 { base.cache_read } else { fallback.cache_read },
        cache_creation: if base.cache_creation > 0 {
            base.cache_creation
        } else {
            fallback.cache_creation
        },
        total: if base.total > 0 { base.total } else { fallback.total },
    }
}

/// Deep fallback: the last assistant entry of
/// metadata.runState.sessionState.mainAgentState.messageHistory carries
/// providerOptions with usage.
fn run_state_usage(metadata: &Value) -> (Usage, Option<String>) {
    let history = metadata
        .pointer("/runState/sessionState/mainAgentState/messageHistory")
        .and_then(Value::as_array);
    let Some(history) = history else {
        return (Usage::default(), None);
    };
    for msg in history.iter().rev() {
        if !is_assistant(msg) {
            continue;
        }
        if let Some(opts) = msg.get("providerOptions") {
            let usage = opts
                .get("usage")
                .map(parse_usage_object)
                .unwrap_or_default();
            let model = util::get_str(opts, &["model"]).map(str::to_string);
            return (usage, model);
        }
    }
    (Usage::default(), None)
}

/// chat ids look like "2026-01-02T03-04-05.678Z": two dashes in the time
/// part stand in for colons.
fn ts_from_chat_id(chat_id: &str) -> Option<i64> {
    // `split_once` keeps the split on a char boundary even for
    // non-ASCII directory names (ccusage `parse_codebuff_chat_id_timestamp`).
    let (date, time) = chat_id.split_once('T')?;
    let fixed = format!("{date}T{}", time.replacen('-', ":", 2));
    util::parse_rfc3339_ms(&fixed)
}

pub fn parse_file(path: &Path) -> Vec<UsageRecord> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(Value::Array(messages)) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };

    // sessions are {channel}/{project}/{chat_id} from the path:
    // .config/{channel}/projects/{project}/chats/{chat_id}/chat-messages.json
    let chat_id = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let project = path
        .ancestors()
        .nth(3)
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let channel = path
        .ancestors()
        .nth(5)
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "manicode".to_string());
    let session_id = format!("{channel}/{project}/{chat_id}");
    let fallback_ts = ts_from_chat_id(&chat_id).unwrap_or_else(|| util::mtime_ms(path));

    let mut records = Vec::new();
    for (ordinal, msg) in messages.iter().enumerate() {
        if !is_assistant(msg) {
            continue;
        }
        // Borrow instead of cloning: `metadata` can embed the whole run
        // state (message history) and is only read here.
        let metadata = msg.get("metadata").unwrap_or(&NULL);
        let direct = metadata.get("usage").map(parse_usage_object).unwrap_or_default();
        let nested = metadata
            .pointer("/codebuff/usage")
            .map(parse_usage_object)
            .unwrap_or_default();
        let (deep, deep_model) = run_state_usage(metadata);
        let usage = merge(merge(direct, nested), deep);

        let input = usage.input;
        let mut output = usage.output;
        if input == 0 && output == 0 && usage.cache_read == 0 && usage.cache_creation == 0 {
            if usage.total == 0 {
                continue;
            }
            output = usage.total;
        }

        let ts = msg
            .get("timestamp")
            .or_else(|| msg.get("createdAt"))
            .or_else(|| metadata.get("timestamp"))
            .and_then(util::ts_from_value)
            .unwrap_or(fallback_ts);
        let model = util::get_str(metadata, &["model"])
            .map(str::to_string)
            .or_else(|| metadata.pointer("/codebuff/model").and_then(Value::as_str).map(str::to_string))
            .or(deep_model)
            .unwrap_or_else(|| "codebuff-unknown".to_string());
        let dedup_key = match util::get_str(msg, &["id"]) {
            Some(id) => format!("codebuff:{session_id}:{id}"),
            None => format!(
                "codebuff:{session_id}:{ts}:{model}:{ordinal}:{input}:{output}"
            ),
        };
        records.push(UsageRecord {
            agent: AGENT.to_string(),
            project: "Codebuff".to_string(),
            session_id: session_id.clone(),
            timestamp_ms: ts,
            model,
            input_tokens: input,
            output_tokens: output,
            cache_creation_5m: usage.cache_creation,
            cache_creation_1h: 0,
            cache_read_tokens: usage.cache_read,
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
    fn openai_cached_tokens_are_subtracted_from_prompt() {
        for usage in [
            json!({"promptTokens": 1000, "completionTokens": 50,
                   "promptTokensDetails": {"cachedTokens": 400}}),
            json!({"prompt_tokens": 1000, "completion_tokens": 50,
                   "prompt_tokens_details": {"cached_tokens": 400}}),
        ] {
            let u = parse_usage_object(&usage);
            assert_eq!((u.input, u.output, u.cache_read), (600, 50, 400));
        }
    }

    #[test]
    fn anthropic_cache_reads_stay_disjoint_from_input() {
        let u = parse_usage_object(&json!({
            "inputTokens": 100, "outputTokens": 50,
            "cacheReadInputTokens": 400, "cacheCreationInputTokens": 30
        }));
        assert_eq!((u.input, u.cache_read, u.cache_creation), (100, 400, 30));
    }

    #[test]
    fn cached_above_prompt_does_not_underflow() {
        let u = parse_usage_object(&json!({"promptTokens": 10, "promptTokensDetails": {"cachedTokens": 40}}));
        assert_eq!((u.input, u.cache_read), (0, 40));
    }

    #[test]
    fn chat_id_timestamps_and_non_ascii_ids() {
        assert_eq!(
            ts_from_chat_id("2026-01-02T03-04-05.678Z"),
            util::parse_rfc3339_ms("2026-01-02T03:04:05.678Z")
        );
        assert_eq!(ts_from_chat_id("日本語のチャットT名前"), None);
        assert_eq!(ts_from_chat_id("短い"), None);
    }
}
