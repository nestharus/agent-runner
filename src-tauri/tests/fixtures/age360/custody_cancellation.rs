//! Deterministic cold/cached scheduling with real registry work and State cancel.
use super::*;

fn prepare(cached: bool, operation: &str) -> (Fixture, ContinuationAttempt) {
    let f = Fixture::new("owner_only");
    f.gate("observe-resume-preflight");
    let mut initial = f.start_with_hold(true);
    f.owner();
    wait(|| {
        f.root
            .path()
            .join("provider-initial-ready")
            .exists()
            .then_some(())
    });
    let barrier = if cached {
        "resume-attributed-preflight"
    } else {
        "resume-uncustodied-preflight"
    };
    f.gate(&format!("{barrier}.hold"));
    if cached {
        f.gate("resume-ingest-before-registry.hold");
    }
    f.gate(&format!("hold-native-{operation}"));
    enqueue(&f);
    f.gate("release-initial-provider");
    f.wait_initial(&mut initial);
    if cached {
        release_after_real_cache_store(&f);
    }
    reached(&f, &format!("native-{operation}"));
    let a = attempt(&f);
    assert_exact_actor(&f, &a, operation);
    assert_cache_branch(&f, cached);
    (f, a)
}

fn observations(f: &Fixture) -> Vec<serde_json::Value> {
    fs::read_to_string(f.root.path().join("resume-preflight.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn release_after_real_cache_store(f: &Fixture) {
    let pid = reached(f, "resume-attributed-preflight");
    let entered = observations(f)
        .into_iter()
        .find(|v| v["pid"] == pid && v["attributed"] == true && v["phase"] == "entered")
        .expect("held attributed preflight lacks exact registry entry observation");
    enqueue_existing_canonical_stream(f);
    assert_eq!(reached(f, "resume-ingest-before-registry"), pid);
    remove_hold(f, "resume-ingest-before-registry");
    let store = wait(|| {
        observations(f).into_iter().find(|v| {
            v["attributed"] == false && v["phase"] == "stored" && same_registry(v, &entered)
        })
    });
    println!("real competing preflight completed before attributed admission: {store}");
    remove_hold(f, "resume-attributed-preflight");
}

fn enqueue_existing_canonical_stream(f: &Fixture) {
    let state = oulipoly_state::StateDb::open(&f.data.join("state.db")).unwrap();
    let key = state.connection().query_row(
        "SELECT provider_name,provider_instance_id,settings_id,session_id FROM session_turn_ingest_streams WHERE session_id=?1 AND projection='canonical_ingest'",
        [SESSION], |row| Ok(oulipoly_state::SessionTurnIngestStreamKey {
            provider_name: row.get(0)?, provider_instance_id: row.get(1)?,
            settings_id: row.get(2)?, session_id: row.get(3)?,
            projection: oulipoly_state::SessionTurnStreamProjection::CanonicalIngest,
        })).unwrap();
    // Explicitly request another real sampling quantum of the finalized initial
    // session through the ingest owner's public enqueue API. Do not clear any
    // delivery/submission state or author endpoint/actor/receipt facts.
    state.enqueue_session_turn_ingest_stream(&key).unwrap();
}

fn assert_cache_branch(f: &Fixture, cached: bool) {
    let phase = if cached { "hit" } else { "miss" };
    let all = observations(f);
    let branch = all
        .iter()
        .find(|v| v["attributed"] == true && v["phase"] == phase)
        .expect("attributed dispatch did not take arranged cache branch");
    let stored = all
        .iter()
        .any(|v| v["attributed"] == false && v["phase"] == "stored" && same_registry(v, branch));
    assert_eq!(
        stored, cached,
        "cold/cached attribution crossed the arranged generation"
    );
    println!("actual registry branch={branch}; observations={all:?}");
}
fn same_registry(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    left["pid"] == right["pid"]
        && stat_start(left) == stat_start(right)
        && left["registry_generation"] == right["registry_generation"]
        && left["account"] == right["account"]
}
fn stat_start(v: &serde_json::Value) -> &str {
    v["stat"]
        .as_str()
        .unwrap()
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(19)
        .unwrap()
}

fn linked_attempt(f: &Fixture, a: &ContinuationAttempt) -> (String, String, String, String) {
    let (generation, invocation) =
        wait(|| f.mailbox().continuation_runtime_identity(a).ok().flatten());
    let state = oulipoly_state::StateDb::open_read_only(&f.data.join("state.db")).unwrap();
    let (attempt, launch): (String, String) = state.connection().query_row(
        "SELECT attempt_id,logical_launch_id FROM provider_launch_attempts WHERE runtime_generation_uuid=?1 AND invocation_uuid=?2",
        rusqlite::params![generation, invocation], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    (attempt, launch, generation, invocation)
}

fn assert_exact_actor(f: &Fixture, a: &ContinuationAttempt, operation: &str) {
    let observed: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(f.root.path().join(format!("native-{operation}.actor.json"))).unwrap(),
    )
    .unwrap();
    let (attempt, launch, generation, invocation) = linked_attempt(f, a);
    assert_eq!(observed["method"], operation);
    assert_eq!(
        observed["actor"]["operation"],
        if operation == "describe" {
            "Describe"
        } else {
            "Policy"
        }
    );
    assert_eq!(observed["actor"]["attempt_id"], attempt);
    assert_eq!(
        observed["provider"]["pid"],
        reached(f, &format!("native-{operation}"))
    );
    for identity in [&observed["provider"], &observed["actor"]["proxy"]] {
        assert!(current_identity_matches(&SourceProcessIdentity {
            pid: identity["pid"].as_i64().unwrap(),
            starttime_ticks: identity["starttime"].as_i64().unwrap(),
            boot_id: observed["boot_id"].as_str().unwrap().into(),
        }));
    }
    println!(
        "exact held actor: activation={} runtime={generation} invocation={invocation} logical={launch} observed={observed}",
        a.attempt_id
    );
}

pub(super) fn prelaunch_cancellation(operation: &str) {
    if private_case(false) {
        return;
    }
    let (f, a) = prepare(false, operation);
    request_linked_cancel(&f, &a);
    // The intended attributed actor was observed live and cancelled above.
    // Release competing observers so their original threads can finish too.
    remove_hold(&f, "resume-uncustodied-preflight");
    let token = fs::read_to_string(f.root.path().join("state-cancel-token")).unwrap();
    let launch = token.split(':').next().unwrap();
    assert_eq!(linked_attempt(&f, &a).1, launch);
    wait(|| (launch_status(&f, launch) == "cancelled").then_some(()));
    let receipt = wait_drained_attempt(&f, &a);
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    println!(
        "actual {operation} cancellation settled before launch; no recipient invocation; receipt={receipt}"
    );
}

fn launch_status(f: &Fixture, launch: &str) -> String {
    oulipoly_state::StateDb::open_read_only(&f.data.join("state.db"))
        .unwrap()
        .connection()
        .query_row(
            "SELECT status FROM provider_logical_launches WHERE logical_launch_id=?1",
            [launch],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn native_cached_describe_progresses_through_policy_and_launch() {
    if private_case(false) {
        return;
    }
    let (f, a) = prepare(true, "policy.evaluate");
    let (attempt, launch, _, _) = linked_attempt(&f, &a);
    let admission: serde_json::Value = serde_json::from_slice(
        &fs::read(
            f.data
                .join("state.native-producer-custody")
                .join(&attempt)
                .join("actors/admissions/describe.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(admission["admitted"], false);
    f.gate("release-resume");
    fs::remove_file(f.root.path().join("hold-native-policy.evaluate")).unwrap();
    wait(|| (launch_status(&f, &launch) == "succeeded").then_some(()));
    let receipt = wait_drained_attempt(&f, &a);
    let prompts = fs::read_to_string(f.root.path().join("resume-prompts.jsonl")).unwrap();
    assert_eq!(prompts.lines().count(), 1);
    assert!(prompts.contains("native-custody-input"));
    println!(
        "cached describe not admitted; exact real policy/launch succeeded: {launch}; receipt={receipt}"
    );
}

#[test]
fn native_cold_describe_release_without_cancel_progresses() {
    if private_case(false) {
        return;
    }
    let (f, a) = prepare(false, "describe");
    let (_, launch, _, _) = linked_attempt(&f, &a);
    assert!(!f.root.path().join("resume-prompts.jsonl").exists());
    assert!(!f.root.path().join("state-cancel-token").exists());
    f.gate("release-resume");
    fs::remove_file(f.root.path().join("hold-native-describe")).unwrap();
    remove_hold(&f, "resume-uncustodied-preflight");
    wait(|| (launch_status(&f, &launch) == "succeeded").then_some(()));
    let receipt = wait_drained_attempt(&f, &a);
    assert!(
        fs::read_to_string(f.root.path().join("resume-prompts.jsonl"))
            .unwrap()
            .contains("native-custody-input")
    );
    println!(
        "negative cancellation control: real cold describe released without cancellation, launch succeeded; {receipt}"
    );
}

#[test]
fn registry_join_rejects_another_generation_in_the_same_process() {
    let current = serde_json::json!({"pid":std::process::id(),
        "stat":fs::read_to_string("/proc/self/stat").unwrap(),
        "registry_generation":2,"account":PROVIDER});
    assert!(same_registry(&current, &current));
    let mut stale = current.clone();
    stale["registry_generation"] = serde_json::json!(1);
    assert!(!same_registry(&stale, &current));
    let mut other = current.clone();
    other["account"] = serde_json::json!("another-account");
    assert!(!same_registry(&other, &current));
}
