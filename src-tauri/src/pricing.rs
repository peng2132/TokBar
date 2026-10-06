use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value};

/// Embedded LiteLLM pricing snapshot (filtered to coding-agent models and
/// pricing fields), same data source as ccusage:
/// BerriAI/litellm model_prices_and_context_window.json
const EMBEDDED_PRICING: &str = include_str!("../data/pricing-snapshot.json");

/// LiteLLM commit date of the embedded snapshot. Regenerate the snapshot
/// with `data/update_snapshot.py` for every release.
pub const SNAPSHOT_DATE: &str = "2026-10-06";

/// Online sources, tried in order. raw.githubusercontent.com is often
/// unreachable from mainland China; jsDelivr mirrors the same file.
const PRICING_URLS: &[&str] = &[
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json",
    "https://cdn.jsdelivr.net/gh/BerriAI/litellm@main/model_prices_and_context_window.json",
];

const CACHE_FILE: &str = "litellm-pricing.json";

/// Bump whenever the builtin tables below or the cost semantics change:
/// it feeds the pricing fingerprint, so stored costs get re-priced.
const RULES_VERSION: u32 = 2;

/// Per-token rates for one pricing tier. `None` = not published; the
/// effective rate is then derived (see [`ModelPricing::effective`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rates {
    pub input: Option<f64>,
    pub output: Option<f64>,
    /// 5-minute cache write (also used when a log has no 5m/1h breakdown).
    pub cache_write_5m: Option<f64>,
    pub cache_write_1h: Option<f64>,
    pub cache_read: Option<f64>,
}

impl Rates {
    fn is_empty(&self) -> bool {
        *self == Rates::default()
    }

    fn scaled(mut self, mult: f64) -> Self {
        for rate in [
            &mut self.input,
            &mut self.output,
            &mut self.cache_write_5m,
            &mut self.cache_write_1h,
            &mut self.cache_read,
        ] {
            if let Some(v) = rate.as_mut() {
                *v *= mult;
            }
        }
        self
    }

    /// Field-wise `self` with gaps filled from `other`.
    fn or(self, other: Rates) -> Rates {
        Rates {
            input: self.input.or(other.input),
            output: self.output.or(other.output),
            cache_write_5m: self.cache_write_5m.or(other.cache_write_5m),
            cache_write_1h: self.cache_write_1h.or(other.cache_write_1h),
            cache_read: self.cache_read.or(other.cache_read),
        }
    }
}

/// Fully resolved per-token rates (every gap filled).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectiveRates {
    pub input: f64,
    pub output: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ModelPricing {
    pub base: Rates,
    /// Long-context tier. When a request's prompt (input + cache writes +
    /// cache reads) exceeds `tier_threshold` tokens, the whole request is
    /// billed at these rates: Anthropic above 200K, OpenAI above 272K and
    /// MiniMax above 512K all bill the entire request, not just the
    /// overflow. 0 = no tier.
    pub tier_threshold: u64,
    pub above: Rates,
    /// Vendor-published fast/priority service-tier rates (LiteLLM
    /// `*_priority`), standard and long-context.
    pub priority: Rates,
    pub priority_above: Rates,
}

impl ModelPricing {
    /// Effective rates for a request with `prompt_tokens` of prompt.
    /// Unpublished cache rates default to the Anthropic-style ratios
    /// ccusage uses: 5m write 1.25x input, 1h write 2x, read 0.1x.
    pub fn effective(&self, prompt_tokens: u64) -> EffectiveRates {
        let input = self.base.input.unwrap_or(0.0);
        let base = EffectiveRates {
            input,
            output: self.base.output.unwrap_or(0.0),
            cache_write_5m: self.base.cache_write_5m.unwrap_or(input * 1.25),
            cache_write_1h: self.base.cache_write_1h.unwrap_or(input * 2.0),
            cache_read: self.base.cache_read.unwrap_or(input * 0.1),
        };
        if self.tier_threshold == 0 || prompt_tokens <= self.tier_threshold || self.above.is_empty()
        {
            return base;
        }
        let a = &self.above;
        let derived = |ratio: f64| a.input.map(|i| i * ratio);
        EffectiveRates {
            input: a.input.unwrap_or(base.input),
            output: a.output.unwrap_or(base.output),
            cache_write_5m: a
                .cache_write_5m
                .or_else(|| derived(1.25))
                .unwrap_or(base.cache_write_5m),
            cache_write_1h: a
                .cache_write_1h
                .or_else(|| derived(2.0))
                .unwrap_or(base.cache_write_1h),
            cache_read: a.cache_read.or_else(|| derived(0.1)).unwrap_or(base.cache_read),
        }
    }

    /// Scale every published rate (time-of-day discounts).
    fn scaled(self, mult: f64) -> Self {
        Self {
            base: self.base.scaled(mult),
            above: self.above.scaled(mult),
            priority: self.priority.scaled(mult),
            priority_above: self.priority_above.scaled(mult),
            ..self
        }
    }

