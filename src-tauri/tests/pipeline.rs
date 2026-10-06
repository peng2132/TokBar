//! End-to-end pipeline test: scans whatever agent data exists on this
//! machine into a temp SQLite db and prints aggregate results.
//! Set TOKBAR_TEST_DIR to keep the (large) scratch database off the
//! system temp dir.

use tokbar_lib::{aggregate, cost::CostMode, db, pricing::PricingMap};

fn scratch_dir() -> std::path::PathBuf {
    std::env::var("TOKBAR_TEST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

#[test]
fn scan_and_aggregate_real_data() {
    let tmp = scratch_dir().join("tokbar-test.db");
    let _ = std::fs::remove_file(&tmp);
    let conn = std::sync::Mutex::new(db::open(&tmp).expect("open db"));
    let pricing = PricingMap::load(None);

    let stats = db::scan_all(&conn, &pricing, |_, _| {}).expect("scan");
    // A second scan only touches files written in the meantime (agents
    // running on this machine), never the bulk of unchanged history.
    let again = db::scan_all(&conn, &pricing, |_, _| {}).expect("rescan");
    println!("rescan: {} files parsed in {} ms", again.files_parsed, again.duration_ms);
    assert!(
        again.files_parsed * 10 <= stats.files_parsed.max(10),
        "unchanged files were re-parsed"
    );
    let conn = conn.into_inner().expect("unpoisoned");
    println!(
        "scan: {} files total, {} parsed, {} entries in {} ms",
        stats.files_total, stats.files_parsed, stats.entries_inserted, stats.duration_ms
    );

    let overview = aggregate::overview(&conn, None, None, CostMode::Auto).expect("overview");
    println!(
        "totals: cost=${:.4} tokens={} requests={} sessions={} days={}",
        overview.totals.cost,
        overview.totals.total_tokens,
        overview.totals.requests,
        overview.totals.sessions,
        overview.totals.active_days
    );
    for a in &overview.by_agent {
        println!(
            "  agent={} cost=${:.4} tokens={} requests={}",
            a.agent, a.cost, a.total_tokens, a.requests
        );
    }

    let models = aggregate::models(&conn, None, None, CostMode::Auto).expect("models");
    for m in models.iter().take(12) {
        println!(
            "  model={} cost=${:.4} in={} out={} cacheW={} cacheR={}",
            m.model,
            m.cost,
            m.input_tokens,
            m.output_tokens,
            m.cache_creation_tokens,
            m.cache_read_tokens
        );
    }
    println!("unpriced: {:?}", db::unpriced_models(&conn, &pricing).expect("unpriced"));

    let projects = aggregate::projects(&conn, None, None, CostMode::Auto, 10).expect("projects");
    for p in &projects {
        println!("  project={} cost=${:.2} sessions={}", p.project, p.cost, p.sessions);
    }

    let blocks = aggregate::blocks(&conn, None, CostMode::Auto, 5.0, None).expect("blocks");
    let usage_blocks = blocks.iter().filter(|b| !b.is_gap).count();
    println!("blocks: {} usage blocks", usage_blocks);

    // Cross-check: calculate mode should also produce a sane number.
    let calc = aggregate::overview(&conn, None, None, CostMode::Calculate).expect("calc");
    println!("calculate-mode cost=${:.4}", calc.totals.cost);

    drop(conn);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(scratch_dir().join(format!("tokbar-test.db{suffix}")));
    }
}

#[test]
fn pricing_matches_claude_models() {
    let pricing = PricingMap::load(None);
    for model in [
        "claude-sonnet-4-5-20250929",
        "claude-opus-4-5-20251101",
        "claude-haiku-4-5-20251001",
        "claude-opus-5-5",
        "gpt-5",
    ] {
        let p = pricing.find(model);
        assert!(p.is_some(), "no pricing match for {model}");
        let r = p.unwrap().effective(0);
        assert!(r.input > 0.0, "zero input rate for {model}");
        println!(
            "{model}: input={} output={} cacheW={} cacheR={}",
            r.input, r.output, r.cache_write_5m, r.cache_read
        );
    }
}

/// Model names that cc-switch & co. configure when pointing Claude Code at
/// third-party providers — they land verbatim in `message.model`, so each
/// must resolve to a non-zero price (vendor list rates in BUILTIN_PRICING).
#[test]
fn pricing_covers_provider_switcher_models() {
    let pricing = PricingMap::load(None);
    // Monday 2026-10-05 02:00 UTC: inside DeepSeek's peak window.
    let ts = 1_791_165_600_000;
    // (logged model name, expected input $/token)
    let cases = [
        ("kimi-k2.6", 9.5e-7),
        ("glm-5.1", 1.4e-6),
        ("deepseek-v4-pro", 1.32e-6),
        ("deepseek-v4-flash", 3e-7),
        ("MiniMax-M2.7", 3e-7),
        ("Pro/MiniMaxAI/MiniMax-M2.7", 3e-7),
        ("step-3.5-flash-2603", 1e-7),
        ("LongCat-Flash-Chat", 2e-7),
        ("qwen3.8-max-preview", 2e-6),
    ];
    for (model, want_input) in cases {
        let r = pricing
            .resolve_at(model, ts)
            .unwrap_or_else(|| panic!("{model} not priced"))
            .effective(0);
        assert!(
            (r.input - want_input).abs() < 1e-12,
            "{model}: input {} != {}",
            r.input,
            want_input
        );
        assert!(r.output > 0.0, "{model}: zero output rate");
    }
}
