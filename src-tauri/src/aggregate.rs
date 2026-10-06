use chrono::TimeZone;
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::cost::CostMode;

const MILLIS_PER_HOUR: i64 = 3_600_000;

/// SQL expression for the effective cost under a cost mode.
pub fn cost_expr(mode: CostMode) -> &'static str {
    match mode {
        CostMode::Auto => "COALESCE(cost_usd, calculated_cost)",
        CostMode::Calculate => "calculated_cost",
        CostMode::Display => "COALESCE(cost_usd, 0)",
    }
}

fn range_clause(since_ms: Option<i64>, until_ms: Option<i64>) -> String {
    let mut clause = String::from("1=1");
    if let Some(s) = since_ms {
        clause.push_str(&format!(" AND timestamp_ms >= {s}"));
    }
    if let Some(u) = until_ms {
        clause.push_str(&format!(" AND timestamp_ms < {u}"));
    }
    clause
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub cost: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub total_tokens: i64,
    pub requests: i64,
    pub sessions: i64,
    pub active_days: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBreakdown {
    pub agent: String,
    pub cost: f64,
    pub total_tokens: i64,
    pub requests: i64,
    pub sessions: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub totals: Totals,
    pub by_agent: Vec<AgentBreakdown>,
}

pub fn overview(
    conn: &Connection,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    mode: CostMode,
) -> Result<Overview, String> {
    let cost = cost_expr(mode);
    let range = range_clause(since_ms, until_ms);
    let totals = conn
        .query_row(
            &format!(
                "SELECT COALESCE(SUM({cost}),0), COALESCE(SUM(input_tokens),0),
                        COALESCE(SUM(output_tokens),0),
                        COALESCE(SUM(cache_creation_5m + cache_creation_1h),0),
                        COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(total_tokens),0),
                        COUNT(*), COUNT(DISTINCT agent || ':' || session_id),
                        COUNT(DISTINCT date_local)
                 FROM entries WHERE {range}"
            ),
            [],
            |row| {
                Ok(Totals {
                    cost: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cache_creation_tokens: row.get(3)?,
                    cache_read_tokens: row.get(4)?,
                    total_tokens: row.get(5)?,
                    requests: row.get(6)?,
                    sessions: row.get(7)?,
                    active_days: row.get(8)?,
                })
            },
        )
        .map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare(&format!(
            "SELECT agent, COALESCE(SUM({cost}),0), COALESCE(SUM(total_tokens),0), COUNT(*),
                    COUNT(DISTINCT session_id)
             FROM entries WHERE {range} GROUP BY agent ORDER BY 2 DESC"
        ))
        .map_err(|e| e.to_string())?;
    let by_agent = stmt
        .query_map([], |row| {
            Ok(AgentBreakdown {
                agent: row.get(0)?,
                cost: row.get(1)?,
                total_tokens: row.get(2)?,
                requests: row.get(3)?,
                sessions: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    Ok(Overview { totals, by_agent })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyRow {
    pub date: String,
    pub agent: String,
    pub cost: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub total_tokens: i64,
    pub requests: i64,
}

pub fn daily(
    conn: &Connection,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    mode: CostMode,
) -> Result<Vec<DailyRow>, String> {
    let cost = cost_expr(mode);
    let range = range_clause(since_ms, until_ms);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT date_local, agent, COALESCE(SUM({cost}),0),
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_creation_5m + cache_creation_1h),0),
                    COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(total_tokens),0), COUNT(*)
             FROM entries WHERE {range}
             GROUP BY date_local, agent ORDER BY date_local ASC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(DailyRow {
                date: row.get(0)?,
                agent: row.get(1)?,
                cost: row.get(2)?,
                input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                cache_creation_tokens: row.get(5)?,
                cache_read_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                requests: row.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

/// Same row shape as daily, but bucketed by local hour ("HH:00") —
/// used when the UI is showing a single day.
pub fn hourly(
    conn: &Connection,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    mode: CostMode,
) -> Result<Vec<DailyRow>, String> {
    let cost = cost_expr(mode);
    let range = range_clause(since_ms, until_ms);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT strftime('%H:00', timestamp_ms/1000, 'unixepoch', 'localtime'), agent,
                    COALESCE(SUM({cost}),0),
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_creation_5m + cache_creation_1h),0),
                    COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(total_tokens),0), COUNT(*)
             FROM entries WHERE {range}
             GROUP BY 1, agent ORDER BY 1 ASC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(DailyRow {
                date: row.get(0)?,
                agent: row.get(1)?,
                cost: row.get(2)?,
                input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                cache_creation_tokens: row.get(5)?,
                cache_read_tokens: row.get(6)?,
                total_tokens: row.get(7)?,
                requests: row.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRow {
    pub model: String,
    pub cost: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub total_tokens: i64,
    pub requests: i64,
    /// False when no price is known for the model (its cost reads $0);
    /// filled in by the command layer, which owns the pricing table.
    pub priced: bool,
}

pub fn models(
    conn: &Connection,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    mode: CostMode,
) -> Result<Vec<ModelRow>, String> {
    let cost = cost_expr(mode);
    let range = range_clause(since_ms, until_ms);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT model, COALESCE(SUM({cost}),0),
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_creation_5m + cache_creation_1h),0),
                    COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(total_tokens),0), COUNT(*)
             FROM entries WHERE {range} GROUP BY model ORDER BY 2 DESC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ModelRow {
                model: row.get(0)?,
                cost: row.get(1)?,
                input_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                cache_creation_tokens: row.get(4)?,
                cache_read_tokens: row.get(5)?,
                total_tokens: row.get(6)?,
                requests: row.get(7)?,
                priced: true,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

/// Per-model breakdown for one session (expandable session detail).
pub fn session_models(
    conn: &Connection,
    agent: &str,
    session_id: &str,
    mode: CostMode,
) -> Result<Vec<ModelRow>, String> {
    let cost = cost_expr(mode);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT model, COALESCE(SUM({cost}),0),
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_creation_5m + cache_creation_1h),0),
                    COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(total_tokens),0), COUNT(*)
             FROM entries WHERE agent = ?1 AND session_id = ?2
             GROUP BY model ORDER BY 2 DESC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![agent, session_id], |row| {
            Ok(ModelRow {
                model: row.get(0)?,
                cost: row.get(1)?,
                input_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                cache_creation_tokens: row.get(4)?,
                cache_read_tokens: row.get(5)?,
                total_tokens: row.get(6)?,
                requests: row.get(7)?,
                priced: true,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub session_id: String,
    pub agent: String,
    pub project: String,
    pub first_ts: i64,
    pub last_ts: i64,
    pub cost: f64,
    pub total_tokens: i64,
    pub requests: i64,
    pub models: String,
}

pub fn sessions(
    conn: &Connection,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    mode: CostMode,
    limit: i64,
) -> Result<Vec<SessionRow>, String> {
    let cost = cost_expr(mode);
    let range = range_clause(since_ms, until_ms);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT session_id, agent, project, MIN(timestamp_ms), MAX(timestamp_ms),
                    COALESCE(SUM({cost}),0), COALESCE(SUM(total_tokens),0), COUNT(*),
                    GROUP_CONCAT(DISTINCT model)
             FROM entries WHERE {range}
             GROUP BY agent, session_id ORDER BY MAX(timestamp_ms) DESC LIMIT ?1"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![limit], |row| {
            Ok(SessionRow {
                session_id: row.get(0)?,
                agent: row.get(1)?,
                project: row.get(2)?,
                first_ts: row.get(3)?,
                last_ts: row.get(4)?,
                cost: row.get(5)?,
                total_tokens: row.get(6)?,
                requests: row.get(7)?,
                models: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRow {
    pub project: String,
    pub cost: f64,
    pub total_tokens: i64,
    pub requests: i64,
    pub sessions: i64,
}

pub fn projects(
    conn: &Connection,
    since_ms: Option<i64>,
    until_ms: Option<i64>,
    mode: CostMode,
    limit: i64,
) -> Result<Vec<ProjectRow>, String> {
    let cost = cost_expr(mode);
    let range = range_clause(since_ms, until_ms);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT project, COALESCE(SUM({cost}),0), COALESCE(SUM(total_tokens),0), COUNT(*),
                    COUNT(DISTINCT agent || ':' || session_id)
             FROM entries WHERE {range} GROUP BY project ORDER BY 2 DESC LIMIT ?1"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![limit], |row| {
            Ok(ProjectRow {
                project: row.get(0)?,
                cost: row.get(1)?,
                total_tokens: row.get(2)?,
                requests: row.get(3)?,
                sessions: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub id: String,
    /// Blocks are per agent: each tool has its own 5-hour usage window.
    pub agent: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub actual_end_ms: Option<i64>,
    pub is_active: bool,
    pub is_gap: bool,
    pub cost: f64,
    pub total_tokens: i64,
    pub requests: i64,
    pub models: Vec<String>,
    /// tokens per minute over the block's elapsed time (active blocks only)
    pub burn_rate_tpm: Option<f64>,
    pub burn_rate_cost_per_hour: Option<f64>,
}

/// One entry as the block builder sees it: (timestamp, cost, tokens, model).
type BlockEntry = (i64, f64, i64, String);

/// 5-hour billing blocks per agent (or for one `agent`), most recent
/// first. Claude's and Codex's usage windows are separate, so mixing
/// their entries into one timeline would misplace both.
pub fn blocks(
    conn: &Connection,
    since_ms: Option<i64>,
    mode: CostMode,
    session_duration_hours: f64,
    agent: Option<&str>,
) -> Result<Vec<Block>, String> {
    let cost = cost_expr(mode);
    let range = range_clause(since_ms, None);
    let mut stmt = conn
        .prepare(&format!(
            "SELECT agent, timestamp_ms, {cost}, total_tokens, model FROM entries
             WHERE {range} AND (?1 IS NULL OR agent = ?1) ORDER BY timestamp_ms ASC"
        ))
        .map_err(|e| e.to_string())?;
    let mut by_agent: std::collections::BTreeMap<String, Vec<BlockEntry>> = Default::default();
    let rows = stmt
        .query_map(params![agent], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?),
            ))
        })
        .map_err(|e| e.to_string())?;
    for (a, entry) in rows.filter_map(Result::ok) {
        by_agent.entry(a).or_default().push(entry);
    }

    let duration_ms = (session_duration_hours * MILLIS_PER_HOUR as f64) as i64;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut all: Vec<Block> = by_agent
        .iter()
        .flat_map(|(a, entries)| blocks_for_agent(a, entries, duration_ms, now_ms))
        .collect();
    all.sort_by(|x, y| y.start_ms.cmp(&x.start_ms).then_with(|| x.agent.cmp(&y.agent)));
    Ok(all)
}

/// Ported from ccusage blocks.rs: sort by time, split when gap-from-start
/// or gap-from-last exceeds the session duration, floor block starts to
/// the hour, insert gap blocks, and mark the trailing block active when
/// still inside its window. Returns oldest first.
fn blocks_for_agent(
    agent: &str,
    entries: &[BlockEntry],
    duration_ms: i64,
    now_ms: i64,
) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut current: Vec<&BlockEntry> = Vec::new();
    let mut current_start: Option<i64> = None;

    let floor_to_hour = |ts: i64| (ts / MILLIS_PER_HOUR) * MILLIS_PER_HOUR;
    let block_id = |ts: i64| {
        chrono::Utc
            .timestamp_millis_opt(ts)
            .single()
            .map(|d| format!("{agent}:{}", d.to_rfc3339()))
            .unwrap_or_default()
    };

    let make_block = |start: i64, entries: &[&BlockEntry]| -> Block {
        let end = start + duration_ms;
        let actual_end = entries.last().map(|e| e.0);
        let is_active = actual_end
            .map(|ae| now_ms < end && now_ms - ae < duration_ms)
            .unwrap_or(false);
        let cost_sum: f64 = entries.iter().map(|e| e.1).sum();
        let tokens: i64 = entries.iter().map(|e| e.2).sum();
        let mut models: Vec<String> = entries.iter().map(|e| e.3.clone()).collect();
        models.sort();
        models.dedup();
        let (burn_tpm, burn_cph) = if is_active {
            let elapsed_min = ((now_ms - start) as f64 / 60_000.0).max(1.0);
            (
                Some(tokens as f64 / elapsed_min),
                Some(cost_sum / (elapsed_min / 60.0)),
            )
        } else {
            (None, None)
        };
        Block {
            id: block_id(start),
            agent: agent.to_string(),
            start_ms: start,
            end_ms: end,
            actual_end_ms: actual_end,
            is_active,
            is_gap: false,
            cost: cost_sum,
            total_tokens: tokens,
            requests: entries.len() as i64,
            models,
            burn_rate_tpm: burn_tpm,
            burn_rate_cost_per_hour: burn_cph,
        }
    };

    for entry in entries {
        match current_start {
            None => {
                current_start = Some(floor_to_hour(entry.0));
            }
            Some(start) => {
                let last_ts = current.last().map(|e| e.0).unwrap_or(start);
                let since_start = entry.0 - start;
                let since_last = entry.0 - last_ts;
                if since_start > duration_ms || since_last > duration_ms {
                    blocks.push(make_block(start, &current));
                    if since_last > duration_ms {
                        blocks.push(Block {
                            id: block_id(last_ts),
                            agent: agent.to_string(),
                            start_ms: last_ts,
                            end_ms: entry.0,
                            actual_end_ms: None,
                            is_active: false,
                            is_gap: true,
                            cost: 0.0,
                            total_tokens: 0,
                            requests: 0,
                            models: Vec::new(),
                            burn_rate_tpm: None,
                            burn_rate_cost_per_hour: None,
                        });
                    }
                    current_start = Some(floor_to_hour(entry.0));
                    current.clear();
                }
            }
        }
        current.push(entry);
    }
    if let (Some(start), false) = (current_start, current.is_empty()) {
        blocks.push(make_block(start, &current));
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_built_per_agent() {
        const H: i64 = MILLIS_PER_HOUR;
        let claude: Vec<BlockEntry> = vec![(10 * H, 1.0, 10, "m".into()), (11 * H, 1.0, 10, "m".into())];
        let codex: Vec<BlockEntry> = vec![(12 * H + 5, 2.0, 5, "g".into())];
        let a = blocks_for_agent("claude-code", &claude, 5 * H, 100 * H);
        let b = blocks_for_agent("codex", &codex, 5 * H, 100 * H);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].requests, 2);
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].start_ms, 12 * H);
        assert_eq!(b[0].agent, "codex");
    }
}
