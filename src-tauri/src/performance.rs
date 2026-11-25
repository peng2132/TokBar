use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

pub struct PerformanceSample {
    pub key: String,
    pub timestamp_ms: i64,
    pub model: String,
    pub provider: String,
    pub output_tps: Option<f64>,
    pub first_token_ms: Option<i64>,
}

/// Discard tiny replies and durations where a buffered chunk can dominate the rate.
pub(crate) fn output_rate(tokens: u64, duration_ms: i64) -> Option<f64> {
    if tokens < 100 || duration_ms < 100 {
        return None;
    }
    let rate = tokens as f64 * 1000.0 / duration_ms as f64;
    (rate <= 10_000.0).then_some(rate)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPerformance {
    pub source: String,
    pub model: String,
    pub provider: String,
    pub output_tps: Option<f64>,
    pub output_samples: usize,
    pub first_token_ms: Option<f64>,
    pub first_token_samples: usize,
}

#[derive(Default)]
struct Measurements {
    output: Vec<f64>,
    first_token: Vec<f64>,
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some((values[(values.len() - 1) / 2] + values[middle]) / 2.0)
}

pub fn models(
    conn: &Connection,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
) -> rusqlite::Result<Vec<ModelPerformance>> {
    let mut stmt = conn.prepare(
        "SELECT source, model, provider, output_tps, first_token_ms FROM performance_samples
         WHERE timestamp_ms >= ?1 AND timestamp_ms < ?2",
    )?;
    let rows = stmt.query_map(
        [since_ms.unwrap_or(i64::MIN), until_ms.unwrap_or(i64::MAX)],
        |row| {
            Ok((
                (row.get(0)?, row.get(1)?, row.get(2)?),
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, Option<f64>>(4)?,
            ))
        },
    )?;
    let mut groups: HashMap<_, Measurements> = HashMap::new();
    for row in rows {
        let (key, speed, first_token) = row?;
        let samples = groups.entry(key).or_default();
        samples.output.extend(speed);
        samples.first_token.extend(first_token);
    }
    let mut result: Vec<_> = groups
        .into_iter()
        .map(
            |((source, model, provider), mut samples)| ModelPerformance {
                source,
                model,
                provider,
                output_tps: median(&mut samples.output),
                output_samples: samples.output.len(),
                first_token_ms: median(&mut samples.first_token),
                first_token_samples: samples.first_token.len(),
            },
        )
        .collect();
    result.sort_unstable_by(|a, b| {
        b.output_tps
            .unwrap_or(0.0)
            .total_cmp(&a.output_tps.unwrap_or(0.0))
            .then_with(|| a.model.cmp(&b.model))
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.provider.cmp(&b.provider))
    });
    Ok(result)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestPerformance {
    pub source: String,
    pub model: String,
    pub provider: String,
    pub timestamp_ms: i64,
    pub output_tps: f64,
}

pub fn latest(conn: &Connection) -> rusqlite::Result<Option<LatestPerformance>> {
    conn.query_row(
        "SELECT source, model, provider, timestamp_ms, output_tps FROM performance_samples
         WHERE output_tps IS NOT NULL ORDER BY timestamp_ms DESC LIMIT 1",
        [],
        |row| {
            Ok(LatestPerformance {
                source: row.get(0)?,
                model: row.get(1)?,
                provider: row.get(2)?,
                timestamp_ms: row.get(3)?,
                output_tps: row.get(4)?,
            })
        },
    )
    .optional()
}
