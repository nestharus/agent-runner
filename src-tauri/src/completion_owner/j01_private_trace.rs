//! Private, bounded observation at the fresh-join decision. These files are
//! fixture evidence only; neither their presence nor contents grant admission.
use super::*;
use serde_json::{Value, json};

fn actor(id: &SourceProcessIdentity) -> Value {
    json!({"pid": id.pid, "boot": id.boot_id, "starttime": id.starttime_ticks})
}

fn parent(pid: i64) -> Option<i64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn chain(peer: &SourceProcessIdentity) -> (Vec<Value>, bool) {
    let mut nodes = Vec::new();
    let mut pid = peer.pid;
    let mut complete = false;
    for _ in 0..8 {
        let observed = identity(pid).ok();
        let ppid = parent(pid);
        nodes.push(json!({"actor": observed.as_ref().map(actor), "ppid": ppid}));
        match ppid {
            Some(next) if next > 1 && next != pid => pid = next,
            Some(_) => {
                complete = true;
                break;
            }
            None => break,
        }
    }
    (nodes, complete)
}

pub(super) fn at_fresh_join(
    path: &Path,
    owner: &CompletionDomainOwner,
    peer: &SourceProcessIdentity,
    leases: &ContextLeases,
    roots: &super::super::original_work::RootAuthorities,
    supervisor: &super::super::root_supervisor::RootSupervisor,
) {
    let Some(root) = path.parent().and_then(Path::parent) else {
        return;
    };
    if !root.join("j01-trace-enabled").exists() {
        return;
    }
    let matched = roots.private_matching_contexts(peer);
    let (first, first_complete) = chain(peer);
    let native = supervisor.private_native_activations();
    let (second, second_complete) = chain(peer);
    let stable = first == second
        && first_complete
        && second_complete
        && identity(peer.pid).as_ref() == Ok(peer)
        && identity(owner.guardian_identity.pid).as_ref() == Ok(&owner.guardian_identity)
        && matched.iter().all(|(_, founder, context)| {
            identity(founder.pid).as_ref() == Ok(founder)
                && identity(context.pid).as_ref() == Ok(context)
        })
        && native.iter().all(|(_, worker, _, _)| {
            worker
                .as_ref()
                .is_none_or(|id| identity(id.pid).as_ref() == Ok(id))
        });
    let matched: Vec<_> = matched
        .iter()
        .map(|(index, founder, context)| {
            let (lease, sockets) = leases.private_lease_presence(context);
            json!({"scope_index": index, "founder": actor(founder),
                "context": actor(context), "lease": lease, "local_sockets": sockets})
        })
        .collect();
    let native: Vec<_> = native
        .iter()
        .map(|(attempt, worker, granted, terminal)| {
            json!({"attempt_id": attempt, "worker": worker.as_ref().map(actor),
            "granted": granted, "terminal": terminal,
            "direct_peer": worker.as_ref().is_some_and(|id| {
                first.first().and_then(|node| node["ppid"].as_i64()) == Some(id.pid)
            })})
        })
        .collect();
    let trace = json!({"peer": actor(peer), "guardian": actor(&owner.guardian_identity),
        "matched": matched, "native": native,
        "original_active_roots": supervisor.original_active_root_ids().len(),
        "chain": first, "snapshot": if stable { "stable" } else { "indeterminate" }});
    let evidence = root.join(format!("j01-trace-{}.json", peer.pid));
    if let Ok(bytes) = serde_json::to_vec(&trace) {
        let _ = std::fs::write(&evidence, bytes);
    }
    // The fixture releases this exact peer after reading the trace. A timeout
    // records a failed bounded observation; it never changes admission.
    let release = root.join(format!("j01-release-{}", peer.pid));
    let deadline = Instant::now() + Duration::from_secs(45);
    while !release.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn after_context_release(path: &Path) {
    let Some(root) = path.parent().and_then(Path::parent) else {
        return;
    };
    let hold = root.join("j01-hold-after-release");
    if !hold.exists() {
        return;
    }
    let _ = std::fs::write(root.join("j01-after-release.reached"), b"released\n");
    let deadline = Instant::now() + Duration::from_secs(45);
    while hold.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}