    /// The fast/priority tier as a standard pricing. Vendor-published
    /// priority rates win; anything unpublished is the standard rate
    /// times the vendor's ratio (priority input / input) when known, else
    /// `fallback_mult`.
    fn fast(self, fallback_mult: f64) -> Self {
        let vendor_mult = match (self.priority.input, self.base.input) {
            (Some(p), Some(b)) if b > 0.0 => Some(p / b),
            _ => None,
        };
        let m = vendor_mult.unwrap_or(fallback_mult);
        Self {
            base: self.priority.or(self.base.scaled(m)),
            above: self.priority_above.or(self.above.scaled(m)),
            tier_threshold: self.tier_threshold,
            priority: Rates::default(),
            priority_above: Rates::default(),
        }
    }

    fn is_free(&self) -> bool {
        self.base.input.unwrap_or(0.0) == 0.0 && self.base.output.unwrap_or(0.0) == 0.0
    }

    fn hash_into(&self, h: &mut impl Hasher) {
        self.tier_threshold.hash(h);
        for r in [&self.base, &self.above, &self.priority, &self.priority_above] {
            for v in [r.input, r.output, r.cache_write_5m, r.cache_write_1h, r.cache_read] {
                v.map(f64::to_bits).hash(h);
            }
        }
    }
}

/// Fast/priority multipliers for models whose vendor data does not carry
/// `*_priority` rates. OpenAI bills Fast at a flat 2x of the matching
/// standard rate (the default below) except GPT-5.5 at 2.5x; Claude fast
/// mode is 6x on Opus 4.6/4.7 and 2x from Opus 4.8 on.
const FAST_EXACT: &[(&str, f64)] = &[("gpt-5.5", 2.5)];
const FAST_PREFIX: &[(&str, f64)] = &[
    ("claude-opus-4-6", 6.0),
    ("claude-opus-4-7", 6.0),
    ("claude-opus-4-8", 2.0),
    ("claude-opus-5", 2.0),
];
const FAST_DEFAULT: f64 = 2.0;

fn fast_multiplier_for(base_model: &str) -> f64 {
    let c = canonical(base_model);
    if let Some((_, m)) = FAST_EXACT.iter().find(|(k, _)| canonical(k) == c) {
        return *m;
    }
    if let Some((_, m)) = FAST_PREFIX.iter().find(|(k, _)| c.starts_with(&canonical(k))) {
        return *m;
    }
    FAST_DEFAULT
}

/// Builds a standard-tier pricing from $/1M list prices; `None` cache
/// rates fall back to the derived defaults.
const fn per_m(
    input: f64,
    output: f64,
    cache_write_5m: Option<f64>,
    cache_read: Option<f64>,
) -> ModelPricing {
    ModelPricing {
        base: Rates {
            input: Some(input / 1e6),
            output: Some(output / 1e6),
            cache_write_5m: match cache_write_5m {
                Some(v) => Some(v / 1e6),
                None => None,
            },
            cache_write_1h: None,
            cache_read: match cache_read {
                Some(v) => Some(v / 1e6),
                None => None,
            },
        },
        tier_threshold: 0,
        above: Rates {
            input: None,
            output: None,
            cache_write_5m: None,
            cache_write_1h: None,
            cache_read: None,
        },
        priority: Rates {
            input: None,
            output: None,
            cache_write_5m: None,
            cache_write_1h: None,
            cache_read: None,
        },
        priority_above: Rates {
            input: None,
            output: None,
            cache_write_5m: None,
            cache_write_1h: None,
            cache_read: None,
        },
    }
}

/// Adds a long-context tier (whole request above `threshold` tokens) to a
/// [`per_m`] pricing, in $/1M.
const fn tiered(
    mut p: ModelPricing,
    threshold: u64,
    input: f64,
    output: f64,
    cache_write_5m: Option<f64>,
    cache_read: Option<f64>,
) -> ModelPricing {
    let above = per_m(input, output, cache_write_5m, cache_read);
    p.tier_threshold = threshold;
    p.above = above.base;
    p
}

