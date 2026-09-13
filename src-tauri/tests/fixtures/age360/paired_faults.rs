//! Real paired source barriers. No simulated helper or counterpart modification.
use super::*;

fn json_file(path: &std::path::Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}
fn cancel_source(
    f: &Fixture,
    source: &oulipoly_state::completion_continuation::SourceRegistration,
) {
    f.gate("cancel-source");
    let result = wait(|| json_file(&f.root.path().join("source-cancel-result.json")));
    assert_eq!(
        result["rc"], 0,
        "actual original owner cancellation failed: {result}"
    );
    let value: serde_json::Value =
        serde_json::from_str(result["stdout"].as_str().unwrap()).unwrap();
    println!("actual original owner cancellation receipt={result}");
    assert_eq!(value["handle"], source.handle);
    assert_eq!(value["requested"], true, "{value}");
    println!("actual source cancellation accepted, drain still separate={value}");
}

pub(super) fn before_acceptance(
    f: &Fixture,
    source: &oulipoly_state::completion_continuation::SourceRegistration,
) {
    let fault = match f.case {
        "publication_race" => "after-terminal-metadata",
        "publication_error" | "publication_io_error" => "publication-error",
        "hash_cancel" => "during-output-hash",
        _ => return,
    };
    let path = PathBuf::from(&source.handle_dir);
    let identity = wait(|| json_file(&path.join(format!("fault-{fault}.reached.json"))));
    let original: oulipoly_state::completion_continuation::SourceProcessIdentity =
        serde_json::from_value(identity.clone()).unwrap();
    assert!(current_identity_matches(&original));
    let meta = json_file(&path.join("meta.json")).unwrap();
    assert_eq!(meta["supervisor_pid"], original.pid);
    assert!(!path.join("completion-snapshot-v2.json").exists());
    println!("actual Bash source barrier={fault} identity={identity}");
    if matches!(f.case, "publication_race" | "hash_cancel") {
        cancel_source(f, source);
    } else {
        let frozen = fs::read(path.join("completion-output-v2.bin")).unwrap();
        assert_eq!(frozen, b"paired-source-output");
        if f.case == "publication_io_error" {
            fs::create_dir(path.join("source-outcome-v2.json")).unwrap();
            fs::write(path.join(format!("fault-{fault}.release")), b"release").unwrap();
            wait(|| {
                fs::read_to_string(path.join("source-publication-error.txt"))
                    .ok()
                    .filter(|s| s.contains("bounded regular file"))
            });
        }
        f.gate("write-more");
        wait(|| f.root.path().join("writer-done").exists().then_some(()));
        wait(|| {
            (fs::metadata(path.join("log")).ok()?.len() >= 1024 * 1024 + frozen.len() as u64)
                .then_some(())
        });
        assert!(current_identity_matches(&original));
        assert_eq!(
            fs::read(path.join("completion-output-v2.bin")).unwrap(),
            frozen
        );
        assert!(!path.join("completion-snapshot-v2.json").exists());
        if f.case == "publication_io_error" {
            fs::remove_dir(path.join("source-outcome-v2.json")).unwrap();
        }
        cancel_source(f, source);
    }
    fs::write(path.join(format!("fault-{fault}.release")), b"release").unwrap();
    // Cancellation acceptance above occurs before publication. This retained
    // drain marker can be produced by the original guardian after observer
    // retirement, which the publication barrier intentionally prevents. Require
    // actual recorded drain after releasing it, not retirement while held.
    wait(|| path.join("cancel-workload-drained").exists().then_some(()));
    println!("original source recorded cancellation drain after publication release");
}

#[test]
fn paired_terminal_metadata_then_cancel_preserves_original_root() {
    if !private_case(true) {
        paired_case("publication_race");
    }
}
#[test]
fn paired_publication_fault_keeps_output_and_delivers_original_ready() {
    if !private_case(true) {
        paired_case("publication_error");
    }
}
#[test]
fn paired_real_publication_io_failure_recovers_original_ready() {
    if !private_case(true) {
        paired_case("publication_io_error");
    }
}
#[test]
fn paired_hash_window_cancellation_delivers_full_ready_artifact() {
    if !private_case(true) {
        paired_case("hash_cancel");
    }
}

pub(super) fn prepare(f: &Fixture) {
    match f.case {
        "registration_reply_loss" => f.gate("registration-committed.hold"),
        "acceptance_reply_loss" => f.gate("acceptance-committed.hold"),
        _ => {}
    }
}
fn lose_reply(f: &Fixture, barrier: &str) {
    let pid: i64 = wait(|| {
        fs::read_to_string(f.root.path().join(format!("{barrier}.reached")))
            .ok()?
            .parse()
            .ok()
    });
    let live = read_live_process_identity(pid).unwrap().unwrap();
    let identity = oulipoly_state::completion_continuation::SourceProcessIdentity {
        pid,
        boot_id: live.os_boot_id,
        starttime_ticks: live.os_pid_starttime_ticks,
    };
    assert!(current_identity_matches(&identity));
    // Acceptance can be observed by the independent CD as well as the Bash
    // helper. Record which actual process reached the commit barrier; no claim
    // that a driver loss was only a transport-worker loss.
    let argv = fs::read(format!("/proc/{pid}/cmdline")).unwrap();
    println!(
        "lost committed reply barrier={barrier} exact identity={} argv={:?}",
        serde_json::to_string(&identity).unwrap(),
        String::from_utf8_lossy(&argv)
    );
    assert_eq!(unsafe { libc::kill(pid as i32, libc::SIGKILL) }, 0);
    fs::remove_file(f.root.path().join(format!("{barrier}.hold"))).unwrap();
}
pub(super) fn after_registration(f: &Fixture) {
    if f.case == "registration_reply_loss" {
        lose_reply(f, "registration-committed");
    }
}
pub(super) fn after_acceptance(f: &Fixture) {
    if f.case == "acceptance_reply_loss" {
        lose_reply(f, "acceptance-committed");
    }
}
#[test]
fn paired_lost_registration_reply_recovers_without_workload_replay() {
    if !private_case(true) {
        paired_case("registration_reply_loss");
    }
}
#[test]
fn paired_lost_acceptance_reply_keeps_exact_event_and_recipient_delivery() {
    if !private_case(true) {
        paired_case("acceptance_reply_loss");
    }
}
