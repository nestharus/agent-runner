use super::*;

/// RC-3 reproduction: real-Claude probing showed `--resume` resolves a
/// symlinked cwd before hashing, so migration must write under the resolved
/// workspace's encoded project directory.
// risk: RC-3 symlinked cwd hashing; level: particular-integration; source: research/15-claude-path-hash-rca.md (RC-3).
#[test]
fn rc3_symlinked_workspace_hashes_resolved_path_not_literal_link_path() {
    let fixture = ClaudePathHashFixture::new();
    let workspace = symlinked_workspace(fixture.dir.path());
    let resolved_workspace = workspace.link.canonicalize().unwrap();
    assert_eq!(resolved_workspace, workspace.real.canonicalize().unwrap());

    let migrated = fixture
        .migrate_to(&workspace.link)
        .expect("post-fix migration should accept symlinked absolute cwd paths");

    let literal_target = fixture.expected_target_path(&workspace.link);
    let resolved_target = fixture.expected_target_path(&resolved_workspace);
    assert_ne!(
        literal_target, resolved_target,
        "fixture must distinguish the literal symlink path from the resolved workspace path"
    );
    assert_eq!(
        migrated.target_jsonl_path, resolved_target,
        "Claude Code hashes the resolved cwd path for symlinked workspaces"
    );
    assert!(migrated.target_jsonl_path.exists());
}