/// Built-in prices at official vendor list prices (verified 2026-10-06).
/// Used only when neither the snapshot nor the online refresh has the
/// model, so LiteLLM wins once it catches up.
///
/// The third-party names are what Claude Code logs when a provider
/// switcher (cc-switch etc.) points ANTHROPIC_BASE_URL at Kimi / DeepSeek /
/// GLM / MiniMax / StepFun / LongCat / Qwen — the JSONL `message.model`
/// carries the real model name, so pricing them here is all that's needed.
const BUILTIN_PRICING: &[(&str, ModelPricing)] = &[
    // Anthropic (platform.claude.com/docs/en/about-claude/pricing). No
    // long-context premium on 4.6+; Fable/Mythos 5.1 cache reads are
    // 0.025x and Opus 5.5 0.05x, not the 0.1x default.
    ("claude-fable-5-1", per_m(10.0, 50.0, Some(12.5), Some(0.25))),
    ("claude-mythos-5-1", per_m(10.0, 50.0, Some(12.5), Some(0.25))),
    ("claude-fable-5", per_m(10.0, 50.0, Some(12.5), Some(1.0))),
    ("claude-mythos-5", per_m(10.0, 50.0, Some(12.5), Some(1.0))),
    ("claude-opus-5-5", per_m(4.0, 20.0, Some(5.0), Some(0.2))),
    ("claude-opus-5", per_m(5.0, 25.0, Some(6.25), Some(0.5))),
    ("claude-sonnet-5-5", per_m(2.0, 10.0, Some(2.5), Some(0.2))),
    ("claude-sonnet-5", per_m(2.0, 10.0, Some(2.5), Some(0.2))),
    // OpenAI (developers.openai.com/api/docs/pricing). Prompts >272K
    // input bill the whole request at 2x input / 1.5x output; Fast is 2x
    // (FAST_DEFAULT). GPT-5.6 rates are the current ones; earlier list
    // prices live in PRICE_ERAS.
    ("gpt-6-astra", tiered(per_m(10.0, 50.0, Some(12.5), Some(1.0)), 272_000, 20.0, 75.0, Some(25.0), Some(2.0))),
    ("gpt-6-sol", tiered(per_m(2.0, 10.0, Some(2.5), Some(0.2)), 272_000, 4.0, 15.0, Some(5.0), Some(0.4))),
    ("gpt-6.1-sol", tiered(per_m(2.0, 10.0, Some(2.5), Some(0.1)), 272_000, 4.0, 15.0, Some(5.0), Some(0.2))),
    ("gpt-6-luna", tiered(per_m(0.1, 0.5, Some(0.125), Some(0.01)), 272_000, 0.2, 0.75, Some(0.25), Some(0.02))),
    ("gpt-5.6-sol", tiered(per_m(4.0, 20.0, Some(5.0), Some(0.4)), 272_000, 8.0, 30.0, Some(10.0), Some(0.8))),
    ("gpt-5.6", tiered(per_m(4.0, 20.0, Some(5.0), Some(0.4)), 272_000, 8.0, 30.0, Some(10.0), Some(0.8))),
    ("gpt-5.6-terra", tiered(per_m(2.0, 12.0, Some(2.5), Some(0.2)), 272_000, 4.0, 18.0, Some(5.0), Some(0.4))),
    ("gpt-5.6-luna", tiered(per_m(0.2, 1.2, Some(0.25), Some(0.02)), 272_000, 0.4, 1.8, Some(0.5), Some(0.04))),
    // Moonshot Kimi (platform.kimi.ai/docs/pricing/chat). Kimi charges no
    // cache-write surcharge, so writes bill at the input rate.
    // kimi-for-coding is a Kimi Code subscription model id; valued at
    // K2.6 API rates (ccusage parity).
    ("kimi-for-coding", per_m(0.95, 4.0, Some(0.95), Some(0.16))),
    ("kimi-k2.6", per_m(0.95, 4.0, Some(0.95), Some(0.16))),
    ("kimi-k2.7-code", per_m(0.95, 4.0, Some(0.95), Some(0.19))),
    ("kimi-k3", per_m(3.0, 15.0, Some(3.0), Some(0.3))),
    // Z.ai GLM (docs.z.ai/guides/overview/pricing): cached input $0.26/1M
    // (~0.19x, not 0.1x); cache storage currently free, so writes bill
    // as input.
    ("glm-5.1", per_m(1.4, 4.4, Some(1.4), Some(0.26))),
    ("glm-5.2", per_m(1.4, 4.4, Some(1.4), Some(0.26))),
    ("glm-5.3", per_m(1.4, 4.4, Some(1.4), Some(0.26))),
    ("glm-5.3-flash", per_m(0.15, 0.5, Some(0.15), Some(0.03))),
    ("glm-5.3-flashx", per_m(0.37, 1.25, Some(0.37), Some(0.075))),
    // DeepSeek (api-docs.deepseek.com/quick_start/pricing), peak rates;
    // off-peak halving is applied by `time_multiplier`. No cache-write
    // surcharge. `deepseek-v4-flash` now routes to deepseek-flash (V4.1).
    ("deepseek-v4-pro", per_m(1.32, 3.96, Some(1.32), Some(0.044))),
    ("deepseek-flash", per_m(0.3, 1.2, Some(0.3), Some(0.006))),
    ("deepseek-v4-flash", per_m(0.3, 1.2, Some(0.3), Some(0.006))),
    // MiniMax (platform.minimax.io/docs/guides/pricing-paygo).
    ("MiniMax-M2.7", per_m(0.3, 1.2, Some(0.375), Some(0.06))),
    ("MiniMax-M3", tiered(per_m(0.3, 1.2, None, Some(0.06)), 512_000, 0.6, 2.4, None, Some(0.12))),
    // StepFun Step-3.5-Flash ($0.10 / $0.30, cache read $0.02 per 1M);
    // also matches dated variants like step-3.5-flash-2603.
    ("step-3.5-flash", per_m(0.1, 0.3, None, Some(0.02))),
    // Meituan LongCat (longcat.chat platform). Flash-Chat was retired in
    // 2026; its rate is the last hosted price, kept for history.
    ("LongCat-2.0", per_m(0.3, 1.2, None, Some(0.006))),
    ("LongCat-2.5-Preview", per_m(0.3, 1.2, None, Some(0.006))),
    ("LongCat-Flash-Chat", per_m(0.2, 0.8, None, None)),
    // Alibaba Model Studio, international. qwen3.8-max-preview was
    // retired and now routes to (and bills as) qwen3.8-max.
    ("qwen3.8-max", per_m(2.0, 6.0, Some(2.5), Some(0.25))),
    ("qwen3.8-max-preview", per_m(2.0, 6.0, Some(2.5), Some(0.25))),
    // Models with no per-token price: billed inside a subscription
    // (codex-auto-review: Codex's approval reviewer, "the backend controls
    // billing") or free (OpenCode's big-pickle). Pricing them at $0
    // explicitly keeps them out of the "unpriced" list.
    ("codex-auto-review", per_m(0.0, 0.0, None, None)),
    ("big-pickle", per_m(0.0, 0.0, None, None)),
];

