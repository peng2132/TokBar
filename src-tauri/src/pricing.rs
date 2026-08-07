use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

/// Embedded LiteLLM pricing snapshot (filtered to coding-agent models),
/// same data source as ccusage: BerriAI/litellm model_prices_and_context_window.json
const EMBEDDED_PRICING: &str = include_str!("../data/pricing-snapshot.json");

const LITELLM_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// Threshold for tiered (long-context) pricing, same as ccusage.
pub const TIER_THRESHOLD: u64 = 200_000;

/// Fast/priority service-tier cost multipliers, from ccusage
/// fast-multiplier-overrides.json (LiteLLM does not carry these).
const FAST_EXACT: &[(&str, f64)] = &[
    ("gpt-5.5", 2.5),
    ("gpt-5.4", 2.0),
    ("gpt-5.3-codex", 2.0),
];
const FAST_PREFIX: &[(&str, f64)] = &[
    ("claude-opus-4-6", 6.0),
    ("claude-opus-4-7", 6.0),
    ("claude-opus-4-8", 2.0),
];

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct ModelPricing {
    #[serde(default)]
    pub input_cost_per_token: Option<f64>,
    #[serde(default)]
    pub output_cost_per_token: Option<f64>,
    #[serde(default)]
    pub cache_creation_input_token_cost: Option<f64>,
    #[serde(default)]
    pub cache_read_input_token_cost: Option<f64>,
    #[serde(default)]
    pub input_cost_per_token_above_200k_tokens: Option<f64>,
    #[serde(default)]
    pub output_cost_per_token_above_200k_tokens: Option<f64>,
    #[serde(default)]
    pub cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
    #[serde(default)]
    pub cache_read_input_token_cost_above_200k_tokens: Option<f64>,
}

impl ModelPricing {
    pub fn input(&self) -> f64 {
        self.input_cost_per_token.unwrap_or(0.0)
    }
    pub fn output(&self) -> f64 {
        self.output_cost_per_token.unwrap_or(0.0)
    }
    /// ccusage default: cache creation = 1.25x input when not specified.
    pub fn cache_create(&self) -> f64 {
        self.cache_creation_input_token_cost
            .unwrap_or_else(|| self.input() * 1.25)
    }
    /// ccusage default: cache read = 0.1x input when not specified.
    pub fn cache_read(&self) -> f64 {
        self.cache_read_input_token_cost
            .unwrap_or_else(|| self.input() * 0.1)
    }

    /// Scale every rate by a fast-tier multiplier (equivalent to
    /// ccusage's `total * fast_multiplier`).
    fn scaled(mut self, mult: f64) -> Self {
        for rate in [
            &mut self.input_cost_per_token,
            &mut self.output_cost_per_token,
            &mut self.cache_creation_input_token_cost,
            &mut self.cache_read_input_token_cost,
            &mut self.input_cost_per_token_above_200k_tokens,
            &mut self.output_cost_per_token_above_200k_tokens,
            &mut self.cache_creation_input_token_cost_above_200k_tokens,
            &mut self.cache_read_input_token_cost_above_200k_tokens,
        ] {
            if let Some(v) = rate.as_mut() {
                *v *= mult;
            }
        }
        self
    }
}

fn fast_multiplier_for(base_model: &str) -> f64 {
    let normalized = normalize_model(base_model);
    if let Some((_, m)) = FAST_EXACT
        .iter()
        .find(|(k, _)| *k == base_model || normalize_model(k) == normalized)
    {
        return *m;
    }
    if let Some((_, m)) = FAST_PREFIX
        .iter()
        .find(|(k, _)| normalized.starts_with(&normalize_model(k)))
    {
        return *m;
    }
    1.0
}

pub struct PricingMap {
    entries: HashMap<String, ModelPricing>,
}

