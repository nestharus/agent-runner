//! Diagnostic tool that scans every provider's CLI session logs and
//! reports per-provider turn counts.
//!
//! Run: cargo run --example session_scan --release
//!
//! Loads `~/.config/oulipoly-agent-runner/sessions.toml`, scans all
//! declared paths, ingests new turns into the unified `session_turns` store,
//! then prints — for each provider — total assistant turns ever ingested
//! and turns since the most recent quota refresh (the value the balancer
//! actually projects with).

use agent_runner_config::SessionsConfig;
use agent_runner_session::scan_all;
use agent_runner_state::StateDb;

fn main() {
    let config_dir = dirs::config_dir()
        .expect("no config dir")
        .join("oulipoly-agent-runner");
    let data_dir = dirs::data_dir()
        .expect("no data dir")
        .join("oulipoly-agent-runner");
    let sessions_path = config_dir.join("sessions.toml");
    let db_path = data_dir.join("state.db");

    println!("config:   {}", sessions_path.display());
    println!("db:       {}", db_path.display());
    println!();

    let sessions_cfg = SessionsConfig::load(&sessions_path).expect("load sessions.toml");
    if sessions_cfg.entries.is_empty() {
        println!(
            "No sessions configured. Create {} to enable session-based\n\
             quota tracking. Example contents:\n\n  \
             [claude]\n  \
             type = \"claude-code\"\n  \
             paths = [\"~/.claude/projects/**/*.jsonl\"]\n",
            sessions_path.display()
        );
        return;
    }
    let db = StateDb::open(&db_path).expect("open state db");

    println!("=== Scan ===");
    let reports = scan_all(&sessions_cfg, &db);
    for (name, r) in &reports {
        println!(
            "  {name:<12} script_lines={:<7} new_turns={:<6} errors={}",
            r.script_lines,
            r.new_turns,
            r.errors.len()
        );
        for e in &r.errors {
            println!("    ! {e}");
        }
    }
    println!();

    println!("=== Per-provider turn counts ===");
    println!(
        "  {:<12} {:>10} {:>14}",
        "provider", "all_turns", "since_refresh"
    );
    for (name, _) in &reports {
        let all = db.count_assistant_turns_since(name, None).unwrap_or(0);
        let since = match db.get_quota(name) {
            Ok(Some(q)) => db
                .count_assistant_turns_since(name, q.refreshed_at.as_ref())
                .unwrap_or(0),
            _ => 0,
        };
        println!("  {name:<12} {all:>10} {since:>14}");
    }
}