/// A dated list-price change: usage before `until` (UTC) bills at
/// `pricing` instead of the model's current rate.
struct PriceEra {
    model: &'static str,
    until: &'static str,
    pricing: ModelPricing,
}

const PRICE_ERAS: &[PriceEra] = &[
    // GPT-5.6 launch prices (2026-07-09). Terra/Luna were cut permanently
    // on 2026-07-30; Sol got a promotional cut on 2026-08-21 (current).
    PriceEra {
        model: "gpt-5.6-sol",
        until: "2026-08-21T00:00:00Z",
        pricing: tiered(per_m(5.0, 30.0, Some(6.25), Some(0.5)), 272_000, 10.0, 45.0, Some(12.5), Some(1.0)),
    },
    PriceEra {
        model: "gpt-5.6",
        until: "2026-08-21T00:00:00Z",
        pricing: tiered(per_m(5.0, 30.0, Some(6.25), Some(0.5)), 272_000, 10.0, 45.0, Some(12.5), Some(1.0)),
    },
    PriceEra {
        model: "gpt-5.6-terra",
        until: "2026-07-30T00:00:00Z",
        pricing: tiered(per_m(2.5, 15.0, Some(3.125), Some(0.25)), 272_000, 5.0, 22.5, Some(6.25), Some(0.5)),
    },
    PriceEra {
        model: "gpt-5.6-luna",
        until: "2026-07-30T00:00:00Z",
        pricing: tiered(per_m(1.0, 6.0, Some(1.25), Some(0.1)), 272_000, 2.0, 9.0, Some(2.5), Some(0.2)),
    },
    // DeepSeek V4 flat list prices before peak/off-peak pricing started
    // (2026-08-16 16:00 UTC).
    PriceEra {
        model: "deepseek-v4-pro",
        until: "2026-08-16T16:00:00Z",
        pricing: per_m(0.435, 0.87, Some(0.435), Some(0.003625)),
    },
    PriceEra {
        model: "deepseek-v4-flash",
        until: "2026-08-16T16:00:00Z",
        pricing: per_m(0.14, 0.28, Some(0.14), Some(0.0028)),
    },
];

/// DeepSeek peak/off-peak pricing (since 2026-08-16 16:00 UTC): peak is
/// Mon–Fri 01:00–04:00 and 06:00–10:00 UTC, everything else bills at half
/// price. Chinese public holidays (also off-peak) are not modeled.
fn time_multiplier(canonical_model: &str, ts_ms: i64) -> f64 {
    use chrono::{Datelike, TimeZone, Timelike, Weekday};
    const DEEPSEEK_TOU_START_MS: i64 = 1_786_896_000_000; // 2026-08-16T16:00:00Z
    if !canonical_model.starts_with("deepseek") || ts_ms < DEEPSEEK_TOU_START_MS {
        return 1.0;
    }
    let Some(t) = chrono::Utc.timestamp_millis_opt(ts_ms).single() else {
        return 1.0;
    };
    let weekday = !matches!(t.weekday(), Weekday::Sat | Weekday::Sun);
    let h = t.hour();
    let peak = weekday && ((1..4).contains(&h) || (6..10).contains(&h));
    if peak {
        1.0
    } else {
        0.5
    }
}

/// Lowercase, provider prefix stripped ("openrouter/z-ai/glm-5.1" ->
/// "glm-5.1"), `.`/`@`/`_` -> `-`, so names from different catalogs and
/// tools compare equal.
fn canonical(model: &str) -> String {
    let tail = model.rsplit('/').next().unwrap_or(model);
    tail.to_ascii_lowercase().replace(['.', '@', '_'], "-")
}

