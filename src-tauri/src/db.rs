use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

use chrono::{Local, TimeZone};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::adapters::{self, jsonl, AdapterDef, Parser, RequestPricing};
use crate::cost::{calculate_cost, calculate_cost_with, TokenCounts};
use crate::performance::PerformanceSample;
use crate::pricing::{PricingMap, ServiceTier};
use crate::types::UsageRecord;

/// Bump when the schema or parser semantics change. Keep stored usage so
/// entries from logs an agent has since deleted stay available as history.
const SCHEMA_VERSION: i64 = 6;
/// Re-read the metadata in an append-only file while keeping its usage cursor.
const METADATA_BACKFILL_MTIME: i64 = -1;

/// Lock a mutex even if a panicking thread poisoned it: every write here
/// is transactional, so the data behind it is still consistent.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn open(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS entries (
           id INTEGER PRIMARY KEY,
           dedup_key TEXT UNIQUE,
           file_path TEXT NOT NULL,
           agent TEXT NOT NULL,
           project TEXT NOT NULL,
           session_id TEXT NOT NULL,
           timestamp_ms INTEGER NOT NULL,
           date_local TEXT NOT NULL,
           model TEXT NOT NULL,
           input_tokens INTEGER NOT NULL,
           output_tokens INTEGER NOT NULL,
           cache_creation_5m INTEGER NOT NULL,
           cache_creation_1h INTEGER NOT NULL,
           cache_read_tokens INTEGER NOT NULL,
           total_tokens INTEGER NOT NULL,
           cost_usd REAL,
           calculated_cost REAL NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_entries_ts ON entries(timestamp_ms);
         CREATE INDEX IF NOT EXISTS idx_entries_date ON entries(date_local);
         CREATE INDEX IF NOT EXISTS idx_entries_file ON entries(file_path);
         CREATE INDEX IF NOT EXISTS idx_entries_session ON entries(agent, session_id);
         CREATE TABLE IF NOT EXISTS performance_samples (
           source TEXT NOT NULL,
           sample_key TEXT NOT NULL,
           file_path TEXT NOT NULL,
           timestamp_ms INTEGER NOT NULL,
           model TEXT NOT NULL,
           provider TEXT NOT NULL,
           output_tps REAL,
           first_token_ms INTEGER,
           PRIMARY KEY (source, sample_key)
         );
         CREATE INDEX IF NOT EXISTS idx_performance_file ON performance_samples(file_path);
         CREATE INDEX IF NOT EXISTS idx_performance_ts ON performance_samples(timestamp_ms);
         CREATE INDEX IF NOT EXISTS idx_performance_latest ON performance_samples(timestamp_ms)
           WHERE output_tps IS NOT NULL;
         CREATE TABLE IF NOT EXISTS codex_responses (
           usage_key TEXT PRIMARY KEY,
           response_id TEXT NOT NULL,
           applied_entry_id INTEGER,
           applied_tier TEXT
         );
         CREATE TABLE IF NOT EXISTS magpie_request_tiers (
           response_id TEXT PRIMARY KEY,
           started_ms INTEGER NOT NULL,
           tier TEXT NOT NULL,
           returned INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )
    .map_err(|e| e.to_string())?;

    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap_or(0);
    if version < 4 {
        // Forget scan state (forcing a full re-parse) but keep entries.
        conn.execute_batch(
            "DROP TABLE IF EXISTS scanned_files;
             DELETE FROM meta WHERE key = 'pricing_fingerprint';",
        )
        .map_err(|e| e.to_string())?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS scanned_files (
           path TEXT PRIMARY KEY,
           agent TEXT NOT NULL,
           mtime_ms INTEGER NOT NULL,
           size INTEGER NOT NULL,
           -- incremental parsing: bytes consumed, hash of the bytes just
           -- before that offset, and the adapter's resume state
           parsed_bytes INTEGER NOT NULL DEFAULT 0,
           tail_hash INTEGER,
           state TEXT
         );",
    )
    .map_err(|e| e.to_string())?;
    if version < SCHEMA_VERSION {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        // The old Codex identity included a guessed -fast suffix. Keep the
        // estimate on the entry, and use the raw model in its identity so
        // reclassifying the request or moving its log cannot count it twice.
        tx.execute_batch(
            "DELETE FROM entries WHERE id IN (
               SELECT old.id FROM entries old JOIN entries base
                 ON base.dedup_key = replace(old.dedup_key, ':' || old.model || ':',
                   ':' || substr(old.model, 1, length(old.model) - 5) || ':')
                AND base.id <> old.id
               WHERE old.agent = 'codex' AND old.model LIKE '%-fast'
                 AND old.dedup_key LIKE 'codex:%'
             );
             UPDATE entries SET dedup_key = replace(dedup_key, ':' || model || ':',
               ':' || substr(model, 1, length(model) - 5) || ':')
             WHERE agent = 'codex' AND model LIKE '%-fast' AND dedup_key LIKE 'codex:%';",
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE scanned_files SET mtime_ms = ?1 WHERE agent IN (?2, ?3)",
            params![METADATA_BACKFILL_MTIME, adapters::codex::AGENT, adapters::magpie::SOURCE],
        )
        .map_err(|e| e.to_string())?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(conn)
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStats {
    pub files_total: usize,
    pub files_parsed: usize,
    pub files_removed: usize,
    pub entries_inserted: usize,
    /// Stored entries whose cost changed because prices changed.
    pub entries_repriced: usize,
    pub duration_ms: u128,
}

