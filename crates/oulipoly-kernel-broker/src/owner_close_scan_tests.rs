//! Focused controls for deferred entries. No installed host or captured State.
use super::*;
use oulipoly_kernel_broker::entry_registry::EntryRecord;
use oulipoly_state::mailbox::EmptyV30BootstrapIdentity;
use std::os::unix::fs::PermissionsExt;

#[test]
fn live_original_candidates_defer_before_state_readback() {
    if std::env::var_os("AGE353_SCAN_TEST_CHILD").is_none() {
        let private_tmp = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("unshare")
            .args(["-Urpfm", "--mount-proc", "/bin/sh", "-c",
                "mount --bind \"$1\" /tmp && exec \"$2\" --exact linux_main::owner_close_scan_tests::live_original_candidates_defer_before_state_readback --nocapture",
                "scan-control"])
            .arg(private_tmp.path())
            .arg(std::env::current_exe().unwrap())
            .env_remove("TMPDIR")
            .env("AGE353_SCAN_TEST_CHILD", "1")
            .status().unwrap();
        assert!(status.success(), "disposable scan control failed");
        return;
    }
    for unavailable in [false, true] {
        for count in [4, 32] {
            let temp = tempfile::tempdir().unwrap();
            let state = temp.path().join("state");
            EmptyV30BootstrapIdentity::bootstrap_at(&state).unwrap();
            let mut roots = RootRegistry::open(&state).unwrap();
            for name in ["entries", "works", "grants", "sources"] {
                fs::create_dir(state.join(name)).unwrap();
                fs::set_permissions(state.join(name), fs::Permissions::from_mode(0o700)).unwrap();
            }
            let actor =
                ProcessStamp::from(&PinnedProcess::open(std::process::id() as i32).unwrap());
            for _ in 0..count {
                let record = EntryRecord {
                    version: 1,
                    root_id: uuid::Uuid::new_v4().to_string(),
                    owner_uid: 0,
                    entry: actor.clone(),
                    prepared_guardian: Some(actor.clone()),
                    domain_id: Some(uuid::Uuid::new_v4().to_string()),
                    supervisor_authority_id: Some(uuid::Uuid::new_v4().to_string()),
                    guardian: Some(actor.clone()),
                    join_consumed: true,
                    joined_child: Some(actor.clone()),
                    prepared_driver: Some(actor.clone()),
                    terminal_settlement: None,
                    offline_close_sha256: None,
                };
                fs::write(
                    state
                        .join("entries")
                        .join(format!("{}.json", record.root_id)),
                    serde_json::to_vec(&record).unwrap(),
                )
                .unwrap();
            }
            let entries = EntryRegistry::open(state.join("entries")).unwrap();
            let works = WorkRegistry::open(state.join("works"), &roots).unwrap();
            let grants = GrantRegistry::open(state.join("grants")).unwrap();
            let sources = SourcePhysicalRegistry::open(state.join("sources")).unwrap();
            let mut sidecar = Some(
                BrokerSidecar::open_existing(&state.join("sidecar/pid-identity.db"), &state)
                    .unwrap(),
            );
            let fences = AdmissionFences::new(HashSet::new());
            let mut completed = HashSet::new();
            let mut queue = OwnerCloseQueue::default();
            let before: Vec<_> = entries.records().to_vec();
            // A live original cannot close even if its State is temporarily
            // unavailable. This makes an accidental full readback discriminable.
            if unavailable {
                fs::rename(
                    state.join("v30/fresh-provider"),
                    state.join("v30/held-provider"),
                )
                .unwrap();
            }
            let begin = Instant::now();
            let cpu = phase_record::thread_cpu_ns();
            for _ in 0..count {
                let mut scan = phase_record::AdvanceScan::default();
                advance_normal_owners(
                    &state,
                    &mut roots,
                    &entries,
                    &works,
                    &grants,
                    &sources,
                    &mut sidecar,
                    &fences,
                    &mut completed,
                    &mut queue,
                    &mut scan,
                )
                .unwrap();
                assert_eq!(scan.pending, count as u64);
                assert_eq!(scan.readback, 0);
                assert!(!scan.acted);
            }
            println!(
                "count={count} unavailable={unavailable} passes={count} wall_us={} cpu_us={} per_pass_cpu_us={}",
                begin.elapsed().as_micros(),
                (phase_record::thread_cpu_ns() - cpu) / 1000,
                (phase_record::thread_cpu_ns() - cpu) / 1000 / count as u64
            );
            assert_eq!(entries.records(), before.as_slice());
            assert!(completed.is_empty(), "deferral became a false close");
            assert_eq!(queue.len(), count, "every deferred entry stays queued");
        }
    }
}