/// Lower is preferred when several catalog keys match one model: the
/// vendor's own bare entry, then vendor-prefixed entries, then resellers.
fn provider_rank(key: &str) -> u8 {
    if !key.contains('/') {
        0
    } else if key.starts_with("openrouter/") {
        2
    } else {
        1
    }
}

struct Entry {
    key: String,
    canon: String,
    rank: u8,
    pricing: ModelPricing,
}

pub struct PricingMap {
    entries: Vec<Entry>,
    by_key: HashMap<String, usize>,
    /// When the online LiteLLM cache was fetched (file mtime), if loaded.
    pub fetched_at_ms: Option<i64>,
    fingerprint: u64,
    memo: Mutex<HashMap<String, Option<ModelPricing>>>,
}

impl PricingMap {
    /// Load the embedded snapshot, overlay the cached online refresh if
    /// present, then fill gaps from the builtin table.
    pub fn load(cache_dir: Option<PathBuf>) -> Self {
        let mut raw: HashMap<String, ModelPricing> = parse_litellm(EMBEDDED_PRICING);
        let mut fetched_at_ms = None;
        if let Some(dir) = cache_dir {
            let cache_file = dir.join(CACHE_FILE);
            if let Ok(text) = std::fs::read_to_string(&cache_file) {
                let online = parse_litellm(&text);
                if !online.is_empty() {
                    raw.extend(online);
                    fetched_at_ms = std::fs::metadata(&cache_file)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as i64);
                }
            }
        }
        for (model, pricing) in BUILTIN_PRICING {
            raw.entry((*model).to_string()).or_insert(*pricing);
        }
        Self::from_entries(raw, fetched_at_ms)
    }

    fn from_entries(raw: HashMap<String, ModelPricing>, fetched_at_ms: Option<i64>) -> Self {
        let mut entries: Vec<Entry> = raw
            .into_iter()
            .map(|(key, pricing)| Entry {
                canon: canonical(&key),
                rank: provider_rank(&key),
                key,
                pricing,
            })
            .collect();
        // Deterministic order: matching tie-breaks and the fingerprint
        // must not depend on HashMap iteration order.
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        let by_key = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.key.clone(), i))
            .collect();

        let mut h = DefaultHasher::new();
        RULES_VERSION.hash(&mut h);
        for e in &entries {
            e.key.hash(&mut h);
            e.pricing.hash_into(&mut h);
        }
        Self {
            entries,
            by_key,
            fetched_at_ms,
            fingerprint: h.finish(),
            memo: Mutex::new(HashMap::new()),
        }
    }

    pub fn model_count(&self) -> usize {
        self.entries.len()
    }

    /// Changes whenever any rate or rule changes; stored costs are
    /// re-priced when it differs from the one they were computed with.
    pub fn fingerprint(&self) -> String {
        format!("{:016x}", self.fingerprint)
    }

    /// Fetch the latest LiteLLM pricing (with mirror fallback) and write
    /// the relevant subset to the cache dir. Returns the model count.
    pub fn refresh_online(cache_dir: &Path) -> Result<usize, String> {
        let mut errors = Vec::new();
        for url in PRICING_URLS {
            match fetch_relevant(url) {
                Ok(subset) => {
                    let count = subset.len();
                    std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
                    let tmp = cache_dir.join(format!("{CACHE_FILE}.tmp"));
                    std::fs::write(&tmp, serde_json::to_string(&subset).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                    std::fs::rename(&tmp, cache_dir.join(CACHE_FILE)).map_err(|e| e.to_string())?;
                    return Ok(count);
                }
                Err(e) => errors.push(format!("{url}: {e}")),
            }
        }
        Err(errors.join("; "))
    }

    /// Whether any price is known for `model` (fast suffix ignored).
    pub fn is_priced(&self, model: &str) -> bool {
        self.resolve(model).is_some()
    }

    /// Current pricing for a model (no date-dependent adjustments). A
    /// "-fast" suffix (set by the adapters for fast/priority service-tier
    /// usage) resolves to the base model's fast-tier rates.
    pub fn resolve(&self, model: &str) -> Option<ModelPricing> {
        let (base, fast) = split_fast(model);
        let p = self.find(base)?;
        Some(if fast { p.fast(fast_multiplier_for(base)) } else { p })
    }

    /// Pricing in effect for usage of `model` at `ts_ms`: dated list-price
    /// eras and time-of-day discounts applied on top of [`resolve`].
    pub fn resolve_at(&self, model: &str, ts_ms: i64) -> Option<ModelPricing> {
        let (base, fast) = split_fast(model);
        let canon = canonical(base);
        let p = era_pricing(&canon, ts_ms).or_else(|| self.find(base))?;
        let p = if fast { p.fast(fast_multiplier_for(base)) } else { p };
        let tm = time_multiplier(&canon, ts_ms);
        Some(if tm == 1.0 { p } else { p.scaled(tm) })
    }

    /// Model-name matching (memoized): exact key -> same canonical name ->
    /// boundary-aware canonical substring. Ported from ccusage's matcher,
    /// with provider-prefix awareness so a reseller's entry
    /// ("openrouter/...") never shadows the vendor's own price.
    pub fn find(&self, model: &str) -> Option<ModelPricing> {
        if model.is_empty() || model == "<synthetic>" {
            return None;
        }
        if let Some(hit) = self.memo.lock().ok().and_then(|m| m.get(model).copied()) {
            return hit;
        }
        let found = self.find_uncached(model);
        if let Ok(mut m) = self.memo.lock() {
            m.insert(model.to_string(), found);
        }
        found
    }

    fn find_uncached(&self, model: &str) -> Option<ModelPricing> {
        if let Some(&i) = self.by_key.get(model) {
            return Some(self.entries[i].pricing);
        }
        let cm = canonical(model);
        // Same canonical name: best provider rank, then key order.
        if let Some(e) = self
            .entries
            .iter()
            .filter(|e| e.canon == cm && !e.key.contains(':'))
            .min_by_key(|e| e.rank)
        {
            return Some(e.pricing);
        }
        // Catalog key inside the model name (dated / suffixed variants):
        // longest key wins. Free placeholder entries never match loosely.
        let usable = |e: &&Entry| !e.key.contains(':') && !e.pricing.is_free();
        if let Some(e) = self
            .entries
            .iter()
            .filter(usable)
            .filter(|e| contains_at_boundary(&cm, &e.canon))
            .min_by_key(|e| (std::cmp::Reverse(e.canon.len()), e.rank))
        {
            return Some(e.pricing);
        }
        // Model name inside a catalog key: closest (shortest) key wins.
        self.entries
            .iter()
            .filter(usable)
            .filter(|e| contains_at_boundary(&e.canon, &cm))
            .min_by_key(|e| (e.canon.len(), e.rank))
            .map(|e| e.pricing)
    }
}