impl ScanStats {
    /// Whether anything visible may have changed.
    pub fn changed(&self) -> bool {
        self.files_parsed > 0 || self.files_removed > 0 || self.entries_repriced > 0
    }
}

/// A file discovered on disk during the scan, with its stat metadata.
struct FileMeta {
    adapter: &'static AdapterDef,
    path: String,
    file: PathBuf,
    mtime_ms: i64,
    size: i64,
}

/// Cached scan state of one file.
struct Known {
    mtime_ms: i64,
    size: i64,
    parsed_bytes: i64,
    tail_hash: Option<i64>,
    state: Option<String>,
}

/// Pure diff of the on-disk file list against the cached scan state:
/// returns indices (into `current`) of changed/new files plus the paths
/// of files that disappeared since the last scan.
fn diff_files(
    current: &[FileMeta],
    known: &HashMap<String, (i64, i64)>,
) -> (Vec<usize>, Vec<String>) {
    let current_paths: HashSet<&str> = current.iter().map(|f| f.path.as_str()).collect();
    let changed = current
        .iter()
        .enumerate()
        .filter(|(_, f)| known.get(&f.path) != Some(&(f.mtime_ms, f.size)))
        .map(|(i, _)| i)
        .collect();
    let deleted = known
        .keys()
        .filter(|p| !current_paths.contains(p.as_str()))
        .cloned()
        .collect();
    (changed, deleted)
}

/// Agents whose files dedup against each other at parse time (Copilot
/// drops OTel rows that a later `session.shutdown` snapshot in another
/// file already covers). When any of their files changes, all of them
/// are re-parsed so those cross-file decisions are re-made.
const CROSS_FILE_AGENTS: &[&str] = &[adapters::copilot::AGENT];

fn expand_cross_file_agents(current: &[FileMeta], changed: &mut Vec<usize>) {
    let touched: HashSet<&str> = changed
        .iter()
        .map(|&i| current[i].adapter.agent)
        .filter(|a| CROSS_FILE_AGENTS.contains(a))
        .collect();
    if touched.is_empty() {
        return;
    }
    let already: HashSet<usize> = changed.iter().copied().collect();
    changed.extend(
        current
            .iter()
            .enumerate()
            .filter(|(i, f)| touched.contains(f.adapter.agent) && !already.contains(i))
            .map(|(i, _)| i),
    );
}

fn mtime_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Stat a source file. SQLite sources in WAL mode (OpenCode, Kilo, ...)
/// write new rows to `<db>-wal` and only touch the main file at
/// checkpoints, so the WAL's size and mtime are folded in.
fn stat_source(file: &Path) -> Option<(i64, i64)> {
    let meta = std::fs::metadata(file).ok()?;
    let mut mtime = mtime_ms(&meta);
    let mut size = meta.len() as i64;
    let is_jsonl = file.extension().is_some_and(|e| e == "jsonl" || e == "json");
    if !is_jsonl {
        let mut wal = file.as_os_str().to_owned();
        wal.push("-wal");
        if let Ok(w) = std::fs::metadata(PathBuf::from(wal)) {
            mtime = mtime.max(mtime_ms(&w));
            size += w.len() as i64;
        }
    }
    Some((mtime, size))
}

/// How many changed files to parse before pausing to write them out in
/// one transaction. Bounds both memory (parsed records held in RAM) and
/// how long the connection lock is held per batch.
const WRITE_CHUNK: usize = 32;

