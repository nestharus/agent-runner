mod common;

use std::fs;

// proposal-test-intent-row 23: README contract alignment
// assumption-register: A9
// named risk: documentation could become a parallel untested contract that diverges from implemented CLI/library behavior
// selected level: component
#[test]
fn readme_examples_document_the_cli_json_contract_shapes() {
    let readme_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let readme = fs::read_to_string(readme_path).expect("README.md exists");

    for command in [
        "agent-store init --db",
        "agent-store put --db",
        "agent-store get --db",
        "agent-store get-meta --db",
        "agent-store list --db",
        "agent-store tombstone --db",
    ] {
        assert!(readme.contains(command), "README missing {command}");
    }

    for json_field in [
        "workflow_run_id",
        "artifact_name",
        "version",
        "content_len",
        "producer_invocation_uuid",
        "sha256",
        "format_hint",
        "verdict_line",
        "predecessor_version",
        "created_at",
        "tombstoned_at",
        "already_tombstoned",
    ] {
        assert!(readme.contains(json_field), "README missing {json_field}");
    }

    for exit_code_row in ["| 64 |", "| 65 |", "| 66 |", "| 70 |", "| 73 |", "| 74 |"] {
        assert!(
            readme.contains(exit_code_row),
            "README missing exit code row {exit_code_row}"
        );
    }
}