fn split_fast(model: &str) -> (&str, bool) {
    match model.strip_suffix("-fast") {
        Some(base) => (base, true),
        None => (model, false),
    }
}

fn era_pricing(canon: &str, ts_ms: i64) -> Option<ModelPricing> {
    PRICE_ERAS
        .iter()
        .filter(|era| canonical(era.model) == strip_date_suffix(canon))
        .find(|era| {
            chrono::DateTime::parse_from_rfc3339(era.until)
                .map(|until| ts_ms < until.timestamp_millis())
                .unwrap_or(false)
        })
        .map(|era| era.pricing)
}

/// "gpt-5-6-sol-2026-07-09" / "...-20260709" -> "gpt-5-6-sol".
fn strip_date_suffix(canon: &str) -> &str {
    let b = canon.as_bytes();
    let digits = |s: &[u8]| s.iter().all(u8::is_ascii_digit);
    if b.len() > 11 && b[b.len() - 11] == b'-' {
        let d = &b[b.len() - 10..];
        if digits(&d[..4]) && d[4] == b'-' && digits(&d[5..7]) && d[7] == b'-' && digits(&d[8..]) {
            return &canon[..canon.len() - 11];
        }
    }
    if b.len() > 9 && b[b.len() - 9] == b'-' && digits(&b[b.len() - 8..]) {
        return &canon[..canon.len() - 9];
    }
    canon
}

fn fetch_relevant(url: &str) -> Result<Map<String, Value>, String> {
    let body = ureq::get(url)
        .timeout(std::time::Duration::from_secs(20))
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    let all: Map<String, Value> = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let subset: Map<String, Value> = all
        .into_iter()
        .filter(|(k, _)| is_relevant_model(k))
        .collect();
    // A truncated or captive-portal response must not replace good data.
    if subset.len() < 100 {
        return Err(format!("only {} relevant models in response", subset.len()));
    }
    Ok(subset)
}

/// Parse a LiteLLM-format JSON object into per-model pricing.
pub fn parse_litellm(text: &str) -> HashMap<String, ModelPricing> {
    let Ok(raw) = serde_json::from_str::<Map<String, Value>>(text) else {
        return HashMap::new();
    };
    raw.iter()
        .filter_map(|(k, v)| Some((k.clone(), pricing_from_litellm(v.as_object()?)?)))
        .collect()
}