/// Builds a builtin entry from per-token rates; `None` cache rates fall
/// back to the ccusage defaults (write 1.25x input, read 0.1x input).
const fn rates(
    input: f64,
    output: f64,
    cache_create: Option<f64>,
    cache_read: Option<f64>,
) -> ModelPricing {
    ModelPricing {
        input_cost_per_token: Some(input),
        output_cost_per_token: Some(output),
        cache_creation_input_token_cost: cache_create,
        cache_read_input_token_cost: cache_read,
        input_cost_per_token_above_200k_tokens: None,
        output_cost_per_token_above_200k_tokens: None,
        cache_creation_input_token_cost_above_200k_tokens: None,
        cache_read_input_token_cost_above_200k_tokens: None,
    }
}

/// Built-in prices for models newer than the LiteLLM database, at official
/// vendor list prices. Used only when neither the snapshot nor the online
/// refresh has the model, so LiteLLM wins once it catches up.
///
/// The third-party names below are what Claude Code logs when a provider
/// switcher (cc-switch etc.) points ANTHROPIC_BASE_URL at Kimi / DeepSeek /
/// GLM / MiniMax / StepFun / LongCat — the JSONL `message.model` carries
/// the real model name, so pricing them here is all that's needed.
const BUILTIN_PRICING: &[(&str, ModelPricing)] = &[
    // Official Anthropic pricing.
    ("claude-fable-5", rates(1e-5, 5e-5, Some(1.25e-5), Some(1e-6))),
    // Kimi for-coding plan = k2.6 rates, ccusage parity.
    // https://platform.kimi.ai/docs/pricing/chat-k26
    ("kimi-for-coding", rates(9.5e-7, 4e-6, Some(1.1875e-6), Some(1.6e-7))),
    ("kimi-k2.6", rates(9.5e-7, 4e-6, Some(1.1875e-6), Some(1.6e-7))),
    // Kimi Code CLI logs "kimi-code/k3" in usage.record events. K3 list
    // price: $3/M input, $15/M output, $0.30/M cache-hit input; cache
    // write defaults to 1.25x input (ccusage behavior).
    ("kimi-code/k3", rates(3e-6, 1.5e-5, None, Some(3e-7))),
    // Z.ai GLM-5.1 list price ($1.40 / $4.40 per 1M).
    ("glm-5.1", rates(1.4e-6, 4.4e-6, None, None)),
    // DeepSeek V4 (api-docs.deepseek.com/quick_start/pricing); cache
    // writes carry no surcharge, so write = input rate.
    ("deepseek-v4-pro", rates(4.35e-7, 8.7e-7, Some(4.35e-7), Some(3.625e-9))),
    ("deepseek-v4-flash", rates(1.4e-7, 2.8e-7, Some(1.4e-7), Some(2.8e-9))),
    // MiniMax M2.7 list price ($0.30 / $1.20, cache read $0.059 per 1M).
    ("MiniMax-M2.7", rates(3e-7, 1.2e-6, None, Some(5.9e-8))),
    // StepFun Step-3.5-Flash ($0.10 / $0.30, cache read $0.02 per 1M);
    // also matches dated variants like step-3.5-flash-2603.
    ("step-3.5-flash", rates(1e-7, 3e-7, None, Some(2e-8))),
    // Meituan LongCat hosted list price ($0.20 / $0.80 per 1M).
    ("LongCat-Flash-Chat", rates(2e-7, 8e-7, None, None)),
];

impl PricingMap {
    /// Load embedded snapshot, then overlay a cached online refresh if present.
    pub fn load(cache_dir: Option<PathBuf>) -> Self {
        let mut entries: HashMap<String, ModelPricing> =
            serde_json::from_str(EMBEDDED_PRICING).unwrap_or_default();
        if let Some(dir) = cache_dir {
            let cache_file = dir.join("litellm-pricing.json");
            if let Ok(text) = std::fs::read_to_string(&cache_file) {
                if let Ok(map) = parse_litellm(&text) {
                    entries.extend(map);
                }
            }
        }
        for (model, pricing) in BUILTIN_PRICING {
            entries.entry((*model).to_string()).or_insert(*pricing);
        }
        Self { entries }
    }

