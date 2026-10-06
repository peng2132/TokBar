use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

use chrono::{Local, TimeZone};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::adapters::{self, jsonl, AdapterDef};
use crate::cost::{calculate_cost, calculate_cost_with, TokenCounts};
use crate::pricing::PricingMap;
use crate::types::UsageRecord;

/// Bump when parsing semantics change so every file is re-parsed. Stored
/// usage is kept across bumps: rows of files that still exist are
/// replaced by the re-parse, rows of files their agent has since deleted
/// stay as history. (Pricing changes need no bump — see `reprice`.)
const SCHEMA_VERSION: i64 = 4;

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
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )
    .map_err(|e| e.to_string())?;

    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap_or(0);
    if version != SCHEMA_VERSION {
        // Forget scan state (forcing a full re-parse) but keep entries.
        conn.execute_batch(&format!(
            "DROP TABLE IF EXISTS scanned_files;
             DELETE FROM meta WHERE key = 'pricing_fingerprint';
             PRAGMA user_version = {SCHEMA_VERSION};"
        ))
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
                tx.execute("DELETE FROM entries WHERE file_path = ?1", params![p.fm.path])
                    .map_err(|e| e.to_string())?;
            }
            for (rec, cost) in &p.records {
                stats.entries_inserted += insert_record(&tx, &p.fm.path, rec, *cost)?;
            }
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

    stats.duration_ms = started.elapsed().as_millis();
    Ok(stats)
}

/// Parse one changed file: incrementally from where the last parse
/// stopped when the adapter supports it and the parsed prefix is
/// unchanged, otherwise in full. Adapter panics are contained so one
/// malformed file cannot take the scanner down.
fn parse_one<'a>(fm: &'a FileMeta, known: Option<Known>, pricing: &PricingMap) -> Option<Parsed<'a>> {
    let resume = match (fm.adapter.parse_append, known) {
        (Some(_), Some(k))
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
        match fm.adapter.parse_append {
            Some(parse_append) => {
                let (offset, state) = resume
                    .as_ref()
                    .map(|k| (k.parsed_bytes as u64, k.state.as_deref()))
                    .unwrap_or((0, None));
                let out = parse_append(&fm.file, offset, state);
                let tail = jsonl::tail_hash(&fm.file, out.offset);
                (out.records, out.offset as i64, tail, out.state)
            }
            None => ((fm.adapter.parse_file)(&fm.file), fm.size, None, None),
        }
    }));
    let Ok((records, parsed_bytes, tail_hash, state)) = outcome else {
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
        parsed_bytes,
        tail_hash,
        state,
    })
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
                "SELECT id, model, timestamp_ms, input_tokens, output_tokens, cache_creation_5m,
                        cache_creation_1h, cache_read_tokens, calculated_cost FROM entries",
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
    fn reprices_stored_entries_when_prices_change() {
        let conn = Mutex::new(Connection::open_in_memory().unwrap());
        {
            let mut g = lock(&conn);
            g.execute_batch(
                "CREATE TABLE entries (id INTEGER PRIMARY KEY, dedup_key TEXT UNIQUE,
                   file_path TEXT, agent TEXT, project TEXT, session_id TEXT, timestamp_ms INTEGER,
                   date_local TEXT, model TEXT, input_tokens INTEGER, output_tokens INTEGER,
                   cache_creation_5m INTEGER, cache_creation_1h INTEGER, cache_read_tokens INTEGER,
                   total_tokens INTEGER, cost_usd REAL, calculated_cost REAL NOT NULL);
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )
            .unwrap();
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