fn pricing_from_litellm(obj: &Map<String, Value>) -> Option<ModelPricing> {
    let f = |k: &str| obj.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0);
    let rates = |suffix: &str| Rates {
        input: f(&format!("input_cost_per_token{suffix}")),
        output: f(&format!("output_cost_per_token{suffix}")),
        cache_write_5m: f(&format!("cache_creation_input_token_cost{suffix}")),
        cache_write_1h: f(&format!("cache_creation_input_token_cost_above_1hr{suffix}")),
        cache_read: f(&format!("cache_read_input_token_cost{suffix}")),
    };
    let base = rates("");
    if base.input.is_none() && base.output.is_none() {
        return None;
    }
    // Long-context tier: the smallest "_above_<N>k_tokens" threshold
    // published for input or output.
    let threshold_k = obj
        .keys()
        .filter_map(|k| {
            let rest = k
                .strip_prefix("input_cost_per_token_above_")
                .or_else(|| k.strip_prefix("output_cost_per_token_above_"))?;
            rest.strip_suffix("k_tokens")?.parse::<u64>().ok()
        })
        .min();
    let (tier_threshold, above, priority_above) = match threshold_k {
        Some(n) => {
            let s = format!("_above_{n}k_tokens");
            (n * 1000, rates(&s), rates(&format!("{s}_priority")))
        }
        None => (0, Rates::default(), Rates::default()),
    };
    Some(ModelPricing {
        base,
        tier_threshold,
        above,
        priority: Rates {
            cache_write_1h: None,
            ..rates("_priority")
        },
        priority_above,
    })
}

