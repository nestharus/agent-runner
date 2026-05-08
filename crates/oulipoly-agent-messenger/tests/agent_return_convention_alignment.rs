use std::fs;
use std::path::PathBuf;

// proposal § Test-Intent Track row: agent-return convention
// contract § Expected observable signals row convention doc lint
// named risk: Agent Return Convention HIGH - prompt-facing docs could preserve path-agreement semantics or redefine schemas independently
// selected level: documentation
#[test]
fn convention_doc_points_to_readme_and_forbids_success_inference_from_returns() {
    let explicit_root = std::env::var_os("AGENT_CONVENTIONS_DIR").map(PathBuf::from);
    let convention_root = explicit_root.unwrap_or_else(|| {
        let Some(home) = std::env::var_os("HOME") else {
            eprintln!("skipping external convention alignment check: HOME is unset");
            return PathBuf::new();
        };
        PathBuf::from(home).join("ai/conventions")
    });
    if convention_root.as_os_str().is_empty() {
        return;
    }
    let convention_path = convention_root.join("agent-return.md");
    if std::env::var_os("AGENT_CONVENTIONS_DIR").is_none() && !convention_path.exists() {
        eprintln!(
            "skipping external convention alignment check: {} does not exist",
            convention_path.display()
        );
        return;
    }
    let convention = fs::read_to_string(&convention_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", convention_path.display()));

    for required in [
        "agent-messenger",
        "README",
        "--db",
        "OULIPOLY_PARENT_INVOCATION",
        "OULIPOLY_RETURN_CHANNEL",
        "returned_artifacts",
        "version_id",
        "do not infer success",
        "filesystem path",
    ] {
        assert!(
            convention.contains(required),
            "convention doc missing {required}"
        );
    }

    assert!(
        !convention.contains("source_json") && !convention.contains("schema_version:"),
        "convention doc must point to the README instead of redefining JSON schemas"
    );
}