/// Parse output for one file, ready to write.
struct Parsed<'a> {
    fm: &'a FileMeta,
    /// Replace the file's rows (full parse) or add to them (append).
    full: bool,
    records: Vec<(UsageRecord, f64)>,
    performance: Vec<PerformanceSample>,
    request_pricing: Vec<RequestPricing>,
    parsed_bytes: i64,
    tail_hash: Option<i64>,
    state: Option<String>,
}

/// Incremental scan: re-parse only files whose mtime/size changed (and
/// for append-only logs only the bytes appended since last time), dedup
/// via UNIQUE(dedup_key) with "more tokens wins" conflict resolution
/// (ccusage replace strategy). Re-prices stored usage first when prices
/// changed. `on_progress(done, total)` is invoked per parsed file.
///
/// Takes the connection *mutex* rather than the connection: directory
/// walking and file parsing (the slow parts) run without the lock, so
/// UI queries and tray updates stay responsive during a scan. Callers
/// must serialize whole scans via `AppState.scan_lock`.
pub fn scan_all(
    conn: &Mutex<Connection>,
    pricing: &PricingMap,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<ScanStats, String> {
    let started = std::time::Instant::now();
    let mut stats = ScanStats {
        entries_repriced: reprice_if_needed(conn, pricing)?,
        ..Default::default()
    };

    // Phase A (lock-free): walk every agent's directories and stat files.
    let mut current: Vec<FileMeta> = Vec::new();
    for a in adapters::ALL.iter() {
        for file in (a.collect_files)() {
            let Some((mtime_ms, size)) = stat_source(&file) else {
                continue;
            };
            current.push(FileMeta {
                adapter: a,
                path: file.to_string_lossy().to_string(),
                file,
                mtime_ms,
                size,
            });
        }
    }
    stats.files_total = current.len();

    // Phase B (short lock): load the cached scan state in one query,
    // diff, and forget files that disappeared. Their entries are kept:
    // agents prune old logs (Claude Code after 30 days by default), and
    // that history should stay in the dashboard.
    let (changed, mut known) = {
        let guard = lock(conn);
        let known: HashMap<String, Known> = {
            let mut stmt = guard
                .prepare(
                    "SELECT path, mtime_ms, size, parsed_bytes, tail_hash, state FROM scanned_files",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        Known {
                            mtime_ms: row.get(1)?,
                            size: row.get(2)?,
                            parsed_bytes: row.get(3)?,
                            tail_hash: row.get(4)?,
                            state: row.get(5)?,
                        },
                    ))
                })
                .map_err(|e| e.to_string())?;
            rows.filter_map(Result::ok).collect()
        };
        let stamps: HashMap<String, (i64, i64)> = known
            .iter()
            .map(|(p, k)| (p.clone(), (k.mtime_ms, k.size)))
            .collect();
        let (mut changed, deleted) = diff_files(&current, &stamps);
        expand_cross_file_agents(&current, &mut changed);
        if !deleted.is_empty() {
            let mut guard = guard;
            let tx = guard.transaction().map_err(|e| e.to_string())?;
            for gone in &deleted {
                tx.execute("DELETE FROM scanned_files WHERE path = ?1", params![gone])
                    .map_err(|e| e.to_string())?;
            }
            tx.commit().map_err(|e| e.to_string())?;
            stats.files_removed = deleted.len();
        }
        (changed, known)
    };

    // Phases C+D, chunked: parse a batch of files without the lock, then
    // take the lock briefly to write the whole batch in one transaction.
    let total_changed = changed.len();
    let mut done = 0usize;
    for chunk in changed.chunks(WRITE_CHUNK) {
        let mut batch: Vec<Parsed> = Vec::with_capacity(chunk.len());
        for &idx in chunk {
            let fm = &current[idx];
            if let Some(parsed) = parse_one(fm, known.remove(&fm.path), pricing) {
                batch.push(parsed);
            }
            done += 1;
            on_progress(done, total_changed);
        }

        let mut guard = lock(conn);
        let tx = guard.transaction().map_err(|e| e.to_string())?;
        for p in &batch {
            if p.full {
                if p.fm.adapter.agent == adapters::codex::AGENT {
                    tx.execute(
                        "DELETE FROM codex_responses WHERE usage_key IN
                         (SELECT dedup_key FROM entries WHERE file_path = ?1)",
                        params![p.fm.path],
                    )
                    .map_err(|e| e.to_string())?;
                }
                tx.execute("DELETE FROM entries WHERE file_path = ?1", params![p.fm.path])
                    .map_err(|e| e.to_string())?;
                tx.execute(
                    "DELETE FROM performance_samples WHERE file_path = ?1",
                    params![p.fm.path],
                )
                .map_err(|e| e.to_string())?;
            }
            for (rec, cost) in &p.records {
                stats.entries_inserted += insert_record(&tx, &p.fm.path, rec, *cost)?;
            }
            if !p.performance.is_empty() {
                insert_performance(&tx, p.fm.adapter.agent, &p.fm.path, &p.performance)
                    .map_err(|e| e.to_string())?;
            }
            save_request_pricing(&tx, &p.request_pricing).map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO scanned_files (path, agent, mtime_ms, size, parsed_bytes, tail_hash, state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(path) DO UPDATE SET mtime_ms = excluded.mtime_ms,
                   size = excluded.size, parsed_bytes = excluded.parsed_bytes,
                   tail_hash = excluded.tail_hash, state = excluded.state",
                params![
                    p.fm.path,
                    p.fm.adapter.agent,
                    p.fm.mtime_ms,
                    p.fm.size,
                    p.parsed_bytes,
                    p.tail_hash,
                    p.state
                ],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        stats.files_parsed += batch.len();
    }

    stats.entries_repriced += apply_request_pricing(conn, pricing).map_err(|e| e.to_string())?;
    stats.duration_ms = started.elapsed().as_millis();
    Ok(stats)
}

/// Parse one changed file: incrementally from where the last parse
/// stopped when the adapter supports it and the parsed prefix is
/// unchanged, otherwise in full. Adapter panics are contained so one
/// malformed file cannot take the scanner down.
fn parse_one<'a>(fm: &'a FileMeta, known: Option<Known>, pricing: &PricingMap) -> Option<Parsed<'a>> {
    let resume = match (&fm.adapter.parser, known) {
        (Parser::Append(_), Some(k))
            if k.parsed_bytes > 0
                && fm.size >= k.parsed_bytes
                && k.tail_hash.is_some()
                && jsonl::tail_hash(&fm.file, k.parsed_bytes as u64) == k.tail_hash =>
        {
            Some(k)
        }
        _ => None,
    };
    let full = resume.is_none();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        match fm.adapter.parser {
            Parser::Append(parse_append) => {
                let (offset, state) = resume
                    .as_ref()
                    .map(|k| (k.parsed_bytes as u64, k.state.as_deref()))
                    .unwrap_or((0, None));
                let backfill = resume.as_ref().is_some_and(|k| k.mtime_ms == METADATA_BACKFILL_MTIME);
                let out = match (backfill, fm.adapter.agent) {
                    (true, adapters::codex::AGENT) => adapters::codex::backfill_metadata(&fm.file, offset),
                    (true, adapters::magpie::SOURCE) => parse_append(&fm.file, 0, None),
                    _ => parse_append(&fm.file, offset, state),
                };
                let tail = jsonl::tail_hash(&fm.file, out.offset);
                (out.records, out.performance, out.request_pricing, out.offset as i64, tail, out.state)
            }
            Parser::File(parse_file) => (parse_file(&fm.file), Vec::new(), Vec::new(), fm.size, None, None),
        }
    }));
    let Ok((records, performance, request_pricing, parsed_bytes, tail_hash, state)) = outcome else {
        eprintln!("tokbar: parser panicked on {}", fm.path);
        return None;
    };
    let records = records
        .into_iter()
        .map(|rec| {
            let cost = pricing
                .resolve_at(&rec.model, rec.timestamp_ms)
                .map_or(0.0, |p| calculate_cost_with(&rec, &p));
            (rec, cost)
        })
        .collect();
    Some(Parsed {
        fm,
        full,
        records,
        performance,
        request_pricing,
        parsed_bytes,
        tail_hash,
        state,
    })
}

