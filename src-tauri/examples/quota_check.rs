//! Diagnostic tool that exercises the quota-aware balancer end-to-end without
//! actually invoking a model.
//!
//! Run: cargo run --example quota_check --release
//!
//! Loads the user's installed models + providers.toml, opens the real state
//! DB, refreshes any stale quotas, then prints — for every multi-provider
//! model — what `select_provider` would pick and the score breakdown.

use agent_runner_lib::balancer::{BalanceEffects, select_provider};
use agent_runner_lib::config::{ProvidersConfig, SessionsConfig, load_models};
use agent_runner_lib::process::OsProcessRunner;
use agent_runner_lib::quota::{InFlight, RefreshOutcome, is_stale, refresh_provider};
use agent_runner_lib::state::StateDb;

struct ExampleBalanceEffects<'a> {
    providers_cfg: &'a ProvidersConfig,
    sessions_cfg: &'a SessionsConfig,
    in_flight: &'a InFlight,
    db: &'a StateDb,
    runner: &'a OsProcessRunner,
}

impl BalanceEffects for ExampleBalanceEffects<'_> {
    fn refresh_quota_if_stale(&self, provider_name: &str) {
        if is_stale(self.db, provider_name) {
            let _ = refresh_provider(
                provider_name,
                self.providers_cfg,
                self.in_flight,
                self.db,
                self.runner,
            );
        }
    }

    fn scan_provider_sessions(&self, provider_name: &str) {
        let _ = agent_runner_lib::sessions::scan_provider_with_runner(
            provider_name,
            self.sessions_cfg,
            self.db,
            self.db,
            self.runner,
        );
    }
}

fn main() {
    let config_dir = dirs::config_dir()
        .expect("no config dir")
        .join("oulipoly-agent-runner");
    let data_dir = dirs::data_dir()
        .expect("no data dir")
        .join("oulipoly-agent-runner");
    let models_dir = config_dir.join("models");
    let providers_path = config_dir.join("providers.toml");
    let db_path = data_dir.join("state.db");

    println!("config dir: {}", config_dir.display());
    println!("db:         {}", db_path.display());
    println!();

    let models = load_models(&models_dir).expect("load models");
    let providers_cfg = ProvidersConfig::load(&providers_path).expect("load providers.toml");
    let sessions_cfg =
        SessionsConfig::load(&config_dir.join("sessions.toml")).expect("load sessions.toml");
    let db = StateDb::open(&db_path).expect("open state db");
    let in_flight = InFlight::new();
    let runner = OsProcessRunner;

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
        let outcome = refresh_provider(name, &providers_cfg, &in_flight, &db, &runner);
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
    let effects = ExampleBalanceEffects {
        providers_cfg: &providers_cfg,
        sessions_cfg: &sessions_cfg,
        in_flight: &in_flight,
        db: &db,
        runner: &runner,
    };
    let mut model_names: Vec<&String> = models.keys().collect();
    model_names.sort();
    for name in model_names {
        let m = &models[name];
        if m.providers.len() <= 1 {
            continue;
        }
        let pick = select_provider(m, &db, Some(&effects));
        let pick_name = &m.providers[pick].name;
        println!("  model {name:<18} -> provider[{pick}] = {pick_name}");
        for (i, p) in m.providers.iter().enumerate() {
            let ws = db.get_windows(&p.name).unwrap_or_default();
            let marker = if i == pick { ">>" } else { "  " };
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
}
