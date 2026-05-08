use std::fs;
use std::path::Path;

// proposal § Test-Intent Track row: README alignment
// contract § Expected observable signals row README alignment
// named risk: Messenger Documentation HIGH - README could become a parallel schema or omit CLI/exit-code rules
// selected level: documentation
#[test]
fn readme_documents_commands_json_fields_exit_codes_and_behavior_rules() {
    let readme_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let readme = fs::read_to_string(&readme_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", readme_path.display()));

    for command in [
        "agent-messenger return --db",
        "agent-messenger list-returned --db",
        "agent-messenger show --db",
        "agent-messenger version --json",
    ] {
        assert!(readme.contains(command), "README missing {command}");
    }

    for field in [
        "schema_version",
        "version_id",
        "name",
        "store_address",
        "workflow_run_id",
        "artifact_name",
        "version",
        "sha256",
        "content_len",
        "format_hint",
        "verdict_line",
        "source",
        "producer_invocation_uuid",
        "returned_at",
    ] {
        assert!(readme.contains(field), "README missing {field}");
    }

    for exit_code_row in [
        "| 0 |", "| 64 |", "| 65 |", "| 66 |", "| 70 |", "| 73 |", "| 74 |",
    ] {
        assert!(
            readme.contains(exit_code_row),
            "README missing exit code row {exit_code_row}"
        );
    }

    for behavior in [
        "--db",
        "--invocation-uuid",
        "OULIPOLY_PARENT_INVOCATION",
        "OULIPOLY_RETURN_CHANNEL",
        "raw bytes",
        "no trailing newline",
        "store://return/",
        "scratchpad:",
        "returned_artifacts",
    ] {
        assert!(
            readme.contains(behavior),
            "README missing behavior {behavior}"
        );
    }
}
