//! Private scheduling/observations for cold versus cached native cancellation.
//! Never compiled into a normal runner, never changes a cache decision.
use super::ProviderRegistry;
use std::io::Write;
use std::path::PathBuf;

fn root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("AGE360_FAULT_ROOT")?);
    let parent = std::env::var_os("AGE360_FAULT_PARENT_NET")?;
    let net = std::fs::read_link("/proc/self/ns/net").ok()?;
    if net.as_os_str() == parent || !root.join("observe-resume-preflight").exists() {
        return None;
    }
    std::env::args().any(|arg| arg == "resume").then_some(root)
}

pub(super) fn next_generation() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_update(
        std::sync::atomic::Ordering::Relaxed,
        std::sync::atomic::Ordering::Relaxed,
        |value| value.checked_add(1),
    )
    .expect("private registry generation exhausted")
}

pub(super) fn before(registry: &ProviderRegistry, account: &str, attributed: bool) {
    if root().is_none() {
        return;
    }
    observe(registry, account, attributed, "entered");
    let phase = if attributed {
        "resume-attributed-preflight"
    } else {
        "resume-uncustodied-preflight"
    };
    // Deliberately outside the endpoint lock: the cached control lets a real
    // competing ingest operation populate this very registry before admission.
    oulipoly_state::completion_continuation::age360_fault_barrier(phase);
}

pub(super) fn observe(registry: &ProviderRegistry, account: &str, attributed: bool, phase: &str) {
    let Some(root) = root() else { return };
    let stat = std::fs::read_to_string("/proc/self/stat").expect("private process identity");
    let value = serde_json::json!({
        "pid":std::process::id(), "stat":stat,
        "registry":format!("{:p}", registry), "registry_generation":registry.fixture_generation,
        "account":account,
        "attributed":attributed, "phase":phase,
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("resume-preflight.jsonl"))
        .expect("private observation log");
    writeln!(file, "{value}").expect("private observation write");
}
