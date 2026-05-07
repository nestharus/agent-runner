mod common;

// proposal-test-intent-row 10: CLI list tombstone visibility flag
// assumption-register: A6, A9
// named risk: CLI tombstone visibility flag could diverge from library semantics or omit tombstone metadata in JSON
// selected level: component
#[test]
fn list_json_include_tombstoned_controls_visibility_and_metadata() {
    let db = common::TempDb::new();
    let db_path = db.path().to_str().expect("utf8 db path");
    let fixture = common::fixture_path("markdown-a.md");
    let fixture_path = fixture.to_str().expect("utf8 fixture path");

    common::assert_success(&common::run_agent_store(&[
        "init", "--db", db_path, "--json",
    ]));
    for artifact in ["active.md", "deleted.md"] {
        common::assert_success(&common::run_agent_store(&[
            "put",
            "--db",
            db_path,
            "--workflow-run-id",
            "R-cli-list-flag",
            "--artifact-name",
            artifact,
            "--content-file",
            fixture_path,
            "--json",
        ]));
    }
    common::assert_success(&common::run_agent_store(&[
        "tombstone",
        "--db",
        db_path,
        "--workflow-run-id",
        "R-cli-list-flag",
        "--artifact-name",
        "deleted.md",
        "--version",
        "1",
        "--actor",
        "tester",
        "--reason",
        "hide deleted fixture",
        "--json",
    ]));

    let default_output = common::run_agent_store(&[
        "list",
        "--db",
        db_path,
        "--workflow-run-id",
        "R-cli-list-flag",
        "--json",
    ]);
    common::assert_success(&default_output);
    let default_json = common::stdout_json(&default_output);
    let included_output = common::run_agent_store(&[
        "list",
        "--db",
        db_path,
        "--workflow-run-id",
        "R-cli-list-flag",
        "--include-tombstoned",
        "--json",
    ]);
    common::assert_success(&included_output);
    let included_json = common::stdout_json(&included_output);

    let default_rows = default_json.as_array().expect("default list array");
    let included_rows = included_json.as_array().expect("included list array");
    assert_eq!(default_rows.len(), 1);
    assert_eq!(default_rows[0]["artifact_name"].as_str(), Some("active.md"));
    assert_eq!(included_rows.len(), 2);
    let active_row = included_rows
        .iter()
        .find(|row| row["artifact_name"] == "active.md")
        .expect("active.md in included list");
    let deleted_row = included_rows
        .iter()
        .find(|row| row["artifact_name"] == "deleted.md")
        .expect("deleted.md in included list");
    assert!(active_row["tombstone"].is_null());
    assert!(deleted_row["tombstone"].is_object());
}

// proposal-test-intent-row 19: CLI list JSON order and shape
// assumption-register: A9
// named risk: CLI list output could use an unstable object shape or nondeterministic order
// selected level: component
#[test]
fn list_json_returns_deterministically_ordered_metadata_array() {
    let db = common::TempDb::new();
    let db_path = db.path().to_str().expect("utf8 db path");
    let fixture = common::fixture_path("markdown-a.md");
    let fixture_path = fixture.to_str().expect("utf8 fixture path");

    common::assert_success(&common::run_agent_store(&[
        "init", "--db", db_path, "--json",
    ]));
    for (run, artifact) in [
        ("R-cli-list-b", "b.md"),
        ("R-cli-list-a", "z.md"),
        ("R-cli-list-a", "a.md"),
    ] {
        common::assert_success(&common::run_agent_store(&[
            "put",
            "--db",
            db_path,
            "--workflow-run-id",
            run,
            "--artifact-name",
            artifact,
            "--content-file",
            fixture_path,
            "--json",
        ]));
    }

    let list_output = common::run_agent_store(&["list", "--db", db_path, "--json"]);
    common::assert_success(&list_output);
    let json = common::stdout_json(&list_output);
    let rows = json.as_array().expect("list array");
    for row in rows {
        common::expect_meta_json_shape(row);
    }
    let observed: Vec<_> = rows
        .iter()
        .map(|row| {
            (
                row["workflow_run_id"].as_str().unwrap(),
                row["artifact_name"].as_str().unwrap(),
                row["version"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        observed,
        vec![
            ("R-cli-list-a", "a.md", 1),
            ("R-cli-list-a", "z.md", 1),
            ("R-cli-list-b", "b.md", 1)
        ]
    );
}
