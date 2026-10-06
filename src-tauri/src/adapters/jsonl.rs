//! Streaming reader for append-only JSONL logs (Claude Code, Codex).
//!
//! Session logs reach hundreds of MB (a 600 MB Codex rollout is mostly
//! tool output and images; its usage lines are under 1 MB), and the
//! active one grows on every turn. Reading from a byte offset with a
//! cheap substring pre-filter keeps a rescan proportional to what was
//! appended and to the lines that can carry usage.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// Stream lines of `path` from byte `offset`. Lines containing any of
/// `needles` (all lines when empty) are passed to `f(line, complete)`;
/// `complete` is false only for a trailing line without its `\n` yet
/// (a writer mid-append). Returns the offset just past the last complete
/// line, i.e. where the next incremental read must resume.
pub fn scan_lines(
    path: &Path,
    offset: u64,
    needles: &[&str],
    mut f: impl FnMut(&str, bool),
) -> std::io::Result<u64> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut consumed = offset;
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        let complete = buf.last() == Some(&b'\n');
        if complete {
            consumed += n as u64;
        }
        if let Ok(line) = std::str::from_utf8(&buf) {
            let line = line.trim();
            if !line.is_empty() && (needles.is_empty() || needles.iter().any(|n| line.contains(n)))
            {
                f(line, complete);
            }
        }
        if !complete {
            break;
        }
    }
    Ok(consumed)
}

/// FNV-1a hash of up to 4 KiB ending at `end`: a cheap check that the
/// bytes already parsed were not rewritten before resuming at `end`.
pub fn tail_hash(path: &Path, end: u64) -> Option<i64> {
    let start = end.saturating_sub(4096);
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::with_capacity((end - start) as usize);
    file.take(end - start).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 != end - start {
        return None;
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    Some(h as i64)
}

/// Readable project name from a working directory: its last component,
/// or "~" for the home directory itself.
pub fn project_from_cwd(cwd: &str) -> Option<String> {
    let cwd = cwd.trim();
    if cwd.is_empty() {
        return None;
    }
    let path = Path::new(cwd);
    if dirs::home_dir().is_some_and(|h| h == path) {
        return Some("~".to_string());
    }
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .or_else(|| Some(cwd.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(name: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::var("TOKBAR_TEST_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("tokbar-jsonl-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn resumes_at_last_complete_line() {
        let p = tmp_file("resume.jsonl", "{\"a\":1}\n{\"usage\":2}\n{\"usage\":3");
        let mut seen = Vec::new();
        let end = scan_lines(&p, 0, &["usage"], |l, c| seen.push((l.to_string(), c))).unwrap();
        assert_eq!(end, 20);
        assert_eq!(
            seen,
            vec![("{\"usage\":2}".to_string(), true), ("{\"usage\":3".to_string(), false)]
        );
        let mut again = Vec::new();
        let end2 = scan_lines(&p, end, &[], |l, _| again.push(l.to_string())).unwrap();
        assert_eq!(end2, end);
        assert_eq!(again, vec!["{\"usage\":3".to_string()]);
        assert!(tail_hash(&p, end).is_some());
    }

    #[test]
    fn project_names_from_cwd() {
        assert_eq!(project_from_cwd("/Users/a/Documents/独立开发/chaogu").as_deref(), Some("chaogu"));
        assert_eq!(project_from_cwd("").as_deref(), None);
    }
}
