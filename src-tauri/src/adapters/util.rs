//! Shared helpers for agent adapters.

use std::io::BufRead;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Resolve data directories from an env var holding a comma- or
/// OS-path-separator-separated list (`:` on Unix, `;` on Windows),
/// falling back to `default_rel` under $HOME. Only existing directories
/// are returned.
///
/// The OS separator is handled by `std::env::split_paths` rather than a
/// literal `':'` so Windows paths like `C:\Users\me\.gemini` stay intact.
pub fn env_dirs(var: &str, default_rel: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os(var)
        .map(|raw| split_env_paths(&raw.to_string_lossy()))
        .unwrap_or_default();
    if dirs.is_empty() {
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(default_rel));
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs.retain(|p| p.is_dir());
    dirs
}

/// Split an env-var path list on commas and the platform path separator,
/// trimming whitespace and dropping empty parts.
pub fn split_env_paths(raw: &str) -> Vec<PathBuf> {
    raw.split(',')
        .flat_map(|part| std::env::split_paths(part.trim()).collect::<Vec<_>>())
        .filter_map(|p| {
            let trimmed = p.to_string_lossy().trim().to_string();
            (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
        })
        .collect()
}

/// Recursively collect regular files under `dir` accepted by `accept`.
///
/// Uses `DirEntry::file_type`, which does not follow symlinks: symlinked
/// directories are never descended into (a link cycle would otherwise
/// recurse forever, and a link to a sibling tree would yield every file
/// twice) and symlinked files are skipped like in ccusage's walkers. The
/// root `dir` itself may still be a symlink.
pub fn walk_files(dir: &Path, accept: &dyn Fn(&Path) -> bool, files: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            walk_files(&path, accept, files);
        } else if file_type.is_file() && accept(&path) {
            files.push(path);
        }
    }
}

/// Recursively collect files whose extension matches one of `exts`.
pub fn collect_with_ext(dir: &Path, exts: &[&str], files: &mut Vec<PathBuf>) {
    walk_files(
        dir,
        &|path| {
            path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| exts.contains(&e))
        },
        files,
    );
}

/// Stream a file line by line as raw bytes (trailing `\n` / `\r\n`
/// stripped), without loading the whole file into memory. Lines are not
/// required to be UTF-8; `serde_json::from_slice` validates what it
/// needs. Returns `false` when the file cannot be opened.
pub fn for_each_line(path: &Path, mut f: impl FnMut(&[u8])) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut reader = std::io::BufReader::new(file);
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let mut line = buf.as_slice();
                if let Some(rest) = line.strip_suffix(b"\n") {
                    line = rest;
                }
                if let Some(rest) = line.strip_suffix(b"\r") {
                    line = rest;
                }
                f(line);
            }
        }
    }
    true
}

/// First line of a file (without the newline), reading nothing beyond it.
pub fn first_line(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut buf = Vec::new();
    reader.read_until(b'\n', &mut buf).ok()?;
    while matches!(buf.last(), Some(b'\n' | b'\r')) {
        buf.pop();
    }
    Some(buf)
}

/// Byte-substring test used as a cheap pre-filter before JSON parsing
/// (ccusage `LinePrefilter`).
pub fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack.windows(needle.len()).any(|w| w == needle)
}

/// Open another application's SQLite database strictly read-only.
///
/// `SQLITE_OPEN_READ_ONLY` guarantees TokBar never writes to (or takes a
/// write lock on) the agent's database; `NO_MUTEX` because each
/// connection is used by a single scan thread. The busy timeout bounds
/// how long a read waits while the owning app holds a checkpoint or
/// recovery lock instead of failing the scan immediately.
pub fn open_readonly_db(path: &Path) -> Option<rusqlite::Connection> {
    use rusqlite::OpenFlags;
    let conn = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    conn.busy_timeout(std::time::Duration::from_millis(2000)).ok()?;
    Some(conn)
}

/// Whether a table exists in an open SQLite database.
pub fn table_exists(conn: &rusqlite::Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
        [table],
        |_| Ok(()),
    )
    .is_ok()
}

/// Column names of `table` (empty when the table is missing).
pub fn table_columns(conn: &rusqlite::Connection, table: &str) -> Vec<String> {
    // Table names cannot be bound as parameters; callers only pass
    // compile-time literals, but quote defensively anyway.
    let sql = format!("SELECT * FROM \"{}\" LIMIT 0", table.replace('"', "\"\""));
    conn.prepare(&sql)
        .map(|stmt| stmt.column_names().into_iter().map(str::to_string).collect())
        .unwrap_or_default()
}

/// File mtime as epoch milliseconds (0 when unavailable).
pub fn mtime_ms(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Upper bound for a single token count read from a log (~2.8e14).
/// Far above any real request or session total, low enough that adding
/// the handful of per-record fields can never overflow `u64` (a debug
/// build would panic on a malformed `1e30` count otherwise).
pub const MAX_TOKENS: u64 = 1 << 48;

/// Clamp a parsed token count to [`MAX_TOKENS`].
pub fn clamp_tokens(n: u64) -> u64 {
    n.min(MAX_TOKENS)
}

/// Read a token count that may be an integer or a finite float.
pub fn as_u64(v: &Value) -> u64 {
    match v {
        Value::Number(n) => clamp_tokens(
            n.as_u64()
                .or_else(|| {
                    n.as_f64()
                        .filter(|f| f.is_finite() && *f > 0.0)
                        .map(|f| f.trunc() as u64)
                })
                .unwrap_or(0),
        ),
        _ => 0,
    }
}

/// `obj[key]` as u64 via [`as_u64`] (0 when missing).
pub fn get_u64(obj: &Value, key: &str) -> u64 {
    obj.get(key).map(as_u64).unwrap_or(0)
}

/// First non-empty string among `keys`.
pub fn get_str<'a>(obj: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .filter_map(|k| obj.get(*k).and_then(Value::as_str))
        .map(str::trim)
        .find(|s| !s.is_empty())
}

