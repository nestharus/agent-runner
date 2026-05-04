use super::*;

/// RC-1 reproduction: Claude Code replaces every non-ASCII-alphanumeric
/// character except '-' with '-', so '.', '_', and CJK characters must not
/// survive in the project directory name.
// risk: RC-1 incomplete Claude Code project-dir encoder; level: particular-integration; source: research/15-claude-path-hash-rca.md (RC-1).
#[test]
fn rc1_project_dir_encoder_replaces_all_non_alnum_except_dash() {
    let fixture = ClaudePathHashFixture::new();
    let resume_workspace = fixture.path_with_non_alnum();
    std::fs::create_dir_all(&resume_workspace).unwrap();

    let migrated = fixture
        .migrate_to(&resume_workspace)
        .expect("post-fix migration should accept absolute paths with special chars");

    assert_eq!(
        migrated.target_jsonl_path,
        fixture.expected_target_path(&resume_workspace),
        "Claude Code project-dir encoding must replace '.', '_', and CJK characters with '-'"
    );
    assert!(migrated.target_jsonl_path.exists());
}