pub fn is_relevant_model(key: &str) -> bool {
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
            || k.starts_with(&format!("dashscope/{p}"))
            || k.starts_with(&format!("openrouter/{p}"))
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis()
    }

    fn per_1m(v: f64) -> f64 {
        (v * 1e6 * 1e6).round() / 1e6
    }

    /// Official Anthropic prices per 1M (input, output, 5m write, read).
    #[test]
    fn claude_current_prices() {
        let map = PricingMap::load(None);
        for (model, i, o, w, r) in [
            ("claude-opus-5-5", 4.0, 20.0, 5.0, 0.2),
            ("claude-sonnet-5-5", 2.0, 10.0, 2.5, 0.2),
            ("claude-fable-5-1", 10.0, 50.0, 12.5, 0.25),
            ("claude-fable-5", 10.0, 50.0, 12.5, 1.0),
            ("claude-opus-5", 5.0, 25.0, 6.25, 0.5),
        ] {
            let e = map.resolve(model).unwrap_or_else(|| panic!("{model}")).effective(0);
            assert_eq!(
                (per_1m(e.input), per_1m(e.output), per_1m(e.cache_write_5m), per_1m(e.cache_read)),
                (i, o, w, r),
                "{model}"
            );
            assert_eq!(per_1m(e.cache_write_1h), i * 2.0, "{model} 1h write");
        }
        // Opus 5.5 must not fall back to Opus 5 rates via substring match.
        let e = map.resolve("claude-opus-5-5").unwrap().effective(0);
        assert_eq!(per_1m(e.input), 4.0);
    }

    #[test]
    fn claude_fast_mode_multipliers() {
        let map = PricingMap::load(None);
        for (model, input_fast) in [
            ("claude-opus-5-5-fast", 8.0),
            ("claude-opus-5-fast", 10.0),
            ("claude-opus-4-6-fast", 30.0),
        ] {
            let e = map.resolve(model).unwrap().effective(0);
            assert_eq!(per_1m(e.input), input_fast, "{model}");
        }
    }

    /// OpenAI Fast is 2x everywhere except GPT-5.5 (2.5x); long prompts
    /// (>272K) bill the whole request at the long-context rates.
    #[test]
    fn openai_fast_and_long_context() {
        let map = PricingMap::load(None);
        let sol = map.resolve("gpt-6.1-sol").unwrap();
        assert_eq!(per_1m(sol.effective(1000).input), 2.0);
        assert_eq!(per_1m(sol.effective(300_000).input), 4.0);
        assert_eq!(per_1m(sol.effective(300_000).output), 15.0);
        let fast = map.resolve("gpt-6-astra-fast").unwrap();
        assert_eq!(per_1m(fast.effective(1000).input), 20.0);
        assert_eq!(per_1m(fast.effective(1000).output), 100.0);
        assert_eq!(per_1m(fast.effective(1000).cache_read), 2.0);
        assert_eq!(per_1m(fast.effective(300_000).input), 40.0);
        let g55 = map.resolve("gpt-5.5-fast").unwrap().effective(0);
        assert_eq!(per_1m(g55.input), 12.5);
    }

    #[test]
    fn gpt_5_6_price_eras() {
        let map = PricingMap::load(None);
        let at = |m: &str, t: &str| {
            let e = map.resolve_at(m, ts(t)).unwrap().effective(0);
            (per_1m(e.input), per_1m(e.output), per_1m(e.cache_read))
        };
        assert_eq!(at("gpt-5.6-sol", "2026-08-01T00:00:00Z"), (5.0, 30.0, 0.5));
        assert_eq!(at("gpt-5.6-sol", "2026-09-01T00:00:00Z"), (4.0, 20.0, 0.4));
        assert_eq!(at("gpt-5.6-terra", "2026-07-20T00:00:00Z"), (2.5, 15.0, 0.25));
        assert_eq!(at("gpt-5.6-terra", "2026-08-01T00:00:00Z"), (2.0, 12.0, 0.2));
        assert_eq!(at("gpt-5.6-luna", "2026-07-20T00:00:00Z"), (1.0, 6.0, 0.1));
        // Fast tier applies on top of the era price.
        let fast = map.resolve_at("gpt-5.6-sol-fast", ts("2026-08-01T00:00:00Z")).unwrap();
        assert_eq!(per_1m(fast.effective(0).input), 10.0);
        // Eras match exactly, never a sibling model.
        assert!(era_pricing("gpt-5-6-cyber", ts("2026-07-20T00:00:00Z")).is_none());
    }

    #[test]
    fn deepseek_off_peak_discount() {
        let map = PricingMap::load(None);
        // Monday 2026-09-07 02:00 UTC = peak; 12:00 UTC = off-peak.
        let peak = map.resolve_at("deepseek-v4-pro", ts("2026-09-07T02:00:00Z")).unwrap();
        let off = map.resolve_at("deepseek-v4-pro", ts("2026-09-07T12:00:00Z")).unwrap();
        assert_eq!(per_1m(peak.effective(0).input), 1.32);
        assert_eq!(per_1m(off.effective(0).input), 0.66);
        let old = map.resolve_at("deepseek-v4-pro", ts("2026-07-01T02:00:00Z")).unwrap();
        assert_eq!(per_1m(old.effective(0).input), 0.435);
    }

    /// Cache-read rates that deviate from the 0.1x default and are therefore
    /// set explicitly from the vendor's published cached-input price.
    #[test]
    fn explicit_cache_read_prices() {
        let map = PricingMap::load(None);
        for (model, cached_1m) in [
            ("glm-5.1", 0.26),
            ("MiniMax-M2.7", 0.06),
            ("qwen3.8-max-preview", 0.25),
        ] {
            let e = map.resolve(model).unwrap_or_else(|| panic!("{model}")).effective(0);
            assert_eq!(per_1m(e.cache_read), cached_1m, "{model}");
        }
    }

    #[test]
    fn provider_prefixed_and_dated_names() {
        let map = PricingMap::load(None);
        let input = |m: &str| per_1m(map.resolve(m).unwrap().effective(0).input);
        assert_eq!(input("chatgpt/gpt-5.6-sol"), 4.0);
        assert_eq!(input("claude-haiku-4-5-20251001"), 1.0);
        assert_eq!(input("anthropic/claude-opus-5-5"), 4.0);
        // A reseller entry must not beat the vendor's own price.
        assert_eq!(input("kimi-k2.6"), 0.95);
        assert!(map.resolve("codex-auto-review").is_some());
        assert!(map.resolve("totally-unknown-model").is_none());
    }

    #[test]
    fn parses_litellm_tiers_and_priority() {
        let p = pricing_from_litellm(
            serde_json::json!({
                "input_cost_per_token": 2e-6,
                "output_cost_per_token": 1e-5,
                "cache_read_input_token_cost": 1e-7,
                "input_cost_per_token_above_272k_tokens": 4e-6,
                "output_cost_per_token_above_272k_tokens": 1.5e-5,
                "input_cost_per_token_priority": 4e-6,
                "output_cost_per_token_priority": 2e-5,
                "input_cost_per_token_above_272k_tokens_priority": 8e-6
            })
            .as_object()
            .unwrap(),
        )
        .unwrap();
        assert_eq!(p.tier_threshold, 272_000);
        let fast = p.fast(FAST_DEFAULT);
        assert_eq!(per_1m(fast.effective(0).output), 20.0);
        assert_eq!(per_1m(fast.effective(300_000).input), 8.0);
        // Unpublished fast long-context output = standard x vendor ratio.
        assert_eq!(per_1m(fast.effective(300_000).output), 30.0);
    }

    #[test]
    fn fingerprint_tracks_rates() {
        let a = PricingMap::load(None);
        let b = PricingMap::load(None);
        assert_eq!(a.fingerprint(), b.fingerprint());
        let mut raw = HashMap::new();
        raw.insert("x".to_string(), per_m(1.0, 2.0, None, None));
        let c = PricingMap::from_entries(raw.clone(), None);
        raw.insert("x".to_string(), per_m(1.0, 3.0, None, None));
        let d = PricingMap::from_entries(raw, None);
        assert_ne!(c.fingerprint(), d.fingerprint());
    }

    #[test]
    fn date_suffix_stripping() {
        assert_eq!(strip_date_suffix("gpt-5-6-sol-2026-07-09"), "gpt-5-6-sol");
        assert_eq!(strip_date_suffix("claude-opus-4-5-20251101"), "claude-opus-4-5");
        assert_eq!(strip_date_suffix("gpt-5-6-sol"), "gpt-5-6-sol");
    }
}
