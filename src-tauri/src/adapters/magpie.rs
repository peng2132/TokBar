use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::adapters::{claude::parse_timestamp_ms, jsonl, AppendParse, RequestPricing};
use crate::performance::{output_rate, PerformanceSample};
use crate::pricing::ServiceTier;

pub const SOURCE: &str = "magpie";

pub fn data_dirs() -> Vec<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .map(|base| base.join("magpie"))
        .filter(|path| path.is_dir())
        .into_iter()
        .collect()
}

pub fn collect_files() -> Vec<PathBuf> {
    data_dirs()
        .iter()
        .map(|dir| dir.join("usage.jsonl"))
        .filter(|path| path.is_file())
        .collect()
}

/// The gateway supplies timing and the tier used for a matching Codex response.
pub fn parse_append(path: &Path, offset: u64, _state: Option<&str>) -> AppendParse {
    let mut performance = Vec::new();
    let mut request_pricing = Vec::new();
    let end = jsonl::scan_lines(
        path,
        offset,
        &["\"ttft_ms\"", "\"tier_served\"", "\"tier_sent\""],
        |line, complete, _| {
            if complete {
                if let Ok(call) = serde_json::from_str::<Call>(line) {
                    performance.extend(call.performance_sample());
                    request_pricing.extend(call.request_tier());
                }
            }
        },
    );
    AppendParse {
        records: Vec::new(),
        performance,
        request_pricing,
        offset: end.unwrap_or(offset),
        state: None,
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Call {
    t: String,
    model: String,
    provider: String,
    agent: String,
    rid: String,
    response_id: String,
    tier_served: Option<String>,
    tier_sent: Option<String>,
    status: u16,
    err: String,
    handed_on: bool,
    kind: String,
    operation: String,
    source: String,
    out: u64,
    reasoning: u64,
    reasoning_hidden: bool,
    ms: i64,
    ttft_ms: i64,
    first_text_ms: i64,
    first_reasoning_ms: i64,
    flow_ms: i64,
}

impl Call {
    fn completed(&self) -> bool {
        (200..300).contains(&self.status) && self.err.is_empty() && !self.handed_on
    }

    fn request_tier(&self) -> Option<RequestPricing> {
        if self.agent != super::codex::AGENT || self.response_id.trim().is_empty() || !self.completed() {
            return None;
        }
        let actual = self.tier_served.as_deref().and_then(ServiceTier::parse);
        let tier = actual.or_else(|| self.tier_sent.as_deref().and_then(ServiceTier::parse))?;
        Some(RequestPricing::MagpieTier {
            response_id: self.response_id.clone(),
            started_ms: parse_timestamp_ms(&self.t)?,
            tier,
            returned: actual.is_some(),
        })
    }

    fn performance_sample(&self) -> Option<PerformanceSample> {
        if !self.completed()
            || self.model.is_empty()
            || self.out == 0
            || !self.kind.is_empty()
            || !self.operation.is_empty()
            || !self.source.is_empty()
            || !(1..=self.ms).contains(&self.ttft_ms)
        {
            return None;
        }
        Some(PerformanceSample {
            key: format!("{}:{}:{}:{}", self.t, self.provider, self.model, self.rid),
            timestamp_ms: parse_timestamp_ms(&self.t)?,
            output_tps: self.output_tps(),
            model: self.model.clone(),
            provider: self.provider.clone(),
            first_token_ms: Some(self.ttft_ms),
        })
    }

    fn output_tps(&self) -> Option<f64> {
        // Follow Magpie's distinction between streamed reasoning and reasoning
        // done before visible text. The latter cannot share the text's window.
        let first_text = (self.ttft_ms..self.ms).contains(&self.first_text_ms);
        let first_reasoning = (1..self.ms).contains(&self.first_reasoning_ms);
        let (tokens, start) = if !first_reasoning && (self.reasoning > 0 || self.reasoning_hidden) {
            if !first_text || self.reasoning == 0 {
                return None;
            }
            (self.out.checked_sub(self.reasoning)?, self.first_text_ms)
        } else if first_reasoning {
            (self.out, self.ttft_ms.min(self.first_reasoning_ms))
        } else {
            (self.out, self.ttft_ms)
        };
        let window = self.ms - start;
        let flow_window = self.ms
            - if first_text {
                self.first_text_ms
            } else {
                start
            };
        if (self.flow_ms > 0 && self.flow_ms.saturating_mul(4) < flow_window)
            || (window < 1000 && start > window * 4)
        {
            return None;
        }
        output_rate(tokens, window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_line(line: &str) -> Option<PerformanceSample> {
        serde_json::from_str::<Call>(line).ok()?.performance_sample()
    }

    #[test]
    fn times_the_stream_after_first_token_and_drops_bursts_and_failures() {
        let line = r#"{"t":"2026-11-24T12:00:00Z","model":"model-a","provider":"provider-a",
            "status":200,"out":450,"ms":3500,"ttft_ms":500}"#;
        let normal = parse_line(line).unwrap();
        assert_eq!(normal.output_tps, Some(150.0));
        assert_eq!(normal.first_token_ms, Some(500));

        let burst = line.replace("\"out\":450", "\"out\":450,\"flow_ms\":20");
        assert_eq!(parse_line(&burst).unwrap().output_tps, None);
        assert!(parse_line(&line.replace("200", "429")).is_none());
    }

    #[test]
    fn measures_visible_output_when_reasoning_was_hidden_and_counted() {
        let line = r#"{"t":"2026-11-24T12:00:00Z","model":"model-a","status":200,
            "out":450,"ms":3500,"ttft_ms":500,"first_text_ms":2500,"reasoning":250}"#;
        let sample = parse_line(line).unwrap();
        assert_eq!(sample.output_tps, Some(200.0));
        assert_eq!(sample.first_token_ms, Some(500));
        let hidden = line.replace("\"reasoning\":250", "\"reasoning_hidden\":true");
        assert_eq!(parse_line(&hidden).unwrap().output_tps, None);
    }
}
