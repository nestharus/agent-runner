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
