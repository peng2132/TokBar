use crate::pricing::ModelPricing;
use crate::types::UsageRecord;

/// Cost mode semantics, identical to ccusage:
/// - Auto: prefer the log's costUSD, otherwise calculate from tokens.
/// - Calculate: always calculate from tokens.
/// - Display: only use the log's costUSD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostMode {
    Auto,
    Calculate,
    Display,
}

impl CostMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "calculate" => Self::Calculate,
            "display" => Self::Display,
            _ => Self::Auto,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Calculate => "calculate",
            Self::Display => "display",
        }
    }
}

/// Token counts of one request, as stored per entry.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokenCounts {
    pub input: u64,
    pub output: u64,
    pub cache_write_5m: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
}

impl From<&UsageRecord> for TokenCounts {
    fn from(r: &UsageRecord) -> Self {
        Self {
            input: r.input_tokens,
            output: r.output_tokens,
            cache_write_5m: r.cache_creation_5m,
            cache_write_1h: r.cache_creation_1h,
            cache_read: r.cache_read_tokens,
        }
    }
}

/// Calculate cost from token counts with pricing already resolved for the
/// request's model and time. Long-context pricing applies to the whole
/// request once its prompt (input + cache writes + cache reads) crosses
/// the model's tier threshold, as Anthropic and OpenAI bill it.
pub fn calculate_cost(t: TokenCounts, p: &ModelPricing) -> f64 {
    let prompt = t.input + t.cache_write_5m + t.cache_write_1h + t.cache_read;
    let r = p.effective(prompt);
    t.input as f64 * r.input
        + t.output as f64 * r.output
        + t.cache_write_5m as f64 * r.cache_write_5m
        + t.cache_write_1h as f64 * r.cache_write_1h
        + t.cache_read as f64 * r.cache_read
}

pub fn calculate_cost_with(record: &UsageRecord, p: &ModelPricing) -> f64 {
    calculate_cost(record.into(), p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::PricingMap;

    #[test]
    fn whole_request_bills_at_long_context_rate() {
        let map = PricingMap::load(None);
        let p = map.resolve("gpt-6.1-sol").unwrap();
        // 200K fresh + 100K cached = 300K prompt > 272K: every token at
        // the long-context rates ($4 in, $0.20 cached, $15 out per 1M).
        let t = TokenCounts {
            input: 200_000,
            output: 1_000,
            cache_read: 100_000,
            ..Default::default()
        };
        let want = 200_000.0 * 4e-6 + 100_000.0 * 2e-7 + 1_000.0 * 1.5e-5;
        assert!((calculate_cost(t, &p) - want).abs() < 1e-9);
    }

    #[test]
    fn one_hour_cache_writes_bill_at_2x_input() {
        let map = PricingMap::load(None);
        let p = map.resolve("claude-opus-5-5").unwrap();
        let t = TokenCounts {
            cache_write_1h: 1_000_000,
            ..Default::default()
        };
        assert!((calculate_cost(t, &p) - 8.0).abs() < 1e-9);
    }
}