    /// Fetch latest LiteLLM pricing and write it to the cache dir.
    /// Returns the number of models on success.
    pub fn refresh_online(cache_dir: &PathBuf) -> Result<usize, String> {
        let body = ureq::get(LITELLM_URL)
            .timeout(std::time::Duration::from_secs(15))
            .call()
            .map_err(|e| e.to_string())?
            .into_string()
            .map_err(|e| e.to_string())?;
        std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
        let subset: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&body)
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|(k, _)| is_relevant_model(k))
                .collect();
        let count = subset.len();
        std::fs::write(
            cache_dir.join("litellm-pricing.json"),
            serde_json::to_string(&subset).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        Ok(count)
    }

    /// Resolve effective pricing for a model. A "-fast" suffix (set by the
    /// adapters for fast/priority service-tier usage) resolves to the base
    /// model's rates scaled by the fast multiplier.
    pub fn resolve(&self, model: &str) -> Option<ModelPricing> {
        if let Some(base) = model.strip_suffix("-fast") {
            return self
                .find(base)
                .map(|p| p.scaled(fast_multiplier_for(base)));
        }
        self.find(model).copied()
    }

    /// Three-tier model matching, ported from ccusage pricing.rs:
    /// exact -> normalized (./@ -> -) -> boundary-aware substring, longest match wins.
    pub fn find(&self, model: &str) -> Option<&ModelPricing> {
        if model.is_empty() || model == "<synthetic>" {
            return None;
        }
        if let Some(p) = self.entries.get(model) {
            return Some(p);
        }
        let normalized = normalize_model(model);
        if let Some(p) = self.entries.get(&normalized) {
            return Some(p);
        }
        let mut best: Option<(&String, &ModelPricing)> = None;
        for (key, pricing) in &self.entries {
            let key_norm = normalize_model(key);
            let matched = contains_at_boundary(model, key)
                || contains_at_boundary(key, model)
                || contains_at_boundary(&normalized, &key_norm)
                || contains_at_boundary(&key_norm, &normalized);
            if matched {
                match best {
                    Some((bk, _)) if bk.len() >= key.len() => {}
                    _ => best = Some((key, pricing)),
                }
            }
        }
        best.map(|(_, p)| p)
    }
}

fn parse_litellm(text: &str) -> Result<HashMap<String, ModelPricing>, serde_json::Error> {
    let raw: serde_json::Map<String, serde_json::Value> = serde_json::from_str(text)?;
    let mut out = HashMap::new();
    for (k, v) in raw {
        if let Ok(p) = serde_json::from_value::<ModelPricing>(v) {
            if p.input_cost_per_token.is_some() || p.output_cost_per_token.is_some() {
                out.insert(k, p);
            }
        }
    }
    Ok(out)
}

fn is_relevant_model(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    [
        "claude", "gpt", "o1", "o3", "o4", "codex", "gemini", "chatgpt", "kimi", "moonshot",
        "minimax", "glm", "qwen", "deepseek", "step", "longcat", "meituan", "z-ai", "zai",
        "zhipu",
    ]
    .iter()
    .any(|p| {
        k.starts_with(p)
            || k.starts_with(&format!("anthropic/{p}"))
            || k.starts_with(&format!("moonshot/{p}"))
            || k.starts_with(&format!("openrouter/{p}"))
    })
}

fn normalize_model(model: &str) -> String {
    model.replace(['.', '@'], "-")
}

/// Substring match where both ends land on non-alphanumeric boundaries,
/// protecting YYYYMMDD date suffixes from being split (ccusage behavior).
fn contains_at_boundary(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    let hay = haystack.as_bytes();
    let mut start = 0usize;
    while let Some(pos) = haystack[start..].find(needle) {
        let idx = start + pos;
        let left_ok = idx == 0 || !hay[idx - 1].is_ascii_alphanumeric();
        let end = idx + needle.len();
        let right_ok = end == hay.len() || !hay[end].is_ascii_alphanumeric();
        if left_ok && right_ok {
            return true;
        }
        start = idx + 1;
        if start >= haystack.len() {
            break;
        }
    }
    false
}
