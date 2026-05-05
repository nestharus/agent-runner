use super::*;
use oulipoly_runtime::balancer::TransitionReason;
use oulipoly_runtime::executor::cli::{ResumePayload, execute_resume};
use oulipoly_runtime::migration::migrate_chain_segment;

/// RC-1 reproduction: after migration, the target provider must be able to
/// honor the resumed session from the actual spawn cwd. This fails pre-fix
/// because migration writes under the source transcript's project directory,
/// while Claude Code resolves `--resume <id>` under the child process cwd.
// risk: RC-1 cwd/source project dir mismatch; level: particular-integration; source: ~/projects/agent-runner/planning/trunk/research/14-session-migration-rca.md (RC-1).
#[test]
fn rc1_migrated_transcript_must_be_honorable_from_resume_working_dir() {
    let fixture = MigrationFixture::new();
    let fake_claude = fixture.fake_claude();
    let model = fixture.model(&fake_claude);
    fixture.seed_source_jsonl();
    let db = fixture.state();
    let resolved = fixture.seed_chain(&db, &model);
    let mut stderr = Vec::new();

    let migrated = migrate_chain_segment(
        &db,
        &empty_sessions_config(),
        &model,
        &resolved,
        &fixture.resume_workspace,
        1,
        TransitionReason::QuotaThreshold,
        &mut stderr,
    )
    .unwrap();

    let source_project_target = fixture
        .target_projects
        .join(claude_project_dir_name(&fixture.source_workspace))
        .join(format!("{SESSION_ID}.jsonl"));
    let resume_project_target = fixture
        .target_projects
        .join(claude_project_dir_name(&fixture.resume_workspace))
        .join(format!("{SESSION_ID}.jsonl"));
    assert_eq!(migrated.target_jsonl_path, resume_project_target);
    assert!(
        !source_project_target.exists(),
        "post-fix migration must not write under the source workspace project dir"
    );

    let target_provider = &model.providers[1];
    let strategy = target_provider.resume.as_ref().unwrap();
    let result = execute_resume(
        target_provider,
        1,
        PromptMode::Arg,
        "continue",
        Some(&fixture.resume_workspace),
        None,
        ResumePayload {
            session_id: SESSION_ID,
            strategy,
        },
    )
    .unwrap();

    assert_eq!(
        result.exit_code, 0,
        "post-fix migration should make target provider resume honorable; stderr was: {}",
        result.stderr
    );
}
