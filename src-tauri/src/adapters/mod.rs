pub mod amp;
pub mod claude;
pub mod codebuff;
pub mod codex;
pub mod copilot;
pub mod droid;
pub mod gemini;
pub mod goose;
pub mod hermes;
pub mod jsonl;
pub mod kilo;
pub mod kimi;
pub mod openclaw;
pub mod opencode;
pub mod pi;
pub mod qwen;
pub mod util;

use std::path::{Path, PathBuf};

use crate::types::UsageRecord;

/// One supported AI coding agent: where its data lives and how to parse
/// it. `scan_all`, the file watcher, and the settings page all iterate
/// [`ALL`] instead of naming adapters individually.
pub struct AdapterDef {
    pub agent: &'static str,
    pub data_dirs: fn() -> Vec<PathBuf>,
    pub collect_files: fn() -> Vec<PathBuf>,
    pub parse_file: fn(&Path) -> Vec<UsageRecord>,
    /// Resumable parser for append-only logs: parses complete lines from
    /// a byte offset, resuming from the adapter state saved last time, so
    /// a growing session log is never re-read from the start.
    pub parse_append: Option<AppendParser>,
}

/// `(file, start offset, saved state) -> parse result`.
pub type AppendParser = fn(&Path, u64, Option<&str>) -> AppendParse;

/// Result of an incremental parse.
pub struct AppendParse {
    pub records: Vec<UsageRecord>,
    /// Byte offset just past the last complete line consumed.
    pub offset: u64,
    /// Opaque adapter state needed to resume at `offset`.
    pub state: Option<String>,
}

pub const ALL: &[AdapterDef] = &[
    AdapterDef {
        agent: claude::AGENT,
        data_dirs: claude::data_dirs,
        collect_files: claude::collect_files,
        parse_file: claude::parse_file,
        parse_append: Some(claude::parse_append),
    },
    AdapterDef {
        agent: codex::AGENT,
        data_dirs: codex::data_dirs,
        collect_files: codex::collect_files,
        parse_file: codex::parse_file,
        parse_append: Some(codex::parse_append),
    },
    AdapterDef {
        agent: kimi::AGENT,
        data_dirs: kimi::data_dirs,
        collect_files: kimi::collect_files,
        parse_file: kimi::parse_file,
        parse_append: None,
    },
    AdapterDef {
        agent: gemini::AGENT,
        data_dirs: gemini::data_dirs,
        collect_files: gemini::collect_files,
        parse_file: gemini::parse_file,
        parse_append: None,
    },
    AdapterDef {
        agent: opencode::AGENT,
        data_dirs: opencode::data_dirs,
        collect_files: opencode::collect_files,
        parse_file: opencode::parse_file,
        parse_append: None,
    },
    AdapterDef {
        agent: openclaw::AGENT,
        data_dirs: openclaw::data_dirs,
        collect_files: openclaw::collect_files,
        parse_file: openclaw::parse_file,
        parse_append: None,
    },
    AdapterDef {
        agent: copilot::AGENT,
        data_dirs: copilot::data_dirs,
        collect_files: copilot::collect_files,
        parse_file: copilot::parse_file,
        parse_append: None,
    },
    AdapterDef {
        agent: qwen::AGENT,
        data_dirs: qwen::data_dirs,
        collect_files: qwen::collect_files,
        parse_file: qwen::parse_file,
        parse_append: None,
    },
    AdapterDef {
        agent: amp::AGENT,
        data_dirs: amp::data_dirs,
        collect_files: amp::collect_files,
        parse_file: amp::parse_file,
        parse_append: None,
    },
    AdapterDef {
        agent: droid::AGENT,
        data_dirs: droid::data_dirs,
        collect_files: droid::collect_files,
        parse_file: droid::parse_file,
        parse_append: None,
    },
    AdapterDef {
        agent: goose::AGENT,
        data_dirs: goose::data_dirs,
        collect_files: goose::collect_files,
        parse_file: goose::parse_file,
        parse_append: None,
    },
    AdapterDef {
        agent: kilo::AGENT,
        data_dirs: kilo::data_dirs,
        collect_files: kilo::collect_files,
        parse_file: kilo::parse_file,
        parse_append: None,
    },
    AdapterDef {
        agent: codebuff::AGENT,
        data_dirs: codebuff::data_dirs,
        collect_files: codebuff::collect_files,
        parse_file: codebuff::parse_file,
        parse_append: None,
    },
    AdapterDef {
        agent: hermes::AGENT,
        data_dirs: hermes::data_dirs,
        collect_files: hermes::collect_files,
        parse_file: hermes::parse_file,
        parse_append: None,
    },
    AdapterDef {
        agent: pi::AGENT,
        data_dirs: pi::data_dirs,
        collect_files: pi::collect_files,
        parse_file: pi::parse_file,
        parse_append: None,
    },
];

pub fn by_agent(agent: &str) -> Option<&'static AdapterDef> {
    ALL.iter().find(|a| a.agent == agent)
}