fn save_request_pricing(tx: &rusqlite::Transaction, evidence: &[RequestPricing]) -> rusqlite::Result<()> {
    for item in evidence {
        match item {
            RequestPricing::CodexResponse { usage_key, response_id } => {
                tx.prepare_cached(
                    "INSERT OR IGNORE INTO codex_responses (usage_key, response_id) VALUES (?1, ?2)",
                )?
                .execute(params![usage_key, response_id])?;
            }
            RequestPricing::MagpieTier { response_id, started_ms, tier, returned } => {
                tx.prepare_cached(
                    "INSERT INTO magpie_request_tiers (response_id, started_ms, tier, returned)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(response_id) DO UPDATE SET
                       tier = excluded.tier, returned = excluded.returned
                     WHERE excluded.returned > magpie_request_tiers.returned",
                )?
                .execute(params![response_id, started_ms, tier.as_str(), returned])?;
            }
        }
    }
    Ok(())
}

/// Codex and gateway logs can arrive in either order. Only exact response IDs
/// change an entry's price, and each entry remains tied to its original usage.
fn apply_request_pricing(conn: &Mutex<Connection>, pricing: &PricingMap) -> rusqlite::Result<usize> {
    let pending = {
        let guard = lock(conn);
        let mut stmt = guard.prepare_cached(
            "SELECT e.id, r.usage_key, e.model, g.tier, g.started_ms,
                        e.input_tokens, e.output_tokens, e.cache_creation_5m,
                        e.cache_creation_1h, e.cache_read_tokens
                 FROM codex_responses r
                 JOIN magpie_request_tiers g ON g.response_id = r.response_id
                 JOIN entries e ON e.dedup_key = r.usage_key AND e.agent = 'codex'
                 WHERE r.applied_entry_id IS NOT e.id OR r.applied_tier IS NOT g.tier",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                TokenCounts {
                    input: r.get::<_, i64>(5)? as u64,
                    output: r.get::<_, i64>(6)? as u64,
                    cache_write_5m: r.get::<_, i64>(7)? as u64,
                    cache_write_1h: r.get::<_, i64>(8)? as u64,
                    cache_read: r.get::<_, i64>(9)? as u64,
                },
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut changed = 0;
    for chunk in pending.chunks(5_000) {
        let mut guard = lock(conn);
        let tx = guard.transaction()?;
        for (entry_id, key, model, tier, started, tokens) in chunk {
            let Some(tier) = ServiceTier::parse(tier) else { continue };
            let model = tier.label(model);
            let cost = pricing.resolve_at(&model, *started)
                .map_or(0.0, |rates| calculate_cost(*tokens, &rates));
            changed += tx.prepare_cached(
                "UPDATE entries SET model = ?2, calculated_cost = ?3
                 WHERE id = ?1 AND (model IS NOT ?2 OR calculated_cost IS NOT ?3)",
            )?.execute(params![entry_id, model, cost])?;
            tx.prepare_cached(
                "UPDATE codex_responses SET applied_entry_id = ?2, applied_tier = ?3
                 WHERE usage_key = ?1",
            )?.execute(params![key, entry_id, tier.as_str()])?;
        }
        tx.commit()?;
    }
    Ok(changed)
}

fn insert_performance(
    tx: &rusqlite::Transaction,
    source: &str,
    file_path: &str,
    samples: &[PerformanceSample],
) -> rusqlite::Result<()> {
    let mut stmt = tx.prepare_cached(
        "INSERT INTO performance_samples
           (source, sample_key, file_path, timestamp_ms, model, provider, output_tps, first_token_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(source, sample_key) DO NOTHING",
    )?;
    for sample in samples {
        stmt.execute(params![
            source,
            sample.key,
            file_path,
            sample.timestamp_ms,
            sample.model,
            sample.provider,
            sample.output_tps,
            sample.first_token_ms,
        ])?;
    }
    Ok(())
}

fn insert_record(
    tx: &rusqlite::Transaction,
    file_path: &str,
    rec: &UsageRecord,
    calculated: f64,
) -> Result<usize, String> {
    let date_local = Local
        .timestamp_millis_opt(rec.timestamp_ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".to_string());
    // Every row needs a key so a moved/renamed log never double counts.
    let dedup_key = rec.dedup_key.clone().unwrap_or_else(|| {
        format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{}",
            rec.agent,
            rec.session_id,
            rec.timestamp_ms,
            rec.model,
            rec.input_tokens,
            rec.output_tokens,
            rec.cache_creation_5m,
            rec.cache_creation_1h,
            rec.cache_read_tokens
        )
    });
    let n = tx
        .execute(
            "INSERT INTO entries (
               dedup_key, file_path, agent, project, session_id, timestamp_ms, date_local,
               model, input_tokens, output_tokens, cache_creation_5m, cache_creation_1h,
               cache_read_tokens, total_tokens, cost_usd, calculated_cost
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
             ON CONFLICT(dedup_key) DO UPDATE SET
               file_path = excluded.file_path,
               project = excluded.project,
               input_tokens = excluded.input_tokens,
               output_tokens = excluded.output_tokens,
               cache_creation_5m = excluded.cache_creation_5m,
               cache_creation_1h = excluded.cache_creation_1h,
               cache_read_tokens = excluded.cache_read_tokens,
               total_tokens = excluded.total_tokens,
               cost_usd = excluded.cost_usd,
               calculated_cost = excluded.calculated_cost
             WHERE excluded.total_tokens > entries.total_tokens",
            params![
                dedup_key,
                file_path,
                rec.agent,
                rec.project,
                rec.session_id,
                rec.timestamp_ms,
                date_local,
                rec.model,
                rec.input_tokens as i64,
                rec.output_tokens as i64,
                rec.cache_creation_5m as i64,
                rec.cache_creation_1h as i64,
                rec.cache_read_tokens as i64,
                rec.total_tokens() as i64,
                rec.cost_usd,
                calculated
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(n)
}

/// Costs are stored per entry for fast aggregation, so they must follow
/// price changes: when the pricing fingerprint differs from the one the
/// stored costs were computed with (online refresh, app update with new
/// built-in prices, a model that was unpriced getting a price), every
/// entry is re-priced from its stored token counts and timestamp.
/// Returns how many entries changed cost.
pub fn reprice_if_needed(conn: &Mutex<Connection>, pricing: &PricingMap) -> Result<usize, String> {
    let fingerprint = pricing.fingerprint();
    let rows: Vec<(i64, String, i64, TokenCounts, f64)> = {
        let guard = lock(conn);
        let stored: Option<String> = guard
            .query_row(
                "SELECT value FROM meta WHERE key = 'pricing_fingerprint'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if stored.as_deref() == Some(fingerprint.as_str()) {
            return Ok(0);
        }
        let mut stmt = guard
            .prepare(
                "SELECT e.id, e.model, COALESCE(g.started_ms, e.timestamp_ms), e.input_tokens,
                        e.output_tokens, e.cache_creation_5m, e.cache_creation_1h, e.cache_read_tokens,
                        e.calculated_cost FROM entries e
                 LEFT JOIN codex_responses r ON r.usage_key = e.dedup_key AND r.applied_entry_id = e.id
                 LEFT JOIN magpie_request_tiers g ON g.response_id = r.response_id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    TokenCounts {
                        input: r.get::<_, i64>(3)? as u64,
                        output: r.get::<_, i64>(4)? as u64,
                        cache_write_5m: r.get::<_, i64>(5)? as u64,
                        cache_write_1h: r.get::<_, i64>(6)? as u64,
                        cache_read: r.get::<_, i64>(7)? as u64,
                    },
                    r.get(8)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok).collect()
    };

    // Compute without holding the lock.
    let updates: Vec<(i64, f64)> = rows
        .into_iter()
        .filter_map(|(id, model, ts, tokens, old)| {
            let new = pricing
                .resolve_at(&model, ts)
                .map_or(0.0, |p| calculate_cost(tokens, &p));
            ((new - old).abs() > 1e-12).then_some((id, new))
        })
        .collect();

    for chunk in updates.chunks(5_000) {
        let mut guard = lock(conn);
        let tx = guard.transaction().map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare_cached("UPDATE entries SET calculated_cost = ?2 WHERE id = ?1")
                .map_err(|e| e.to_string())?;
            for (id, cost) in chunk {
                stmt.execute(params![id, cost]).map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    lock(conn)
        .execute(
            "INSERT INTO meta (key, value) VALUES ('pricing_fingerprint', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![fingerprint],
        )
        .map_err(|e| e.to_string())?;
    Ok(updates.len())
}

/// Distinct models with usage but no known price (their cost reads $0).
pub fn unpriced_models(conn: &Connection, pricing: &PricingMap) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT DISTINCT model FROM entries WHERE total_tokens > 0 ORDER BY model")
        .map_err(|e| e.to_string())?;
    let models: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(models
        .into_iter()
        .filter(|m| !pricing.is_priced(m))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(path: &str, mtime_ms: i64, size: i64) -> FileMeta {
        FileMeta {
            adapter: &adapters::ALL[0],
            path: path.to_string(),
            file: PathBuf::from(path),
            mtime_ms,
            size,
        }
    }

    #[test]
    fn diff_detects_new_changed_deleted_and_skips_unchanged() {
        let current = vec![meta("a", 1, 10), meta("b", 2, 20), meta("c", 3, 30)];
        let mut known = HashMap::new();
        known.insert("a".to_string(), (1, 10)); // unchanged -> skipped
        known.insert("b".to_string(), (1, 20)); // mtime changed -> reparse
        known.insert("gone".to_string(), (9, 9)); // vanished -> delete
        let (changed, deleted) = diff_files(&current, &known);
        let changed_paths: Vec<&str> =
            changed.iter().map(|&i| current[i].path.as_str()).collect();
        assert_eq!(changed_paths, vec!["b", "c"]);
        assert_eq!(deleted, vec!["gone".to_string()]);
    }

    #[test]
    fn diff_size_change_alone_triggers_reparse() {
        let current = vec![meta("a", 1, 11)];
        let mut known = HashMap::new();
        known.insert("a".to_string(), (1, 10));
        let (changed, deleted) = diff_files(&current, &known);
        assert_eq!(changed, vec![0]);
        assert!(deleted.is_empty());
    }

    #[test]
    fn diff_empty_cache_marks_everything_changed() {
        let current = vec![meta("a", 1, 10), meta("b", 2, 20)];
        let (changed, deleted) = diff_files(&current, &HashMap::new());
        assert_eq!(changed, vec![0, 1]);
        assert!(deleted.is_empty());
    }

    fn record(key: &str, model: &str, input: u64) -> UsageRecord {
        UsageRecord {
            agent: "claude-code".into(),
            project: "p".into(),
            session_id: "s".into(),
            timestamp_ms: 1_790_000_000_000,
            model: model.into(),
            input_tokens: input,
            output_tokens: 0,
            cache_creation_5m: 0,
            cache_creation_1h: 0,
            cache_read_tokens: 0,
            cost_usd: None,
            dedup_key: Some(key.into()),
        }
    }

    #[test]
    fn codex_reprices_a_legacy_entry_when_the_gateway_returns_a_different_tier() {
        let dir = adapters::util::test_dir("codex-request-pricing");
        let path = dir.join("usage.db");
        let codex = dir.join("session.jsonl");
        std::fs::write(&codex, concat!(
            r#"{"type":"turn_context","payload":{"model":"gpt-6.2-sol"}}"#, "\n",
            r#"{"type":"token_usage_record","payload":{"response_id":"resp_1","usage":{"input_tokens":1000,"cached_input_tokens":250,"output_tokens":120}}}"#, "\n",
            r#"{"timestamp":"2026-11-25T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":250,"output_tokens":120}}}}"#, "\n",
        )).unwrap();
        let parsed = adapters::codex::parse_append(&codex, 0, None);
        let mut old = parsed.records[0].clone();
        old.model.push_str("-fast");
        old.dedup_key = old.dedup_key.map(|key| key.replace(":gpt-6.2-sol:", ":gpt-6.2-sol-fast:"));
        {
            let mut conn = open(&path).unwrap();
            let tx = conn.transaction().unwrap();
            insert_record(&tx, "session.jsonl", &old, 0.00545).unwrap();
            tx.pragma_update(None, "user_version", 5).unwrap();
            tx.commit().unwrap();
        }

        let conn = Mutex::new(open(&path).unwrap());
        let prices = PricingMap::load(None);
        let gateway = dir.join("magpie.jsonl");
        let sent = r#"{"t":"2026-11-25T00:00:00Z","agent":"codex","status":200,"response_id":"resp_1","tier_sent":"ultrafast"}"#;
        let returned = r#"{"t":"2026-11-25T00:00:00Z","agent":"codex","status":200,"response_id":"resp_1","tier_sent":"ultrafast","tier_served":"default"}"#;
        std::fs::write(&gateway, format!("{sent}\n")).unwrap();
        let first = adapters::magpie::parse_append(&gateway, 0, None);
        let save = |rows: &[RequestPricing]| {
            let mut guard = lock(&conn);
            let tx = guard.transaction().unwrap();
            save_request_pricing(&tx, rows).unwrap();
            tx.commit().unwrap();
        };
        save(&first.request_pricing);
        assert_eq!(apply_request_pricing(&conn, &prices).unwrap(), 0);
        save(&parsed.request_pricing);
        assert_eq!(apply_request_pricing(&conn, &prices).unwrap(), 1);
        let overview = || crate::aggregate::overview(&lock(&conn), None, None, crate::cost::CostMode::Calculate).unwrap();
        let before = overview();
        assert_eq!(before.totals.requests, 1);
        assert_eq!(before.codex_pricing.sent, 1);
        assert!((before.totals.cost - 0.008175).abs() < 1e-12);

        std::fs::write(&gateway, format!("{sent}\n{returned}\n{sent}\n")).unwrap();
        let update = adapters::magpie::parse_append(&gateway, first.offset, None);
        save(&update.request_pricing);
        assert_eq!(apply_request_pricing(&conn, &prices).unwrap(), 1);
        assert_eq!(apply_request_pricing(&conn, &prices).unwrap(), 0);
        let after = overview();
        assert_eq!(after.totals.requests, 1);
        assert_eq!(after.codex_pricing.returned, 1);
        assert!((after.totals.cost - 0.002725).abs() < 1e-12);
    }

    #[test]
    fn performance_backfill_keeps_usage_and_groups_distinct_samples() {
        let path = adapters::util::test_dir("performance-migration").join("usage.db");
        {
            let mut old = open(&path).unwrap();
            let tx = old.transaction().unwrap();
            insert_record(&tx, "archived.jsonl", &record("old", "model-a", 150), 1.5).unwrap();
            tx.execute_batch(
                "DROP TABLE performance_samples;
                 PRAGMA user_version = 4;
                 INSERT INTO scanned_files (path, agent, mtime_ms, size, parsed_bytes)
                 VALUES ('session.jsonl', 'codex', 1, 100, 90),
                        ('claude.jsonl', 'claude-code', 2, 200, 180);",
            )
            .unwrap();
            tx.commit().unwrap();
        }

        let mut conn = open(&path).unwrap();
        for (agent, expected) in [("codex", (METADATA_BACKFILL_MTIME, 90)), ("claude-code", (2, 180))] {
            let stamp: (i64, i64) = conn
                .query_row(
                    "SELECT mtime_ms, parsed_bytes FROM scanned_files WHERE agent = ?1",
                    [agent],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(stamp, expected);
        }
        let sample = |key: &str, timestamp_ms, output_tps, first_token_ms| PerformanceSample {
            key: key.to_string(),
            timestamp_ms,
            model: "model-a".to_string(),
            provider: "provider-a".to_string(),
            output_tps,
            first_token_ms,
        };
        let tx = conn.transaction().unwrap();
        let samples = [
            sample("one", 1000, Some(30.0), Some(300)),
            sample("two", 2000, Some(50.0), Some(900)),
            sample("three", 3000, None, Some(1000)),
        ];
        insert_performance(&tx, "magpie", "usage.jsonl", &samples).unwrap();
        insert_performance(&tx, "magpie", "usage.jsonl", &samples).unwrap();
        insert_performance(&tx, "codex", "session.jsonl", &samples[..1]).unwrap();
        tx.commit().unwrap();

        let groups = crate::performance::models(&conn, Some(1000), Some(3000)).unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].source, "magpie");
        assert_eq!(groups[0].output_tps, Some(40.0));
        assert_eq!(groups[0].first_token_ms, Some(600.0));
        assert_eq!(
            (groups[0].output_samples, groups[0].first_token_samples),
            (2, 2)
        );
        assert_eq!(groups[1].output_samples, 1);
        assert_eq!(
            crate::performance::latest(&conn).unwrap().unwrap().timestamp_ms,
            2000
        );

        let usage =
            crate::aggregate::overview(&conn, None, None, crate::cost::CostMode::Auto).unwrap();
        assert_eq!((usage.totals.requests, usage.totals.total_tokens), (1, 150));
        assert_eq!(usage.totals.cost, 1.5);
    }

    #[test]
    fn reprices_stored_entries_when_prices_change() {
        let conn = Mutex::new(open(Path::new(":memory:")).unwrap());
        {
            let mut g = lock(&conn);
            let tx = g.transaction().unwrap();
            // Stored at $0 as if the model had been unpriced at scan time.
            insert_record(&tx, "f", &record("k1", "claude-opus-5-5", 1_000_000), 0.0).unwrap();
            tx.commit().unwrap();
        }
        let pricing = PricingMap::load(None);
        assert_eq!(reprice_if_needed(&conn, &pricing).unwrap(), 1);
        let cost: f64 = lock(&conn)
            .query_row("SELECT calculated_cost FROM entries", [], |r| r.get(0))
            .unwrap();
        assert!((cost - 4.0).abs() < 1e-9);
        // Same fingerprint: nothing to do.
        assert_eq!(reprice_if_needed(&conn, &pricing).unwrap(), 0);
    }
}
