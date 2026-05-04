use super::*;

/// RC-2 reproduction: Claude Code applies the same encoder to Windows-shaped
/// paths by treating backslashes as separators before non-alnum filtering.
// risk: RC-2 Windows-shaped Claude Code project-dir hashing; level: particular-integration; source: research/15-claude-path-hash-rca.md (RC-2).
#[test]
fn rc2_windows_shape_path_uses_backslash_and_non_alnum_encoding_rule() {
    let fixture = ClaudePathHashFixture::new();
    let resume_workspace = windows_shape_path();

    let migrated = fixture
        .migrate_to(&resume_workspace)
        .expect("post-fix migration should encode Windows-shaped paths via the Claude Code rule");

    assert_eq!(
        expected_claude_code_project_dir(&resume_workspace),
        "C--Users-foo-bar-work-tree---"
    );
    assert_eq!(
        migrated.target_jsonl_path,
        fixture.expected_target_path(&resume_workspace),
        "Windows path separators, ':', '.', '_', and CJK characters must be normalized by the Claude Code encoder"
    );
    assert!(migrated.target_jsonl_path.exists());
}