/// Old roots needing progress keep their revisit bound while new roots
/// arrive on every pass (the cursor counterexample: a cursor over the
/// current vector length never wrapped back to them), and completed roots
/// stop taking turns. A visit costs the whole budget, so each pass reaches
/// exactly one root: the worst case for the bound.
#[test]
fn owner_close_revisit_is_independent_of_later_appends() {
    let mut retained: Vec<String> = (0..4).map(|index| format!("old-{index}")).collect();
    let mut queue = OwnerCloseQueue::default();
    let mut completed = HashSet::new();
    let mut visits: Vec<String> = Vec::new();
    for pass in 0..40 {
        // One new root is retained before every pass.
        retained.push(format!("new-{pass}"));
        let (_, result) = run_owner_close_pass(
            &mut queue,
            retained.iter().map(String::as_str),
            &mut completed,
            std::time::Duration::ZERO,
            |root_id| {
                visits.push(root_id.to_owned());
                Ok(false)
            },
        );
        result.unwrap();
    }
    // Each old root was first visited within the four passes it was queued
    // behind, and again after the roots queued ahead of it at its requeue.
    for index in 0..4 {
        let old = format!("old-{index}");
        let seen: Vec<usize> = visits
            .iter()
            .enumerate()
            .filter(|(_, root)| **root == old)
            .map(|(pass, _)| pass)
            .collect();
        assert_eq!(seen.first(), Some(&index), "{old} first visit: {visits:?}");
        assert!(seen.len() >= 2, "{old} was never revisited: {visits:?}");
        for pair in seen.windows(2) {
            // Requeued behind everything waiting at that moment: at most the
            // roots retained by then, not the roots retained afterwards.
            assert!(pair[1] - pair[0] <= 4 + pair[0] + 1, "{old}: {seen:?}");
        }
    }
    // Every root retained by pass p is visited before any root retained
    // after its own requeue: no later append overtakes a waiting root.
    let mut positions = std::collections::HashMap::new();
    for (pass, root) in visits.iter().enumerate() {
        positions.entry(root.clone()).or_insert(pass);
    }
    for index in 0..39 {
        if let Some(later) = positions.get(&format!("new-{}", index + 1)) {
            let earlier = positions.get(&format!("new-{index}"));
            assert!(
                earlier.is_some_and(|first| first < later),
                "new-{index} overtaken"
            );
        }
    }
}

#[test]
fn completed_roots_leave_and_errors_keep_their_place() {
    let retained: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
    let mut queue = OwnerCloseQueue::default();
    let mut completed = HashSet::new();
    // "a" completes on its first visit; "b" fails, stopping the pass.
    let mut visits = Vec::new();
    let (pending, result) = run_owner_close_pass(
        &mut queue,
        retained.iter().map(String::as_str),
        &mut completed,
        std::time::Duration::from_secs(60),
        |root_id| {
            visits.push(root_id.to_owned());
            match root_id {
                "a" => Ok(true),
                "b" => Err(io::Error::other("readback unavailable")),
                _ => Ok(false),
            }
        },
    );
    assert_eq!(pending, 3);
    assert!(result.is_err());
    assert_eq!(visits, ["a", "b"]);
    assert!(completed.contains("a"));
    // Next pass: "c" (never reached) comes first, then "b"; "a" never again.
    let mut visits = Vec::new();
    let (pending, result) = run_owner_close_pass(
        &mut queue,
        retained.iter().map(String::as_str),
        &mut completed,
        std::time::Duration::from_secs(60),
        |root_id| {
            visits.push(root_id.to_owned());
            Ok(false)
        },
    );
    result.unwrap();
    assert_eq!(pending, 2);
    assert_eq!(visits, ["c", "b"]);
}

