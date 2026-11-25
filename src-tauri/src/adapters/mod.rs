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
pub(crate) mod magpie;
pub mod openclaw;
pub mod opencode;
pub mod pi;
pub mod qwen;
pub mod util;

use std::path::{Path, PathBuf};

use crate::performance::PerformanceSample;
use crate::pricing::ServiceTier;
use crate::types::UsageRecord;

/// One supported AI coding agent: where its data lives and how to parse
/// it. `scan_all`, the file watcher, and the settings page all iterate
/// [`ALL`] instead of naming adapters individually.
pub struct AdapterDef {
    pub agent: &'static str,
    pub data_dirs: fn() -> Vec<PathBuf>,
    pub collect_files: fn() -> Vec<PathBuf>,
    pub parser: Parser,
}

pub enum Parser {
    File(fn(&Path) -> Vec<UsageRecord>),
    /// Resumable parser for append-only logs: parses complete lines from
    /// a byte offset, resuming from the adapter state saved last time, so
    /// a growing session log is never re-read from the start.
    Append(fn(&Path, u64, Option<&str>) -> AppendParse),
}

/// Result of an incremental parse.
pub struct AppendParse {
    pub records: Vec<UsageRecord>,
    pub performance: Vec<PerformanceSample>,
    pub(crate) request_pricing: Vec<RequestPricing>,
    /// Byte offset just past the last complete line consumed.
    pub offset: u64,
    /// Opaque adapter state needed to resume at `offset`.
    pub state: Option<String>,
}

pub(crate) enum RequestPricing {
    CodexResponse { usage_key: String, response_id: String },
    MagpieTier {
        response_id: String,
        started_ms: i64,
        tier: ServiceTier,
        returned: bool,
    },
}

pub const ALL: &[AdapterDef] = &[
    AdapterDef {
        agent: claude::AGENT,
        data_dirs: claude::data_dirs,
        collect_files: claude::collect_files,
        parser: Parser::Append(claude::parse_append),
    },
    AdapterDef {
        agent: codex::AGENT,
        data_dirs: codex::data_dirs,
        collect_files: codex::collect_files,
        parser: Parser::Append(codex::parse_append),
    },
    AdapterDef {
        agent: magpie::SOURCE,
        data_dirs: magpie::data_dirs,
        collect_files: magpie::collect_files,
        parser: Parser::Append(magpie::parse_append),
    },
    AdapterDef {
        agent: kimi::AGENT,
        data_dirs: kimi::data_dirs,
        collect_files: kimi::collect_files,
        parser: Parser::File(kimi::parse_file),
    },
    AdapterDef {
        agent: gemini::AGENT,
        data_dirs: gemini::data_dirs,
        collect_files: gemini::collect_files,
        parser: Parser::File(gemini::parse_file),
    },
    AdapterDef {
        agent: opencode::AGENT,
        data_dirs: opencode::data_dirs,
        collect_files: opencode::collect_files,
        parser: Parser::File(opencode::parse_file),
    },
    AdapterDef {
        agent: openclaw::AGENT,
        data_dirs: openclaw::data_dirs,
        collect_files: openclaw::collect_files,
        parser: Parser::File(openclaw::parse_file),
    },
    AdapterDef {
        agent: copilot::AGENT,
        data_dirs: copilot::data_dirs,
        collect_files: copilot::collect_files,
        parser: Parser::File(copilot::parse_file),
    },
    AdapterDef {
        agent: qwen::AGENT,
        data_dirs: qwen::data_dirs,
        collect_files: qwen::collect_files,
        parser: Parser::File(qwen::parse_file),
    },
    AdapterDef {
        agent: amp::AGENT,
        data_dirs: amp::data_dirs,
        collect_files: amp::collect_files,
        parser: Parser::File(amp::parse_file),
    },
    AdapterDef {
        agent: droid::AGENT,
        data_dirs: droid::data_dirs,
        collect_files: droid::collect_files,
        parser: Parser::File(droid::parse_file),
    },
    AdapterDef {
        agent: goose::AGENT,
        data_dirs: goose::data_dirs,
        collect_files: goose::collect_files,
        parser: Parser::File(goose::parse_file),
    },
    AdapterDef {
        agent: kilo::AGENT,
        data_dirs: kilo::data_dirs,
        collect_files: kilo::collect_files,
        parser: Parser::File(kilo::parse_file),
    },
    AdapterDef {
        agent: codebuff::AGENT,
        data_dirs: codebuff::data_dirs,
        collect_files: codebuff::collect_files,
        parser: Parser::File(codebuff::parse_file),
    },
    AdapterDef {
        agent: hermes::AGENT,
        data_dirs: hermes::data_dirs,
        collect_files: hermes::collect_files,
        parser: Parser::File(hermes::parse_file),
    },
    AdapterDef {
        agent: pi::AGENT,
        data_dirs: pi::data_dirs,
        collect_files: pi::collect_files,
        parser: Parser::File(pi::parse_file),
    },
];

pub fn by_agent(agent: &str) -> Option<&'static AdapterDef> {
    ALL.iter().find(|a| a.agent == agent)
}