/// RFC 3339 timestamp as epoch milliseconds.
///
/// Kept here (instead of borrowing the Claude adapter's parser) so the
/// non-Claude adapters do not depend on claude.rs internals.
pub fn parse_rfc3339_ms(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts.trim())
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// Interpret an integer timestamp that may be in seconds, milliseconds,
/// microseconds, or nanoseconds (ccusage smart-unit detection).
pub fn smart_unit_ms(n: i64) -> i64 {
    if n >= 100_000_000_000_000_000 {
        n / 1_000_000
    } else if n >= 100_000_000_000_000 {
        n / 1_000
    } else if n >= 100_000_000_000 {
        n
    } else {
        n.saturating_mul(1000)
    }
}

/// Timestamp from a JSON value that is either an integer (s/ms) or an
/// RFC3339 string.
pub fn ts_from_value(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().filter(|x| *x > 0).map(smart_unit_ms),
        Value::String(s) => parse_rfc3339_ms(s),
        _ => None,
    }
}

/// Stable text form of an optional cost for use inside content-based
/// dedup keys: missing, zero, negative and non-finite costs all map to
/// "0" so a copy that only differs by an absent vs. zero cost still
/// collapses.
pub fn cost_key(cost: Option<f64>) -> String {
    match cost {
        Some(c) if c.is_finite() && c > 0.0 => format!("{c}"),
        _ => "0".to_string(),
    }
}

/// Base directory for adapter unit tests that need real files:
/// `$TOKBAR_TEST_DIR` (kept on the external SSD in CI/dev runs), else the
/// OS temp dir. Each caller gets a fresh, unique subdirectory.
#[cfg(test)]
pub fn test_dir(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let base = std::env::var_os("TOKBAR_TEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let unique = format!(
        "{name}-{}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let dir = base.join("tokbar-adapter-tests").join(unique);
    std::fs::create_dir_all(&dir).expect("create test dir");
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_env_paths_handles_commas_and_os_separator() {
        let sep = if cfg!(windows) { ';' } else { ':' };
        let raw = format!(" /a/b ,/c{sep}/d,, ");
        assert_eq!(
            split_env_paths(&raw),
            vec![PathBuf::from("/a/b"), PathBuf::from("/c"), PathBuf::from("/d")]
        );
    }

    #[cfg(windows)]
    #[test]
    fn split_env_paths_keeps_windows_drive_letters() {
        assert_eq!(
            split_env_paths(r"C:\Users\me\.gemini;D:\x"),
            vec![PathBuf::from(r"C:\Users\me\.gemini"), PathBuf::from(r"D:\x")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn walk_files_does_not_follow_symlinked_dirs() {
        let root = test_dir("walk");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/x.jsonl"), "{}").unwrap();
        // A cycle back to the root plus a link to a sibling subtree.
        std::os::unix::fs::symlink(&root, root.join("a/b/loop")).unwrap();
        std::os::unix::fs::symlink(root.join("a"), root.join("alias")).unwrap();
        std::os::unix::fs::symlink(root.join("a/b/x.jsonl"), root.join("link.jsonl")).unwrap();

        let mut files = Vec::new();
        collect_with_ext(&root, &["jsonl"], &mut files);
        assert_eq!(files, vec![root.join("a/b/x.jsonl")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn for_each_line_strips_newlines_and_tolerates_bad_utf8() {
        let root = test_dir("lines");
        let path = root.join("f.jsonl");
        std::fs::write(&path, b"{\"a\":1}\r\n\xff\xfe\n{\"b\":2}").unwrap();
        let mut lines: Vec<Vec<u8>> = Vec::new();
        assert!(for_each_line(&path, |l| lines.push(l.to_vec())));
        assert_eq!(
            lines,
            vec![b"{\"a\":1}".to_vec(), b"\xff\xfe".to_vec(), b"{\"b\":2}".to_vec()]
        );
        assert!(!for_each_line(&root.join("missing"), |_| {}));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn readonly_db_cannot_write() {
        let root = test_dir("ro-db");
        let path = root.join("x.db");
        {
            let rw = rusqlite::Connection::open(&path).unwrap();
            rw.execute_batch("CREATE TABLE t (a INTEGER); INSERT INTO t VALUES (1);")
                .unwrap();
        }
        let conn = open_readonly_db(&path).expect("open read-only");
        assert!(table_exists(&conn, "t"));
        assert!(!table_exists(&conn, "missing"));
        assert_eq!(table_columns(&conn, "t"), vec!["a".to_string()]);
        assert!(conn.execute("INSERT INTO t VALUES (2)", []).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cost_key_normalizes_missing_and_zero() {
        assert_eq!(cost_key(None), "0");
        assert_eq!(cost_key(Some(0.0)), "0");
        assert_eq!(cost_key(Some(-0.0)), "0");
        assert_eq!(cost_key(Some(f64::NAN)), "0");
        assert_eq!(cost_key(Some(0.25)), "0.25");
    }
}
