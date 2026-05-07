mod common;

use chrono::{TimeDelta, Utc};
use serde_json::Value;
use uuid::Uuid;

// proposal § Test-Intent Track rows 9, 13
// contract § Expected observable signals row gc-invocation-canonical-safe
// named risk: Scratchpad CLI HIGH - invocation GC could tombstone canonical rows
// selected level: cli_integration
#[test]
fn gc_invocation_json_tombstones_scope_and_preserves_canonical_rows() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    let other = Uuid::new_v4();
    common::put_scratchpad_row(&store, invocation, "a.md", b"a".to_vec());
    common::put_scratchpad_row(&store, other, "b.md", b"b".to_vec());
    let canonical = common::put_canonical_row(&store, "canonical-run", "artifact.md");

    let output = common::run_agent_scratchpad(&[
        "gc",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--json",
    ]);

    let json = common::stdout_json(&output);
    assert_eq!(json.get("dry_run").and_then(Value::as_bool), Some(false));
    assert_eq!(
        json.get("tombstoned_rows")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    common::assert_no_canonical_rows_tombstoned(&store);
    store
        .get(&canonical.key, Some(canonical.version))
        .expect("canonical still readable");
    store
        .get(
            &common::store_key(&common::scratchpad_workflow(other), "b.md"),
            None,
        )
        .expect("other invocation row must survive invocation-scoped GC");
}

// proposal § Test-Intent Track row 10
// contract § Expected observable signals row gc-expired-before-past-noop
// named risk: Scratchpad CLI HIGH - expired-before cutoff could delete fresh rows
// selected level: cli_integration
#[test]
fn gc_expired_before_past_json_noops() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    common::put_scratchpad_row(&store, invocation, "fresh.md", b"fresh".to_vec());
    let cutoff = (Utc::now() - TimeDelta::days(1)).to_rfc3339();

    let output = common::run_agent_scratchpad(&[
        "gc",
        "--db",
        &db.path_arg(),
        "--expired-before",
        &cutoff,
        "--json",
    ]);

    let json = common::stdout_json(&output);
    assert_eq!(
        json.get("tombstoned_rows")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0)
    );
    let fresh = store
        .get(
            &common::store_key(&common::scratchpad_workflow(invocation), "fresh.md"),
            None,
        )
        .expect("fresh still readable");
    assert_eq!(fresh.content, b"fresh".to_vec());
}

// proposal § Test-Intent Track row 10 and Assumption Register A5
// named risk: Scratchpad CLI HIGH - unsafe global GC without selector could delete too broadly
// selected level: cli_integration
#[test]
fn gc_requires_invocation_or_expired_before_selector() {
    let (db, _store) = common::init_temp_store();

    let output = common::run_agent_scratchpad(&["gc", "--db", &db.path_arg()]);

    common::assert_exit_code(&output, 64);
    assert!(
        common::stderr_text(&output).contains("--invocation-uuid")
            || common::stderr_text(&output).contains("--expired-before")
    );
}

// proposal § Test-Intent Track row 10
// named risk: Scratchpad CLI HIGH - dry-run sweep could mutate rows while claiming not to
// selected level: cli_integration
#[test]
fn gc_dry_run_json_reports_candidates_without_tombstoning() {
    let (db, store) = common::init_temp_store();
    let invocation = Uuid::new_v4();
    common::put_scratchpad_row(&store, invocation, "candidate.md", b"candidate".to_vec());

    let output = common::run_agent_scratchpad(&[
        "gc",
        "--db",
        &db.path_arg(),
        "--invocation-uuid",
        &invocation.to_string(),
        "--dry-run",
        "--json",
    ]);

    let json = common::stdout_json(&output);
    assert_eq!(json.get("dry_run").and_then(Value::as_bool), Some(true));
    assert_eq!(
        json.get("tombstoned_rows")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );
    store
        .get(
            &common::store_key(&common::scratchpad_workflow(invocation), "candidate.md"),
            None,
        )
        .expect("candidate still readable");
}
