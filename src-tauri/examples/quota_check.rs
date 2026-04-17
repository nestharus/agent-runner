//! Diagnostic tool that exercises the quota-aware balancer end-to-end without
//! actually invoking a model.
//!
//! Run: cargo run --example quota_check --release
//!
//! Loads the user's installed models + providers.toml, opens the real state
//! DB, refreshes any stale quotas, then prints — for every multi-provider
//! model — what `select_provider` would pick and the score breakdown.

use agent_runner_lib::balancer::{BalanceContext, select_provider};
use agent_runner_lib::config::{ProvidersConfig, load_models};
use agent_runner_lib::quota::{InFlight, RefreshOutcome, is_stale, refresh_provider};
use agent_runner_lib::state::StateDb;
use std::path::PathBuf;

fn main() {
    let config_dir = dirs::config_dir()
        .expect("no config dir")
        .join("oulipoly-agent-runner");
    let data_dir = dirs::data_dir()
        .expect("no data dir")
        .join("oulipoly-agent-runner");
    let models_dir = config_dir.join("models");
    let providers_path = config_dir.join("providers.toml");
    // Mirror StateDb::open_default — the CLI writes to data_dir, not config_dir.
    let db_path = data_dir.join("state.db");

    println!("config dir: {}", config_dir.display());
    println!("db:         {}", db_path.display());
    println!();

    let models = load_models(&models_dir).expect("load models");
    let providers_cfg = ProvidersConfig::load(&providers_path).expect("load providers.toml");
    let db = StateDb::open(&db_path).expect("open state db");
    let in_flight = InFlight::new();

    // Collect distinct provider names across all multi-provider models.
    let mut distinct: std::collections::BTreeSet<String> = Default::default();
    for m in models.values() {
        if m.providers.len() > 1 {
            for p in &m.providers {
                distinct.insert(p.name.clone());
            }
        }
    }

    println!("=== Quota refresh ===");
    for name in &distinct {
        let stale_before = is_stale(&db, name);
        let outcome = refresh_provider(name, &providers_cfg, &in_flight, &db);
        let tag = match outcome {
            RefreshOutcome::Updated { used_percent, ref resets_at } => format!(
                "UPDATED used={:.2}% resets={}",
                used_percent * 100.0,
                resets_at.as_deref().unwrap_or("-")
            ),
            RefreshOutcome::NoScript => "NO_SCRIPT (fallback to invocation-count)".into(),
            RefreshOutcome::AlreadyInFlight => "IN_FLIGHT".into(),
            RefreshOutcome::Failed(ref msg) => format!("FAILED: {msg}"),
        };
        println!("  {name:<12} stale={stale_before} -> {tag}");
    }
    println!();

    println!("=== Current cached quotas ===");
    for name in &distinct {
        let q = db.get_quota(name).unwrap();
        match q {
            Some(q) => println!(
                "  {name:<12} used={:>5.2}% calls_since={:>4} last_delta={:?} refreshed={}",
                q.used_percent * 100.0,
                q.calls_since_refresh,
                q.last_delta_percent.zip(q.last_delta_calls),
                q.refreshed_at
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_else(|| "-".into()),
            ),
            None => println!("  {name:<12} <no quota>"),
        }
    }
    println!();

    println!("=== Balancer picks for multi-provider models ===");
    let ctx = BalanceContext {
        providers_cfg: &providers_cfg,
        in_flight: &in_flight,
    };
    let mut model_names: Vec<&String> = models.keys().collect();
    model_names.sort();
    for name in model_names {
        let m = &models[name];
        if m.providers.len() <= 1 {
            continue;
        }
        // Use ctx=None for the decision to avoid re-refreshing; we just
        // refreshed above.
        let pick = select_provider(m, &db, None);
        let pick_name = &m.providers[pick].name;
        let _ = &ctx; // silence unused
        println!("  model {name:<18} -> provider[{pick}] = {pick_name}");
        for (i, p) in m.providers.iter().enumerate() {
            let q = db.get_quota(&p.name).unwrap();
            let marker = if i == pick { ">>" } else { "  " };
            match q {
                Some(q) => println!(
                    "    {marker} [{i}] {:<10} used={:>5.2}% calls_since={}",
                    p.name,
                    q.used_percent * 100.0,
                    q.calls_since_refresh
                ),
                None => {
                    // Fall back display: show invocation_count so we can see
                    // why fallback logic picks what it picks.
                    let rec = db.get_provider(name, i).unwrap();
                    let count = rec.map(|r| r.invocation_count).unwrap_or(0);
                    println!(
                        "    {marker} [{i}] {:<10} <no quota> invocations={}",
                        p.name, count
                    );
                }
            }
        }
    }

    // Touch BalanceContext so a future change can switch `select_provider` to
    // use `Some(&ctx)` here and re-exercise the lazy refresh path.
    let _ = PathBuf::from(".");
    let _ = ctx.providers_cfg.entries.len();
}
