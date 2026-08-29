//! Diagnostic tool that exercises the quota-aware balancer end-to-end without
//! actually invoking a model.
//!
//! Run: cargo run --example quota_check --release
//!
//! Loads the user's installed models + providers.toml, opens the real state
//! DB, refreshes any stale quotas, then prints — for every multi-provider
//! model — what `select_provider` would pick and the score breakdown.

use oulipoly_config::{ProvidersConfig, SessionsConfig, load_models};
use oulipoly_runtime::balancer::{BalanceContext, select_provider};
use oulipoly_runtime::quota::{InFlight, RefreshOutcome, is_stale, refresh_provider};
use oulipoly_state::StateDb;

fn main() {
    let config_dir = oulipoly_state::paths::config_dir().expect("no configured config dir");
    let data_dir = oulipoly_state::paths::data_dir().expect("no configured data dir");
    let models_dir = config_dir.join("models");
    let providers_path = config_dir.join("providers.toml");
    let db_path = data_dir.join("state.db");

    println!("config dir: {}", config_dir.display());
    println!("db:         {}", db_path.display());
    println!();

    let providers_cfg = ProvidersConfig::load(&providers_path).expect("load providers.toml");
    let models = load_models(&models_dir, Some(&providers_cfg)).expect("load models");
    let sessions_cfg =
        SessionsConfig::load(&config_dir.join("sessions.toml")).expect("load sessions.toml");
    let db = StateDb::open(&db_path).expect("open state db");
    let in_flight = InFlight::new();

    // Distinct provider names across multi-provider models.
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
            RefreshOutcome::Updated { ref windows } => {
                let parts: Vec<String> = windows
                    .iter()
                    .map(|w| {
                        let now = chrono::Utc::now();
                        let hours = ((w.resets_at - now).num_seconds() as f64) / 3600.0;
                        format!("{:.1}%@{:.1}h", w.used_percent * 100.0, hours)
                    })
                    .collect();
                format!("UPDATED windows=[{}]", parts.join(", "))
            }
            RefreshOutcome::NoScript => "NO_SCRIPT (fallback to invocation-count)".into(),
            RefreshOutcome::AlreadyInFlight => "IN_FLIGHT".into(),
            RefreshOutcome::Failed(ref msg) => format!("FAILED: {msg}"),
        };
        println!("  {name:<12} stale={stale_before} -> {tag}");
    }
    println!();

    println!("=== Current cached state ===");
    for name in &distinct {
        let q = db.get_quota(name).unwrap();
        let ws = db.get_windows(name).unwrap_or_default();
        match q {
            Some(q) => {
                let parts: Vec<String> = ws
                    .iter()
                    .map(|w| {
                        let now = chrono::Utc::now();
                        let hours = ((w.resets_at - now).num_seconds() as f64) / 3600.0;
                        format!(
                            "[{}]={:.1}%@{:.1}h",
                            w.window_id,
                            w.used_percent * 100.0,
                            hours
                        )
                    })
                    .collect();
                println!(
                    "  {name:<12} {} refreshed={}",
                    parts.join(" "),
                    q.refreshed_at
                        .map(|d| d.to_rfc3339())
                        .unwrap_or_else(|| "-".into()),
                );
            }
            None => println!("  {name:<12} <no quota>"),
        }
    }
    println!();

    println!("=== Balancer picks for multi-provider models ===");
    let ctx = BalanceContext {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
    };
    let mut model_names: Vec<&String> = models.keys().collect();
    model_names.sort();
    for name in model_names {
        let m = &models[name];
        if m.providers.len() <= 1 {
            continue;
        }
        let pick = select_provider(m, &db, Some(&ctx));
        if let Err(err) = &pick {
            println!("  model {name:<18} -> ERROR: {err}");
        }
        let pick = pick.ok();
        let pick_name = pick
            .map(|index| m.providers[index].name.as_str())
            .unwrap_or("-");
        println!(
            "  model {name:<18} -> {}",
            pick.map(|index| format!("provider[{index}] = {pick_name}"))
                .unwrap_or_else(|| "no route".to_string())
        );
        for (i, p) in m.providers.iter().enumerate() {
            let ws = db.get_windows(&p.name).unwrap_or_default();
            let marker = if Some(i) == pick { ">>" } else { "  " };
            if ws.is_empty() {
                let rec = db.get_provider(name, &p.name).unwrap();
                let count = rec.map(|r| r.invocation_count).unwrap_or(0);
                println!(
                    "    {marker} [{i}] {:<10} <no windows> invocations={}",
                    p.name, count
                );
            } else {
                let parts: Vec<String> = ws
                    .iter()
                    .map(|w| {
                        let now = chrono::Utc::now();
                        let hours = ((w.resets_at - now).num_seconds() as f64) / 3600.0;
                        format!(
                            "w{}={:.1}%@{:.1}h",
                            w.window_id,
                            w.used_percent * 100.0,
                            hours
                        )
                    })
                    .collect();
                println!("    {marker} [{i}] {:<10} {}", p.name, parts.join(" "));
            }
        }
    }

    let _ = ctx.providers_cfg.entries.len();
}
