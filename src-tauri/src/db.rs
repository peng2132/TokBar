use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use chrono::{Local, TimeZone};
use rusqlite::{params, Connection};
use serde::Serialize;

use crate::adapters;
use crate::cost::calculate_cost_with;
use crate::pricing::{ModelPricing, PricingMap};
use crate::types::UsageRecord;

/// Bump when parsing/pricing semantics change so cached entries rebuild.
const SCHEMA_VERSION: i64 = 4;

pub fn open(db_path: &Path) -> Result<Connection, String> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS scanned_files (
           path TEXT PRIMARY KEY,
           agent TEXT NOT NULL,
           mtime_ms INTEGER NOT NULL,
           size INTEGER NOT NULL
         );
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
         CREATE INDEX IF NOT EXISTS idx_entries_session ON entries(agent, session_id);",
    )
    .map_err(|e| e.to_string())?;

    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap_or(0);
    if version != SCHEMA_VERSION {
        conn.execute_batch(&format!(
            "DELETE FROM entries; DELETE FROM scanned_files; PRAGMA user_version = {SCHEMA_VERSION};"
        ))
        .map_err(|e| e.to_string())?;
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
    pub duration_ms: u128,
}

/// A file discovered on disk during the scan, with its stat metadata.
struct FileMeta {
    agent: &'static str,
    path: String,
    file: PathBuf,
    mtime_ms: i64,
    size: i64,
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

/// How many changed files to parse before pausing to write them out in
/// one transaction. Bounds both memory (parsed records held in RAM) and
/// how long the connection lock is held per batch.
const WRITE_CHUNK: usize = 32;

/// Incremental scan: re-parse only files whose mtime/size changed,
/// drop entries for deleted files, dedup via UNIQUE(dedup_key) with
/// "more tokens wins" conflict resolution (ccusage replace strategy).
/// `on_progress(done, total)` is invoked per parsed file.
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
    let mut stats = ScanStats::default();

    // Phase A (lock-free): walk every agent's directories and stat files.
    let mut current: Vec<FileMeta> = Vec::new();
    for a in adapters::ALL.iter() {
        for file in (a.collect_files)() {
            let Ok(meta) = std::fs::metadata(&file) else {
                continue;
            };
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            current.push(FileMeta {
                agent: a.agent,
                path: file.to_string_lossy().to_string(),
                file,
                mtime_ms,
                size: meta.len() as i64,
            });
        }
    }
    stats.files_total = current.len();

    // Phase B (short lock): load the cached scan state in one query
    // (instead of one SELECT per file), diff, and drop deleted files.
    let changed: Vec<usize> = {
        let mut guard = conn.lock().map_err(|e| e.to_string())?;
        let known: HashMap<String, (i64, i64)> = {
            let mut stmt = guard
                .prepare("SELECT path, mtime_ms, size FROM scanned_files")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, (row.get(1)?, row.get(2)?)))
                })
                .map_err(|e| e.to_string())?;
            rows.filter_map(Result::ok).collect()
        };
        let (changed, deleted) = diff_files(&current, &known);
        if !deleted.is_empty() {
            let tx = guard.transaction().map_err(|e| e.to_string())?;
            for gone in &deleted {
                tx.execute("DELETE FROM entries WHERE file_path = ?1", params![gone])
                    .map_err(|e| e.to_string())?;
                tx.execute("DELETE FROM scanned_files WHERE path = ?1", params![gone])
                    .map_err(|e| e.to_string())?;
            }
            tx.commit().map_err(|e| e.to_string())?;
            stats.files_removed = deleted.len();
        }
        changed
    };

    // Phases C+D, chunked: parse a batch of files without the lock
    // (memoizing pricing resolution per distinct model string), then
    // take the lock briefly to write the whole batch in one transaction.
    // One commit per chunk instead of one fsync per file.
    let total_changed = changed.len();
    let mut price_cache: HashMap<String, Option<ModelPricing>> = HashMap::new();
    let mut done = 0usize;
    for chunk in changed.chunks(WRITE_CHUNK) {
        let mut batch: Vec<(&FileMeta, Vec<(UsageRecord, f64)>)> =
            Vec::with_capacity(chunk.len());
        for &idx in chunk {
            let fm = &current[idx];
            let records = adapters::by_agent(fm.agent)
                .map(|a| (a.parse_file)(&fm.file))
                .unwrap_or_default();
            let priced = records
                .into_iter()
                .map(|rec| {
                    let cost = match price_cache.get(&rec.model) {
                        Some(p) => p.as_ref().map_or(0.0, |p| calculate_cost_with(&rec, p)),
                        None => {
                            let resolved = pricing.resolve(&rec.model);
                            let cost =
                                resolved.as_ref().map_or(0.0, |p| calculate_cost_with(&rec, p));
                            price_cache.insert(rec.model.clone(), resolved);
                            cost
                        }
                    };
                    (rec, cost)
                })
                .collect();
            batch.push((fm, priced));
            done += 1;
            on_progress(done, total_changed);
        }

        let mut guard = conn.lock().map_err(|e| e.to_string())?;
        let tx = guard.transaction().map_err(|e| e.to_string())?;
        for (fm, priced) in &batch {
            tx.execute("DELETE FROM entries WHERE file_path = ?1", params![fm.path])
                .map_err(|e| e.to_string())?;
            for (rec, cost) in priced {
                stats.entries_inserted += insert_record(&tx, &fm.path, rec, *cost)?;
            }
            tx.execute(
                "INSERT INTO scanned_files (path, agent, mtime_ms, size) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(path) DO UPDATE SET mtime_ms = excluded.mtime_ms, size = excluded.size",
                params![fm.path, fm.agent, fm.mtime_ms, fm.size],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        stats.files_parsed += batch.len();
    }

    stats.duration_ms = started.elapsed().as_millis();
    Ok(stats)
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
    let n = tx
        .execute(
            "INSERT INTO entries (
               dedup_key, file_path, agent, project, session_id, timestamp_ms, date_local,
               model, input_tokens, output_tokens, cache_creation_5m, cache_creation_1h,
               cache_read_tokens, total_tokens, cost_usd, calculated_cost
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
             ON CONFLICT(dedup_key) DO UPDATE SET
               file_path = excluded.file_path,
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
                rec.dedup_key,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(path: &str, mtime_ms: i64, size: i64) -> FileMeta {
        FileMeta {
            agent: "claude-code",
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
}