/// The emitted selection records rebuild each root's timeline: first
/// admission, every visit (pass, position, outcome, last step), the passes a
/// pending root waited unvisited, and a pass stopped by an error. Read from
/// the actual record file, filtered to this test's roots; other tests in
/// this binary may share the process-global recorder.
#[test]
fn owner_close_selection_records_rebuild_each_roots_timeline() {
    let records_root = tempfile::tempdir().unwrap();
    let path = phase_record::init(records_root.path()).unwrap();
    let roots: Vec<String> = (0..3).map(|_| uuid::Uuid::new_v4().to_string()).collect();
    let (a, b, c) = (roots[0].clone(), roots[1].clone(), roots[2].clone());
    let mut queue = OwnerCloseQueue::default();
    let mut completed = HashSet::new();
    // Pass 1: zero budget, so only `a` is reached; `b` and `c` wait.
    let (pending, result) = run_owner_close_pass(
        &mut queue,
        roots.iter().map(String::as_str),
        &mut completed,
        std::time::Duration::ZERO,
        |_| {
            phase_record::owner_close_step("pid1-echild");
            Ok(false)
        },
    );
    result.unwrap();
    assert_eq!(pending, 3);
    // Pass 2: `b` completes, `c` errors and stops the pass before `a`.
    let (_, result) = run_owner_close_pass(
        &mut queue,
        roots.iter().map(String::as_str),
        &mut completed,
        std::time::Duration::from_secs(60),
        |root| {
            if root == b {
                phase_record::owner_close_step("close-proof");
                phase_record::owner_close_effects(true, true);
                Ok(true)
            } else {
                phase_record::owner_close_step("readback");
                Err(io::Error::other("readback unavailable"))
            }
        },
    );
    assert!(result.is_err());
    // Pass 3: everything left is visited; `b` never again.
    let (pending, result) = run_owner_close_pass(
        &mut queue,
        roots.iter().map(String::as_str),
        &mut completed,
        std::time::Duration::from_secs(60),
        |_| Ok(false),
    );
    result.unwrap();
    assert_eq!(pending, 2);

    let ours = |record: &serde_json::Value| {
        record["root"]
            .as_str()
            .is_some_and(|root| roots.iter().any(|ours| ours == root))
    };
    let records: Vec<serde_json::Value> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    // Passes are numbered process-wide; other tests may interleave theirs.
    let first = records
        .iter()
        .find(|record| record["k"] == "oca" && ours(record))
        .map(|record| record["pass"].as_u64().unwrap())
        .unwrap();
    let visits = |root: &str| -> Vec<serde_json::Value> {
        records
            .iter()
            .filter(|record| record["k"] == "ocv" && record["root"] == root)
            .cloned()
            .collect()
    };
    // Each root is admitted once, in the first pass, in retained order.
    let admitted: Vec<_> = records
        .iter()
        .filter(|record| record["k"] == "oca" && ours(record))
        .collect();
    assert_eq!(admitted.len(), 3);
    for (record, root) in admitted.iter().zip(&roots) {
        assert_eq!(record["root"], root.as_str());
        assert_eq!(record["pass"], first);
    }
    // a: visited in passes 1 and 3, pending both times, its step retained.
    let a_visits = visits(&a);
    let b_visits = visits(&b);
    let (second, third) = (
        b_visits[0]["pass"].as_u64().unwrap(),
        a_visits[1]["pass"].as_u64().unwrap(),
    );
    assert!(first < second && second < third);
    assert_eq!(a_visits.len(), 2);
    assert_eq!(a_visits[0]["pass"], first);
    assert_eq!(a_visits[0]["out"], "pending");
    assert_eq!(a_visits[0]["step"], "pid1-echild");
    assert_eq!(a_visits[1]["step"], "", "a visit with no step says so");
    // The revisit interval is the gap between visit starts.
    let revisit = a_visits[1]["from"].as_u64().unwrap() - a_visits[0]["from"].as_u64().unwrap();
    assert!(revisit > 0);
    // b: waited unvisited in pass 1, then completed once with its effects.
    assert_eq!(b_visits.len(), 1);
    assert_eq!(
        b_visits[0]["pos"], 0,
        "b was queued first behind the requeued a"
    );
    assert_eq!(b_visits[0]["out"], "complete");
    assert_eq!(
        (b_visits[0]["rb"].clone(), b_visits[0]["acted"].clone()),
        (true.into(), true.into())
    );
    // c: waited through pass 1, failed in pass 2 at its step, retried in 3.
    let c_visits = visits(&c);
    assert_eq!(
        c_visits
            .iter()
            .map(|v| v["out"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["error", "pending"]
    );
    assert_eq!(c_visits[0]["step"], "readback");
    assert_eq!(
        (c_visits[0]["pass"].as_u64(), c_visits[1]["pass"].as_u64()),
        (Some(second), Some(third))
    );
    assert_eq!(
        (c_visits[0]["rb"].clone(), c_visits[0]["acted"].clone()),
        (false.into(), false.into())
    );
    // Pass records: pending at start, roots reached, budget and error.
    let pass = |number: u64| -> serde_json::Value {
        records
            .iter()
            .find(|record| record["k"] == "ocp" && record["pass"] == number)
            .cloned()
            .unwrap()
    };
    let (p1, p2, p3) = (pass(first), pass(second), pass(third));
    assert_eq!(
        (p1["pending"].as_u64(), p1["visited"].as_u64()),
        (Some(3), Some(1))
    );
    assert_eq!(
        (p1["budget_spent"].clone(), p1["err"].clone()),
        (true.into(), false.into())
    );
    assert_eq!(
        (p2["visited"].as_u64(), p2["err"].clone()),
        (Some(2), true.into())
    );
    assert_eq!(
        (p3["pending"].as_u64(), p3["visited"].as_u64()),
        (Some(2), Some(2))
    );
    assert_eq!(p3["budget_spent"], false);
    if let Some(export) = std::env::var_os("AGE353_OBS_RECORDS_EXPORT") {
        let lines: String = records
            .iter()
            .filter(|record| {
                ours(record)
                    || (record["k"] == "ocp"
                        && [first, second, third].contains(&record["pass"].as_u64().unwrap()))
            })
            .map(|record| format!("{record}\n"))
            .collect();
        fs::write(export, lines).unwrap();
    }
}
