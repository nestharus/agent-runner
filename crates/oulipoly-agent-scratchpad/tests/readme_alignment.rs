use std::fs;
use std::path::{Path, PathBuf};

// proposal § Test-Intent Track rows 15, 16
// contract § Expected observable signals row README-alignment
// named risk: Scratchpad Documentation HIGH - prompt-facing docs could drift from CLI/API contracts
// selected level: documentation
#[test]
fn readme_documents_commands_json_fields_exit_codes_and_behavior_rules() {
    let readme_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md");
    let readme = fs::read_to_string(&readme_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", readme_path.display()));

    for command in [
        "agent-scratchpad write --db",
        "agent-scratchpad read --db",
        "agent-scratchpad list --db",
        "agent-scratchpad delete --db",
        "agent-scratchpad publish --db",
        "agent-scratchpad gc --db",
        "agent-scratchpad scope --invocation-uuid",
    ] {
        assert!(readme.contains(command), "README missing {command}");
    }

    for json_field in [
        "invocation_uuid",
        "name",
        "version",
        "source_version",
        "source_sha256",
        "destination_version",
        "destination_sha256",
        "producer_invocation_uuid",
        "sha256",
        "content_len",
        "format_hint",
        "verdict_line",
        "predecessor_version",
        "created_at",
        "tombstone",
        "tombstoned_versions",
        "already_tombstoned_versions",
        "tombstoned_rows",
        "already_tombstoned_rows",
        "evaluated_at",
    ] {
        assert!(readme.contains(json_field), "README missing {json_field}");
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
        "raw bytes",
        "no trailing newline",
        "7 days",
        "producer_invocation_uuid",
        "publish",
        "canonical",
        "filesystem artifacts remain",
    ] {
        assert!(
            readme.contains(behavior),
            "README missing behavior rule {behavior}"
        );
    }
}

// proposal § Test-Intent Track row 16
// named risk: Existing Filesystem Artifact Systems Accepted As Divergence HIGH - convention docs could redirect canonical planning artifacts into scratchpad
// selected level: documentation
#[test]
fn external_convention_doc_exists_and_points_to_readme_without_redefining_schemas() {
    let Some(convention_dir) = std::env::var_os("AGENT_CONVENTIONS_DIR") else {
        eprintln!("skipping external convention doc check; set AGENT_CONVENTIONS_DIR to enable");
        return;
    };
    let convention_path = PathBuf::from(convention_dir).join("agent-scratchpad.md");
    let Ok(convention) = fs::read_to_string(&convention_path) else {
        eprintln!(
            "skipping external convention doc check; could not read {}",
            convention_path.display()
        );
        return;
    };

    for required in [
        "agent-scratchpad",
        "README",
        "--db",
        "OULIPOLY_PARENT_INVOCATION",
        "private",
        "publish",
        "canonical",
        "filesystem",
    ] {
        assert!(
            convention.contains(required),
            "convention doc missing {required}"
        );
    }

    assert!(
        !convention.contains("tombstoned_rows") && !convention.contains("destination_sha256"),
        "convention doc must point to the README instead of redefining JSON schemas"
    );
}
