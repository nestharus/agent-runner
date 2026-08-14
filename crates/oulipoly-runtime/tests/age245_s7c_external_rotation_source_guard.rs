//! Declared roles: accessor, mapper, validator, predicate, orchestration.

use chrono::{DateTime, Utc};
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, SessionsConfig,
    provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_provider::generated::{
    MigrationApplyResult, MigrationPlanResult, RotationAssessResult,
};
use oulipoly_runtime::provider_registry::{
    ProviderRegistry, ProviderRegistryHandle, ProviderRegistryOptions,
};
use oulipoly_runtime::rotation_external_provider::{
    ExternalRotationError, apply_migration, assess_rotation, materialize_rotation, plan_migration,
    resolve_rotation_external_provider_identity,
};
use oulipoly_runtime::services::MigrationServicePort;
use oulipoly_runtime::services::{
    MigrationServiceOutput, MigrationServiceRequest, ProductionMigrationService, ServiceError,
};
use oulipoly_state::{InvocationStart, ResolvedResume, SessionTurnIngest, StateDb};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

const MODEL: &str = "model-alpha";
const SOURCE_PROVIDER: &str = "source-provider";
const TARGET_PROVIDER: &str = "target-provider";
const SOURCE_SESSION: &str = "session-source";
const TARGET_SESSION: &str = "session-target";
const TURN_ID: &str = "turn-alpha";

#[test]
fn s7c_migration_service_declares_strict_three_state_identity_without_fail_open() {
    let migration = read_sources(&[
        "crates/oulipoly-runtime/src/services/migration.rs",
        "crates/oulipoly-runtime/src/services/migration/external_branch_orchestration.rs",
        "crates/oulipoly-runtime/src/services/migration/external_identity_accessor.rs",
        "crates/oulipoly-runtime/src/services/migration/error_formatter.rs",
    ]);
    for needle in [
        "select_migration_branch",
        "resolve_external_provider_identity",
        "MigrationBranch::BuiltIn",
        "MigrationBranch::External { identity }",
        "Err(error)",
        "construct_migration_service_error",
    ] {
        assert_contains(
            "services/migration external branch surface",
            &migration,
            needle,
        );
    }
    assert_not_contains(
        "services/migration external branch surface",
        &migration,
        "unwrap_or_else(|_| MigrationServiceOutput::Stay)",
    );
    assert_not_contains(
        "services/migration external branch surface",
        &migration,
        "unwrap_or_else(|_| crate::migration::migrate_chain_segment",
    );
}

#[test]
fn s7c_external_rotation_dispatch_uses_registry_client_and_capability_gates() {
    let module = read_source("crates/oulipoly-runtime/src/rotation_external_provider/mod.rs");
    let provider_access = read_sources(&[
        "crates/oulipoly-runtime/src/rotation_external_provider/provider_access.rs",
        "crates/oulipoly-runtime/src/rotation_external_provider/provider_access/registry_artifact_access.rs",
        "crates/oulipoly-runtime/src/rotation_external_provider/provider_access/capability_predicates.rs",
    ]);
    let provider_dispatch =
        read_source("crates/oulipoly-runtime/src/rotation_external_provider/provider_dispatch.rs");
    for needle in [
        "pub fn assess_rotation(",
        "pub fn materialize_rotation(",
        "pub fn plan_migration(",
        "pub fn apply_migration(",
        "ProviderRegistryHandle",
        "\"rotation.assess\"",
        "\"rotation.materialize\"",
        "\"migration.plan\"",
        "\"migration.apply\"",
    ] {
        assert_contains("rotation_external_provider/mod.rs", &module, needle);
    }
    for needle in [
        "describe_model_provider",
        "enabled_artifact_for_model",
        "client_factory",
        "supports_rotation_or_migration",
    ] {
        assert_contains(
            "rotation_external_provider/provider_access.rs",
            &provider_access,
            needle,
        );
    }
    assert_contains(
        "rotation_external_provider/provider_dispatch.rs",
        &provider_dispatch,
        "invoke_provider_contract",
    );
    assert_not_contains(
        "rotation_external_provider/mod.rs",
        &module,
        "migrate_chain_segment(",
    );
}

#[test]
fn s7c_host_apply_keeps_validation_artifacts_and_sqlite_transaction_separate() {
    let host_apply = read_sources(&[
        "crates/oulipoly-runtime/src/rotation_host_apply/mod.rs",
        "crates/oulipoly-runtime/src/rotation_host_apply/state_access.rs",
        "crates/oulipoly-runtime/src/rotation_host_apply/plan_validation.rs",
        "crates/oulipoly-runtime/src/rotation_host_apply/artifact_verification.rs",
        "crates/oulipoly-runtime/src/rotation_host_apply/mutation_mapper.rs",
        "crates/oulipoly-runtime/src/rotation_host_apply/predicates.rs",
    ]);
    for needle in [
        "validate_host_state_plan",
        "verify_rotation_artifacts",
        "load_chain_segment_snapshot",
        "compute_chain_segment_mutations",
        "apply_chain_segment_transaction",
        "last_used_at",
        "find_conflicting_active_segment",
        "close_active_segment_returning",
        "open_chain_segment",
    ] {
        assert_contains("rotation_host_apply/mod.rs", &host_apply, needle);
    }
    assert_not_contains("rotation_host_apply/mod.rs", &host_apply, "ProviderClient");
    assert_not_contains("rotation_host_apply/mod.rs", &host_apply, "invoke_typed");
}

#[test]
fn s7c_rotation_journal_covers_option_a_crash_recovery_before_provider_dispatch() {
    let journal = read_sources(&[
        "crates/oulipoly-runtime/src/rotation_journal/mod.rs",
        "crates/oulipoly-runtime/src/rotation_journal/types.rs",
        "crates/oulipoly-runtime/src/rotation_journal/record_access.rs",
        "crates/oulipoly-runtime/src/rotation_journal/classifier.rs",
        "crates/oulipoly-runtime/src/rotation_journal/recovery_mapper.rs",
        "crates/oulipoly-runtime/src/rotation_journal/recovery_executor.rs",
    ]);
    let journal_formatter =
        read_source("crates/oulipoly-runtime/src/rotation_journal/error_formatter.rs");
    for needle in [
        "write_rotation_journal_record",
        "classify_rotation_journal_state",
        "build_rotation_recovery_plan",
        "execute_rotation_recovery_plan",
        "crash_after_artifact",
        "crash_during_apply",
        "startup_recovery_before_provider_dispatch",
    ] {
        assert_contains("rotation_journal/mod.rs", &journal, needle);
    }
    assert_contains(
        "rotation_journal/error_formatter.rs",
        &journal_formatter,
        "quarantine",
    );
}

#[test]
fn s7c_external_rotation_errors_have_stable_no_fallback_constructors() {
    let formatter =
        read_source("crates/oulipoly-runtime/src/rotation_external_provider/error_formatter.rs");
    for needle in [
        "malformed_external_identity",
        "missing_registry_handle",
        "missing_enabled_artifact",
        "describe_failure",
        "capability_missing",
        "disabled_artifact",
        "provider_transport_failure",
        "protocol_invalid_response",
        "semantic_host_plan_rejection",
        "artifact_verification_failure",
        "host_apply_conflict",
        "journal_recovery_failure",
    ] {
        assert_contains(
            "rotation_external_provider/error_formatter.rs",
            &formatter,
            needle,
        );
    }
}

#[test]
fn s7c_external_provider_matrix_executes_assess_plan_apply_and_materialize_paths() {
    let assess = RuntimeFixture::new("s7c-rotation-assess-success");
    let before = assess.snapshot();
    let before_tree = assess.workspace_tree();
    let assess_result: RotationAssessResult = assess_rotation(
        &assess.registry,
        assess.identity(),
        &assess.request(&mut Vec::new()),
    )
    .expect("assess success");
    assert!(assess_result.allowed);
    assert_eq!(assess.snapshot(), before);
    assert_eq!(assess.workspace_tree(), before_tree);
    assert!(!oulipoly_runtime::rotation_journal::rotation_journal_path(&assess.workspace).exists());
    assert!(
        !oulipoly_runtime::rotation_journal::rotation_journal_lock_path(&assess.workspace).exists()
    );
    assess.assert_call_count(2);
    assess.assert_last_request("rotation.assess");

    let denied = RuntimeFixture::new("s7c-rotation-assess-denied");
    let before = denied.snapshot();
    let before_tree = denied.workspace_tree();
    let denied_result: RotationAssessResult = assess_rotation(
        &denied.registry,
        denied.identity(),
        &denied.request(&mut Vec::new()),
    )
    .expect("assess denied is a successful provider decision");
    assert!(!denied_result.allowed);
    assert_eq!(denied.snapshot(), before);
    assert_eq!(denied.workspace_tree(), before_tree);
    assert!(!oulipoly_runtime::rotation_journal::rotation_journal_path(&denied.workspace).exists());
    assert!(
        !oulipoly_runtime::rotation_journal::rotation_journal_lock_path(&denied.workspace).exists()
    );
    denied.assert_last_request("rotation.assess");

    let plan = RuntimeFixture::new("s7c-migration-plan-success");
    let plan_result: MigrationPlanResult = plan_migration(
        &plan.registry,
        plan.identity(),
        &plan.request(&mut Vec::new()),
    )
    .expect("migration plan success");
    assert_eq!(plan_result.actions.len(), 1);
    plan.assert_call_count(2);
    plan.assert_last_request("migration.plan");

    let apply = RuntimeFixture::new("s7c-migration-apply-success");
    let apply_result: MigrationApplyResult = apply_migration(
        &apply.registry,
        apply.identity(),
        &apply.request(&mut Vec::new()),
    )
    .expect("migration apply success");
    assert_eq!(apply_result.applied_actions.len(), 1);
    apply.assert_call_count(2);
    apply.assert_last_request("migration.apply");

    let no_change = RuntimeFixture::new("s7c-rotation-materialize-no-change");
    let before = no_change.snapshot();
    let output = materialize_rotation(
        &no_change.registry,
        no_change.identity(),
        &no_change.request(&mut Vec::new()),
    )
    .expect("materialize no-change succeeds");
    assert!(matches!(output, MigrationServiceOutput::Stay));
    assert_eq!(no_change.snapshot(), before);
    no_change.assert_last_request("rotation.materialize");

    let no_change_wrong_chain =
        RuntimeFixture::new("s7c-rotation-materialize-no-change-wrong-chain");
    let before = no_change_wrong_chain.snapshot();
    let err = materialize_rotation(
        &no_change_wrong_chain.registry,
        no_change_wrong_chain.identity(),
        &no_change_wrong_chain.request(&mut Vec::new()),
    )
    .expect_err("no-change host_state_plan still must validate");
    assert!(matches!(
        err,
        ExternalRotationError::SemanticHostPlanRejection { .. }
    ));
    assert_eq!(no_change_wrong_chain.snapshot(), before);
    no_change_wrong_chain.assert_last_request("rotation.materialize");

    let dry_run = RuntimeFixture::new("s7c-rotation-materialize-dry-run");
    let before = dry_run.snapshot();
    let output = materialize_rotation(
        &dry_run.registry,
        dry_run.identity(),
        &dry_run.request(&mut Vec::new()),
    )
    .expect("materialize dry-run succeeds as no change");
    assert!(matches!(output, MigrationServiceOutput::Stay));
    assert_eq!(dry_run.snapshot(), before);
    dry_run.assert_call_count(2);
    dry_run.assert_last_request("rotation.materialize");

    let success = RuntimeFixture::new("s7c-rotation-materialize-success");
    let before = success.snapshot();
    let output = materialize_rotation(
        &success.registry,
        success.identity(),
        &success.request(&mut Vec::new()),
    )
    .expect("materialize success");
    let MigrationServiceOutput::Migrated { segment } = output else {
        panic!("expected migrated output");
    };
    assert_eq!(segment.target_provider, TARGET_PROVIDER);
    assert_eq!(segment.target_session_id, TARGET_SESSION);
    assert_eq!(
        success.snapshot().chains,
        before.chains,
        "external host apply must preserve session_chains.last_used_at"
    );
    success.assert_provider_plan_boundary_was_applied();
    assert!(
        !oulipoly_runtime::rotation_journal::rotation_journal_path(&success.workspace).exists()
    );
    assert!(
        !oulipoly_runtime::rotation_journal::rotation_journal_lock_path(&success.workspace)
            .exists()
    );
    success.assert_last_request("rotation.materialize");

    let compaction = RuntimeFixture::new("s7c-rotation-materialize-compaction-boundary");
    let output = materialize_rotation(
        &compaction.registry,
        compaction.identity(),
        &compaction.request(&mut Vec::new()),
    )
    .expect("materialize compaction boundary");
    let MigrationServiceOutput::Migrated { segment } = output else {
        panic!("expected compaction-boundary migrated output");
    };
    assert_eq!(segment.target_provider, TARGET_PROVIDER);
    assert_eq!(
        compaction.active_segment(),
        (TARGET_PROVIDER.to_string(), TARGET_SESSION.to_string())
    );
    compaction.assert_call_count(2);
    compaction.assert_last_request("rotation.materialize");
}

#[test]
fn s7c_external_provider_matrix_executes_error_protocol_capability_and_no_mutation_paths() {
    let assess_provider_error = RuntimeFixture::new("provider-error");
    let before = assess_provider_error.snapshot();
    let err = assess_rotation(
        &assess_provider_error.registry,
        assess_provider_error.identity(),
        &assess_provider_error.request(&mut Vec::new()),
    )
    .expect_err("assess provider error must be hard failure");
    assert!(matches!(
        err,
        ExternalRotationError::CapabilityMissing { .. }
    ));
    assert_eq!(assess_provider_error.snapshot(), before);
    assess_provider_error.assert_call_count(2);
    assess_provider_error.assert_last_request("rotation.assess");

    let assess_protocol = RuntimeFixture::new("schema-invalid-success");
    let before = assess_protocol.snapshot();
    let err = assess_rotation(
        &assess_protocol.registry,
        assess_protocol.identity(),
        &assess_protocol.request(&mut Vec::new()),
    )
    .expect_err("assess protocol-invalid response must fail");
    assert!(matches!(
        err,
        ExternalRotationError::ProtocolInvalidResponse { .. }
    ));
    assert_eq!(assess_protocol.snapshot(), before);
    assess_protocol.assert_call_count(2);
    assess_protocol.assert_last_request("rotation.assess");

    let assess_transport = RuntimeFixture::new("exit-nonzero-no-envelope");
    let before = assess_transport.snapshot();
    let err = assess_rotation(
        &assess_transport.registry,
        assess_transport.identity(),
        &assess_transport.request(&mut Vec::new()),
    )
    .expect_err("assess transport failure must fail");
    assert!(matches!(
        err,
        ExternalRotationError::ProviderTransportFailure { .. }
    ));
    assert_eq!(assess_transport.snapshot(), before);
    assess_transport.assert_call_count(2);
    assess_transport.assert_last_request("rotation.assess");

    let missing_source = RuntimeFixture::new("s7c-rotation-materialize-missing-source");
    assert_hard_materialize_failure_no_mutation(
        &missing_source,
        |err| matches!(err, ExternalRotationError::CapabilityMissing { .. }),
        "materialize missing-source provider error must be hard failure",
    );

    let materialize_provider_error = RuntimeFixture::new("provider-error");
    assert_hard_materialize_failure_no_mutation(
        &materialize_provider_error,
        |err| matches!(err, ExternalRotationError::CapabilityMissing { .. }),
        "materialize provider error must be hard failure",
    );

    let protocol = RuntimeFixture::new("s7c-rotation-materialize-protocol-invalid");
    let before = protocol.snapshot();
    let err = materialize_rotation(
        &protocol.registry,
        protocol.identity(),
        &protocol.request(&mut Vec::new()),
    )
    .expect_err("protocol-invalid provider result must fail");
    assert!(matches!(
        err,
        ExternalRotationError::ProtocolInvalidResponse { .. }
    ));
    assert_eq!(protocol.snapshot(), before);

    let capability = RuntimeFixture::new("describe-rotation-disabled");
    let before = capability.snapshot();
    let err = assess_rotation(
        &capability.registry,
        capability.identity(),
        &capability.request(&mut Vec::new()),
    )
    .expect_err("missing rotation capability must fail");
    assert!(matches!(
        err,
        ExternalRotationError::CapabilityMissing { .. }
    ));
    assert_eq!(capability.snapshot(), before);

    let describe_failure = RuntimeFixture::new("describe-failed");
    let before = describe_failure.snapshot();
    let service =
        ProductionMigrationService::with_registry_handle(describe_failure.registry.clone());
    let err = service
        .migrate(describe_failure.request_manual(&mut Vec::new(), TARGET_PROVIDER))
        .expect_err("describe failure must be a hard service dependency failure");
    assert!(matches!(err, ServiceError::Dependency { .. }));
    assert_eq!(describe_failure.snapshot(), before);

    let disabled_artifact = RuntimeFixture::new_disabled_artifact();
    let before = disabled_artifact.snapshot();
    let service =
        ProductionMigrationService::with_registry_handle(disabled_artifact.registry.clone());
    let err = service
        .migrate(disabled_artifact.request_manual(&mut Vec::new(), TARGET_PROVIDER))
        .expect_err("runtime-disabled artifact must be a hard service dependency failure");
    assert!(matches!(err, ServiceError::Dependency { .. }));
    assert_eq!(disabled_artifact.snapshot(), before);

    let mismatch = RuntimeFixture::new("s7c-rotation-materialize-artifact-hash-mismatch");
    let before = mismatch.snapshot();
    let err = materialize_rotation(
        &mismatch.registry,
        mismatch.identity(),
        &mismatch.request(&mut Vec::new()),
    )
    .expect_err("artifact hash mismatch must fail before host mutation");
    assert!(matches!(
        err,
        ExternalRotationError::ArtifactVerificationFailure { .. }
    ));
    assert_eq!(mismatch.snapshot(), before);
}

#[test]
fn s7c_migration_plan_apply_matrix_executes_provider_error_and_protocol_invalid_distinctly() {
    let plan_error = RuntimeFixture::new("provider-error");
    let before = plan_error.snapshot();
    let err = plan_migration(
        &plan_error.registry,
        plan_error.identity(),
        &plan_error.request(&mut Vec::new()),
    )
    .expect_err("migration.plan provider error must fail");
    assert!(matches!(
        err,
        ExternalRotationError::CapabilityMissing { .. }
    ));
    assert_eq!(plan_error.snapshot(), before);
    plan_error.assert_call_count(2);
    plan_error.assert_last_request("migration.plan");

    let plan_protocol = RuntimeFixture::new("s7c-migration-plan-protocol-invalid");
    let before = plan_protocol.snapshot();
    let err = plan_migration(
        &plan_protocol.registry,
        plan_protocol.identity(),
        &plan_protocol.request(&mut Vec::new()),
    )
    .expect_err("migration.plan protocol invalid must fail");
    assert!(matches!(
        err,
        ExternalRotationError::ProtocolInvalidResponse { .. }
    ));
    assert_eq!(plan_protocol.snapshot(), before);
    plan_protocol.assert_call_count(2);
    plan_protocol.assert_last_request("migration.plan");

    let apply_error = RuntimeFixture::new("provider-error");
    let before = apply_error.snapshot();
    let err = apply_migration(
        &apply_error.registry,
        apply_error.identity(),
        &apply_error.request(&mut Vec::new()),
    )
    .expect_err("migration.apply provider error must fail");
    assert!(matches!(
        err,
        ExternalRotationError::CapabilityMissing { .. }
    ));
    assert_eq!(apply_error.snapshot(), before);
    apply_error.assert_call_count(2);
    apply_error.assert_last_request("migration.apply");

    let apply_protocol = RuntimeFixture::new("s7c-migration-apply-protocol-invalid");
    let before = apply_protocol.snapshot();
    let err = apply_migration(
        &apply_protocol.registry,
        apply_protocol.identity(),
        &apply_protocol.request(&mut Vec::new()),
    )
    .expect_err("migration.apply protocol invalid must fail");
    assert!(matches!(
        err,
        ExternalRotationError::ProtocolInvalidResponse { .. }
    ));
    assert_eq!(apply_protocol.snapshot(), before);
    apply_protocol.assert_call_count(2);
    apply_protocol.assert_last_request("migration.apply");

    let plan_capability = RuntimeFixture::new("describe-migration-disabled");
    let before = plan_capability.snapshot();
    let err = plan_migration(
        &plan_capability.registry,
        plan_capability.identity(),
        &plan_capability.request(&mut Vec::new()),
    )
    .expect_err("migration.plan missing capability must fail");
    assert!(matches!(
        err,
        ExternalRotationError::CapabilityMissing { .. }
    ));
    assert_eq!(plan_capability.snapshot(), before);

    let apply_capability = RuntimeFixture::new("describe-migration-disabled");
    let before = apply_capability.snapshot();
    let err = apply_migration(
        &apply_capability.registry,
        apply_capability.identity(),
        &apply_capability.request(&mut Vec::new()),
    )
    .expect_err("migration.apply missing capability must fail");
    assert!(matches!(
        err,
        ExternalRotationError::CapabilityMissing { .. }
    ));
    assert_eq!(apply_capability.snapshot(), before);
}

#[test]
fn s7c_host_apply_transaction_rolls_back_when_open_segment_fails() {
    let fixture = RuntimeFixture::new("s7c-rotation-materialize-success");
    let before = fixture.snapshot();
    fixture.install_open_failure_trigger();
    let err = materialize_rotation(
        &fixture.registry,
        fixture.identity(),
        &fixture.request(&mut Vec::new()),
    )
    .expect_err("triggered open failure must fail host apply");
    assert!(matches!(
        err,
        ExternalRotationError::HostApplyConflict { .. }
    ));
    fixture.drop_open_failure_trigger();
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn s7c_external_rotation_identity_uses_target_account_as_settings_id() {
    let fixture = RuntimeFixture::new("s7c-rotation-materialize-success");

    assert_eq!(fixture.identity().settings_id, TARGET_PROVIDER);
}

#[test]
fn s7c_host_apply_transaction_rechecks_validated_source_segment() {
    let fixture = RuntimeFixture::new("s7c-rotation-materialize-success");
    let result = fixture.materialize_result();
    let identity = fixture.identity();
    oulipoly_runtime::rotation_host_apply::validate_host_state_plan(
        &result.host_state_plan,
        &result.artifacts,
        &fixture.request(&mut Vec::new()),
        &identity,
    )
    .expect("initial host plan validates against source segment");
    let changed_at = DateTime::parse_from_rfc3339("2026-05-01T00:00:01Z")
        .expect("time")
        .with_timezone(&Utc);
    fixture
        .state
        .close_active_segment_returning(&fixture.resolved.chain_id, &changed_at)
        .expect("close original active source");
    fixture
        .state
        .open_chain_segment(
            &fixture.resolved.chain_id,
            "interloper-provider",
            "interloper-session",
            &changed_at,
            oulipoly_runtime::balancer::TransitionReason::Imported,
        )
        .expect("open interloper segment");

    let before = fixture.snapshot();
    let err = oulipoly_runtime::rotation_host_apply::apply_chain_segment_transaction(
        &fixture.request(&mut Vec::new()),
        &identity,
        &result,
    )
    .expect_err("transaction must recheck validated source provider/session");
    assert!(matches!(
        err,
        ExternalRotationError::HostApplyConflict { .. }
    ));
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(
        fixture.active_segment(),
        (
            "interloper-provider".to_string(),
            "interloper-session".to_string()
        )
    );
}

#[test]
fn s7c_fake_provider_crash_modes_are_hard_failures_or_recoverable_journal_states() {
    let after_artifact = RuntimeFixture::new("s7c-rotation-materialize-crash-after-artifact");
    let before = after_artifact.snapshot();
    let err = materialize_rotation(
        &after_artifact.registry,
        after_artifact.identity(),
        &after_artifact.request(&mut Vec::new()),
    )
    .expect_err("fake-provider crash after artifact must leave rollback journal");
    assert!(matches!(
        err,
        ExternalRotationError::ArtifactVerificationFailure { .. }
    ));
    assert_eq!(after_artifact.snapshot(), before);
    assert!(
        oulipoly_runtime::rotation_journal::rotation_journal_path(&after_artifact.workspace)
            .exists()
    );
    assert!(
        oulipoly_runtime::rotation_journal::rotation_journal_lock_path(&after_artifact.workspace)
            .exists()
    );
    oulipoly_runtime::rotation_journal::startup_recovery_before_provider_dispatch(
        &after_artifact.request(&mut Vec::new()),
    )
    .expect("recover fake-provider crash after artifact");
    assert_eq!(after_artifact.snapshot(), before);
    assert!(!after_artifact.artifact_path.exists());
    assert!(
        !oulipoly_runtime::rotation_journal::rotation_journal_path(&after_artifact.workspace)
            .exists()
    );
    assert!(
        !oulipoly_runtime::rotation_journal::rotation_journal_lock_path(&after_artifact.workspace)
            .exists()
    );
    after_artifact.assert_call_count(2);
    after_artifact.assert_last_request("rotation.materialize");

    let during_apply = RuntimeFixture::new("s7c-rotation-materialize-crash-during-apply");
    during_apply.install_open_failure_trigger();
    let err = materialize_rotation(
        &during_apply.registry,
        during_apply.identity(),
        &during_apply.request(&mut Vec::new()),
    )
    .expect_err("fake-provider crash during apply must leave recovery journal");
    assert!(matches!(
        err,
        ExternalRotationError::HostApplyConflict { .. }
    ));
    assert!(
        oulipoly_runtime::rotation_journal::rotation_journal_path(&during_apply.workspace).exists()
    );
    assert!(
        oulipoly_runtime::rotation_journal::rotation_journal_lock_path(&during_apply.workspace)
            .exists()
    );
    during_apply.drop_open_failure_trigger();
    oulipoly_runtime::rotation_journal::startup_recovery_before_provider_dispatch(
        &during_apply.request(&mut Vec::new()),
    )
    .expect("recover fake-provider crash during apply");
    assert_eq!(
        during_apply.active_segment(),
        (TARGET_PROVIDER.to_string(), TARGET_SESSION.to_string())
    );
    assert!(
        !oulipoly_runtime::rotation_journal::rotation_journal_path(&during_apply.workspace)
            .exists()
    );
    assert!(
        !oulipoly_runtime::rotation_journal::rotation_journal_lock_path(&during_apply.workspace)
            .exists()
    );
}

#[test]
fn s7c_rotation_journal_lock_is_exclusive_before_host_apply_mutation() {
    let fixture = RuntimeFixture::new("s7c-rotation-materialize-success");
    let lock_path =
        oulipoly_runtime::rotation_journal::rotation_journal_lock_path(&fixture.workspace);
    std::fs::write(&lock_path, "held").expect("preexisting lock");
    let before = fixture.snapshot();

    let err = materialize_rotation(
        &fixture.registry,
        fixture.identity(),
        &fixture.request(&mut Vec::new()),
    )
    .expect_err("preexisting lock must block materialize before host apply");

    assert!(matches!(
        err,
        ExternalRotationError::JournalRecoveryFailure { .. }
    ));
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(
        fixture.active_segment(),
        (SOURCE_PROVIDER.to_string(), SOURCE_SESSION.to_string())
    );
    assert!(
        lock_path.exists(),
        "failed lock acquisition must not remove a lock owned by another rotation"
    );
    std::fs::remove_file(&lock_path).expect("cleanup preexisting lock");
    fixture.assert_last_request("rotation.materialize");
}

#[test]
fn s7c_rotation_journal_recovers_crash_after_artifact_and_during_apply() {
    let after_artifact = RuntimeFixture::new("s7c-rotation-materialize-success");
    let result = after_artifact.materialize_result();
    oulipoly_runtime::rotation_journal::publish_after_artifact_record(
        &after_artifact.request(&mut Vec::new()),
        &after_artifact.identity(),
        &result,
    )
    .expect("publish after-artifact journal");
    let before = after_artifact.snapshot();
    let before_tree = after_artifact.workspace_tree();
    after_artifact.assert_journal_preimage();
    oulipoly_runtime::rotation_journal::startup_recovery_before_provider_dispatch(
        &after_artifact.request(&mut Vec::new()),
    )
    .expect("recover after artifact");
    assert_eq!(after_artifact.snapshot(), before);
    assert_ne!(after_artifact.workspace_tree(), before_tree);
    assert!(!after_artifact.artifact_path.exists());
    assert!(
        !oulipoly_runtime::rotation_journal::rotation_journal_path(&after_artifact.workspace)
            .exists()
    );
    assert!(
        !oulipoly_runtime::rotation_journal::rotation_journal_lock_path(&after_artifact.workspace)
            .exists()
    );

    let during_apply = RuntimeFixture::new("s7c-rotation-materialize-success");
    let result = during_apply.materialize_result();
    oulipoly_runtime::rotation_journal::publish_during_apply_record(
        &during_apply.request(&mut Vec::new()),
        &during_apply.identity(),
        &result,
    )
    .expect("publish during-apply journal");
    let before = during_apply.snapshot();
    let before_tree = during_apply.workspace_tree();
    during_apply.assert_journal_preimage();
    oulipoly_runtime::rotation_journal::startup_recovery_before_provider_dispatch(
        &during_apply.request(&mut Vec::new()),
    )
    .expect("recover during apply");
    assert_ne!(during_apply.snapshot(), before);
    assert_ne!(during_apply.workspace_tree(), before_tree);
    assert_eq!(
        during_apply.active_segment(),
        (TARGET_PROVIDER.to_string(), TARGET_SESSION.to_string())
    );
    assert!(
        !oulipoly_runtime::rotation_journal::rotation_journal_path(&during_apply.workspace)
            .exists()
    );
    assert!(
        !oulipoly_runtime::rotation_journal::rotation_journal_lock_path(&during_apply.workspace)
            .exists()
    );

    let committed = RuntimeFixture::new("s7c-rotation-materialize-success");
    let result = committed.materialize_result();
    let identity = committed.identity();
    oulipoly_runtime::rotation_journal::publish_during_apply_record(
        &committed.request(&mut Vec::new()),
        &identity,
        &result,
    )
    .expect("publish committed during-apply journal");
    oulipoly_runtime::rotation_host_apply::apply_chain_segment_transaction(
        &committed.request(&mut Vec::new()),
        &identity,
        &result,
    )
    .expect("simulate commit before journal cleanup");
    assert_eq!(
        committed.active_segment(),
        (TARGET_PROVIDER.to_string(), TARGET_SESSION.to_string())
    );
    assert!(
        oulipoly_runtime::rotation_journal::rotation_journal_path(&committed.workspace).exists()
    );
    oulipoly_runtime::rotation_journal::startup_recovery_before_provider_dispatch(
        &committed.request(&mut Vec::new()),
    )
    .expect("recover committed during-apply journal idempotently");
    assert_eq!(
        committed.active_segment(),
        (TARGET_PROVIDER.to_string(), TARGET_SESSION.to_string())
    );
    assert!(
        !oulipoly_runtime::rotation_journal::rotation_journal_path(&committed.workspace).exists()
    );
    assert!(
        !oulipoly_runtime::rotation_journal::rotation_journal_lock_path(&committed.workspace)
            .exists()
    );
}

#[test]
fn s7c_host_state_plan_runtime_rejects_schema_valid_semantic_mismatches_before_mutation() {
    for case in [
        HostPlanCase::WrongChain,
        HostPlanCase::ProviderSessionMismatch,
        HostPlanCase::InvalidReason,
        HostPlanCase::StaleSnapshot,
        HostPlanCase::MissingArtifact,
        HostPlanCase::UnsupportedVersion,
        HostPlanCase::ArtifactResultMismatch,
    ] {
        let fixture = RuntimeFixture::new("s7c-rotation-materialize-success");
        let before = fixture.snapshot();
        let mut result = fixture.materialize_result();
        apply_host_plan_case(&mut result, case);
        let err = oulipoly_runtime::rotation_host_apply::validate_host_state_plan(
            &result.host_state_plan,
            &result.artifacts,
            &fixture.request(&mut Vec::new()),
            &fixture.identity(),
        )
        .expect_err("schema-valid semantic mismatch must be rejected");
        assert!(matches!(
            err,
            ExternalRotationError::SemanticHostPlanRejection { .. }
        ));
        assert_eq!(fixture.snapshot(), before, "{case:?} mutated host state");
    }

    let conflict = RuntimeFixture::new("s7c-rotation-materialize-success");
    conflict.seed_conflicting_target_segment();
    let before = conflict.snapshot();
    let result = conflict.materialize_result();
    let err = oulipoly_runtime::rotation_host_apply::validate_host_state_plan(
        &result.host_state_plan,
        &result.artifacts,
        &conflict.request(&mut Vec::new()),
        &conflict.identity(),
    )
    .expect_err("active target conflict must be rejected");
    assert!(matches!(
        err,
        ExternalRotationError::HostApplyConflict { .. }
    ));
    assert_eq!(conflict.snapshot(), before);
}

#[test]
fn s7c_production_migration_service_external_failure_does_not_fail_open_to_builtin() {
    let fixture = RuntimeFixture::new("provider-error");
    let before = fixture.snapshot();
    let service = ProductionMigrationService::with_registry_handle(fixture.registry.clone());
    let err = service
        .migrate(fixture.request_manual(&mut Vec::new(), TARGET_PROVIDER))
        .expect_err("external provider failure must be a hard service dependency failure");
    assert!(matches!(err, ServiceError::Dependency { .. }));
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(
        fixture.active_segment(),
        (SOURCE_PROVIDER.to_string(), SOURCE_SESSION.to_string())
    );
    fixture.assert_call_count(2);
    fixture.assert_last_request("rotation.materialize");
    fixture.assert_materialize_request_context("manual");
}

#[test]
fn s7c_production_migration_service_runs_journal_recovery_before_provider_request() {
    let fixture = RuntimeFixture::new("s7c-rotation-materialize-success");
    let journal_path =
        oulipoly_runtime::rotation_journal::rotation_journal_path(&fixture.workspace);
    std::fs::create_dir_all(journal_path.parent().expect("journal parent")).expect("journal dir");
    std::fs::write(&journal_path, b"{not-json").expect("ambiguous journal");
    let service = ProductionMigrationService::with_registry_handle(fixture.registry.clone());
    let err = service
        .migrate(fixture.request_manual(&mut Vec::new(), TARGET_PROVIDER))
        .expect_err("ambiguous journal must block provider dispatch");
    assert!(matches!(err, ServiceError::Dependency { .. }));
    fixture.assert_call_count(0);
    assert!(
        journal_path.exists(),
        "quarantined journal remains for operator inspection"
    );
}

#[test]
fn s7c_provider_ref_manual_rotation_service_applies_external_plan_without_builtin_stderr() {
    let fixture = RuntimeFixture::new("s7c-rotation-materialize-success");
    let before = fixture.snapshot();
    let service = ProductionMigrationService::with_registry_handle(fixture.registry.clone());
    let mut stderr = Vec::new();

    let output = service
        .migrate(fixture.request_manual(&mut stderr, TARGET_PROVIDER))
        .expect("external service migration");

    let MigrationServiceOutput::Migrated { segment } = output else {
        panic!("expected migrated output");
    };
    assert_eq!(segment.target_provider, TARGET_PROVIDER);
    assert_eq!(segment.target_session_id, TARGET_SESSION);
    assert_eq!(
        fixture.snapshot().chains,
        before.chains,
        "external host apply must preserve session_chains.last_used_at"
    );
    fixture.assert_provider_plan_boundary_was_applied();
    assert_eq!(
        fixture.active_segment(),
        (TARGET_PROVIDER.to_string(), TARGET_SESSION.to_string())
    );
    assert!(fixture.artifact_path.exists());
    assert!(
        String::from_utf8_lossy(&stderr).is_empty(),
        "external success must not emit built-in migration stderr"
    );
    fixture.assert_call_count(2);
    fixture.assert_last_request("rotation.materialize");
}

struct RuntimeFixture {
    _dir: tempfile::TempDir,
    state: StateDb,
    sessions: SessionsConfig,
    model: ModelConfig,
    resolved: ResolvedResume,
    registry: ProviderRegistryHandle,
    workspace: PathBuf,
    artifact_path: PathBuf,
    record_path: PathBuf,
    count_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DbSnapshot {
    chains: Vec<Vec<String>>,
    segments: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HostPlanCase {
    WrongChain,
    ProviderSessionMismatch,
    InvalidReason,
    StaleSnapshot,
    MissingArtifact,
    UnsupportedVersion,
    ArtifactResultMismatch,
}

impl RuntimeFixture {
    fn new(mode: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let state = StateDb::open(&dir.path().join("state.db")).expect("state db");
        let model = external_model_from_ref(path_provider_ref(&provider_wrapper(dir.path(), mode)));
        let resolved = seed_chain(&state, &model);
        let artifact_path = workspace.join("session-target.jsonl");
        let record_path = dir.path().join("request-record.txt");
        let count_path = dir.path().join("provider-count.txt");
        std::fs::write(&count_path, "0").expect("count seed");
        rewrite_wrapper_env(
            model
                .provider
                .as_ref()
                .and_then(|provider| provider.path.as_deref())
                .expect("path"),
            &artifact_path,
            &record_path,
            &count_path,
            &resolved.chain_id,
        );
        let registry = ProviderRegistryHandle::new(Arc::new(
            ProviderRegistry::from_model_configs(
                std::slice::from_ref(&model),
                ProviderRegistryOptions::default(),
            )
            .expect("registry"),
        ));
        Self {
            _dir: dir,
            state,
            sessions: SessionsConfig::default(),
            model,
            resolved,
            registry,
            workspace,
            artifact_path,
            record_path,
            count_path,
        }
    }

    fn new_disabled_artifact() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let state = StateDb::open(&dir.path().join("state.db")).expect("state db");
        let model = external_model_from_ref(ProviderImplementationRef {
            path: None,
            crate_name: Some("fake-provider-crate".to_string()),
            version: Some("0.1.0".to_string()),
            binary: None,
            script: None,
        });
        let resolved = seed_chain(&state, &model);
        let artifact_path = workspace.join("session-target.jsonl");
        let record_path = dir.path().join("request-record.txt");
        let count_path = dir.path().join("provider-count.txt");
        std::fs::write(&count_path, "0").expect("count seed");
        let registry = ProviderRegistryHandle::new(Arc::new(
            ProviderRegistry::from_model_configs(
                std::slice::from_ref(&model),
                ProviderRegistryOptions::default(),
            )
            .expect("registry"),
        ));
        Self {
            _dir: dir,
            state,
            sessions: SessionsConfig::default(),
            model,
            resolved,
            registry,
            workspace,
            artifact_path,
            record_path,
            count_path,
        }
    }

    fn identity(&self) -> oulipoly_runtime::rotation_external_provider::ExternalRotationIdentity {
        let registry = self.registry.current();
        resolve_rotation_external_provider_identity(
            registry.as_ref(),
            &self.model,
            &self.resolved,
            TARGET_PROVIDER,
        )
        .expect("external identity")
    }

    fn request<'a>(&'a self, stderr: &'a mut Vec<u8>) -> MigrationServiceRequest<'a> {
        MigrationServiceRequest {
            state: &self.state,
            sessions_cfg: &self.sessions,
            resolved: &self.resolved,
            manual_target: None,
            active_exhausted: false,
            migration_model: &self.model,
            effective_cwd: &self.workspace,
            stderr,
        }
    }

    fn request_manual<'a>(
        &'a self,
        stderr: &'a mut Vec<u8>,
        manual_target: &'a str,
    ) -> MigrationServiceRequest<'a> {
        MigrationServiceRequest {
            manual_target: Some(manual_target),
            ..self.request(stderr)
        }
    }

    fn materialize_result(&self) -> oulipoly_provider::generated::RotationMaterializeResult {
        std::fs::write(&self.artifact_path, []).expect("artifact");
        let artifact = self.artifact_path.display().to_string();
        serde_json::from_value(serde_json::json!({
            "changed": true,
            "target_provider_session_id": TARGET_SESSION,
            "artifacts": [{
                "kind": "file",
                "path": artifact.clone(),
                "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            }],
            "host_state_plan": {
                "schema_version": 1,
                "operation": "rotation.materialize",
                "chain_id": self.resolved.chain_id,
                "source_provider": SOURCE_PROVIDER,
                "target_provider": TARGET_PROVIDER,
                "source_session_id": SOURCE_SESSION,
                "target_session_id": TARGET_SESSION,
                "transition_reason": "quota_threshold",
                "segments": [
                    { "provider": SOURCE_PROVIDER, "session_id": SOURCE_SESSION, "ended_at": "2026-05-01T00:00:00Z" },
                    { "provider": TARGET_PROVIDER, "session_id": TARGET_SESSION, "started_at": "2026-05-01T00:00:00Z" }
                ],
                "artifacts": [{
                    "kind": "file",
                    "path": artifact,
                    "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                }]
            }
        }))
        .expect("materialize result")
    }

    fn snapshot(&self) -> DbSnapshot {
        let connection = rusqlite::Connection::open(self.state.path()).expect("state connection");
        DbSnapshot {
            chains: full_rows(
                &connection,
                "SELECT chain_id, created_at, last_used_at, model_name FROM session_chains ORDER BY chain_id",
            ),
            segments: full_rows(
                &connection,
                "SELECT id, chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason FROM session_chain_segments ORDER BY id",
            ),
        }
    }

    fn active_segment(&self) -> (String, String) {
        self.state
            .connection()
            .query_row(
                "SELECT provider_name, session_id FROM session_chain_segments WHERE chain_id = ?1 AND ended_at IS NULL",
                rusqlite::params![self.resolved.chain_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("active segment")
    }

    fn assert_provider_plan_boundary_was_applied(&self) {
        let (source_ended_at, target_started_at): (String, String) = self
            .state
            .connection()
            .query_row(
                "SELECT source.ended_at, target.started_at
                     FROM session_chain_segments source
                     JOIN session_chain_segments target ON source.chain_id = target.chain_id
                     WHERE source.chain_id = ?1
                       AND source.provider_name = ?2
                       AND source.session_id = ?3
                       AND target.provider_name = ?4
                       AND target.session_id = ?5",
                rusqlite::params![
                    self.resolved.chain_id,
                    SOURCE_PROVIDER,
                    SOURCE_SESSION,
                    TARGET_PROVIDER,
                    TARGET_SESSION
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("segment boundary rows");
        assert_eq!(source_ended_at, "2026-05-01T00:00:00+00:00");
        assert_eq!(target_started_at, "2026-05-01T00:00:00+00:00");
    }

    fn workspace_tree(&self) -> Vec<String> {
        let mut entries = Vec::new();
        collect_tree(&self.workspace, &self.workspace, &mut entries);
        entries.sort();
        entries
    }

    fn assert_journal_preimage(&self) {
        let journal = std::fs::read_to_string(
            oulipoly_runtime::rotation_journal::rotation_journal_path(&self.workspace),
        )
        .expect("journal");
        assert!(
            journal.contains("\"preimage\""),
            "journal must include host preimage: {journal}"
        );
        assert!(
            journal.contains(SOURCE_PROVIDER),
            "journal preimage must include source provider: {journal}"
        );
        assert!(
            oulipoly_runtime::rotation_journal::rotation_journal_lock_path(&self.workspace)
                .exists(),
            "journal lock must be present while recovery is pending"
        );
    }

    fn assert_last_request(&self, operation: &str) {
        let record = std::fs::read_to_string(&self.record_path).expect("record");
        assert!(
            record.contains(operation),
            "record must contain operation {operation}: {record}"
        );
        assert!(
            record.contains(SOURCE_PROVIDER),
            "record must contain source provider: {record}"
        );
        assert!(
            record.contains(TARGET_PROVIDER),
            "record must contain target provider: {record}"
        );
        assert!(
            record.contains(&self.resolved.chain_id),
            "record must contain chain id: {record}"
        );
    }

    fn assert_materialize_request_context(&self, transition_reason: &str) {
        let record = std::fs::read_to_string(&self.record_path).expect("record");
        let request: serde_json::Value = serde_json::from_str(
            record
                .split_once("stdin:\n")
                .map(|(_, stdin)| stdin)
                .expect("recorded provider stdin"),
        )
        .expect("recorded provider request JSON");
        assert_eq!(request["params"]["settings_id"], TARGET_PROVIDER);
        assert_eq!(request["params"]["transition_reason"], transition_reason);
        assert!(
            request["host"]["data_root"]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "rotation materialization requires a durable artifact root: {request}"
        );
    }

    fn assert_call_count(&self, expected: u64) {
        let actual = std::fs::read_to_string(&self.count_path)
            .expect("count")
            .trim()
            .parse::<u64>()
            .expect("count integer");
        assert_eq!(actual, expected);
    }

    fn install_open_failure_trigger(&self) {
        rusqlite::Connection::open(self.state.path())
            .expect("state connection")
            .execute_batch(
                "CREATE TRIGGER fail_target_open
                 BEFORE INSERT ON session_chain_segments
                 WHEN NEW.provider_name = 'target-provider'
                 BEGIN
                   SELECT RAISE(ABORT, 'forced target open failure');
                 END;",
            )
            .expect("install trigger");
    }

    fn drop_open_failure_trigger(&self) {
        rusqlite::Connection::open(self.state.path())
            .expect("state connection")
            .execute_batch("DROP TRIGGER fail_target_open;")
            .expect("drop trigger");
    }

    fn seed_conflicting_target_segment(&self) {
        self.state
            .mint_imported_chain_if_absent(
                TARGET_PROVIDER,
                TARGET_SESSION,
                &DateTime::parse_from_rfc3339("2026-05-01T00:00:00Z")
                    .expect("time")
                    .with_timezone(&Utc),
                MODEL,
            )
            .expect("conflicting target segment");
    }
}

fn assert_hard_materialize_failure_no_mutation(
    fixture: &RuntimeFixture,
    matches_error: impl FnOnce(&ExternalRotationError) -> bool,
    message: &str,
) {
    let before = fixture.snapshot();
    let err = materialize_rotation(
        &fixture.registry,
        fixture.identity(),
        &fixture.request(&mut Vec::new()),
    )
    .expect_err(message);
    assert!(matches_error(&err), "{message}: {err:?}");
    assert_eq!(fixture.snapshot(), before);
    fixture.assert_call_count(2);
    fixture.assert_last_request("rotation.materialize");
}

fn apply_host_plan_case(
    result: &mut oulipoly_provider::generated::RotationMaterializeResult,
    case: HostPlanCase,
) {
    match case {
        HostPlanCase::WrongChain => set_plan_field(result, "chain_id", "chain-other"),
        HostPlanCase::ProviderSessionMismatch => {
            set_plan_field(result, "source_session_id", "session-other")
        }
        HostPlanCase::InvalidReason => set_plan_field(result, "transition_reason", "invalid"),
        HostPlanCase::StaleSnapshot => {
            set_plan_segment_field(result, 0, "ended_at", "2026-04-30T00:00:00Z")
        }
        HostPlanCase::MissingArtifact => {
            let missing = "/tmp/oulipoly-s7c-missing-artifact.jsonl";
            set_plan_artifact_field(result, 0, "path", missing);
            result.artifacts[0].path = Some(missing.to_string());
        }
        HostPlanCase::UnsupportedVersion => {
            result.host_state_plan["schema_version"] = serde_json::json!(999);
        }
        HostPlanCase::ArtifactResultMismatch => set_plan_artifact_field(
            result,
            0,
            "sha256",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ),
    }
}

fn set_plan_field(
    result: &mut oulipoly_provider::generated::RotationMaterializeResult,
    field: &str,
    value: &str,
) {
    result.host_state_plan[field] = serde_json::json!(value);
}

fn set_plan_segment_field(
    result: &mut oulipoly_provider::generated::RotationMaterializeResult,
    index: usize,
    field: &str,
    value: &str,
) {
    result.host_state_plan["segments"][index][field] = serde_json::json!(value);
}

fn set_plan_artifact_field(
    result: &mut oulipoly_provider::generated::RotationMaterializeResult,
    index: usize,
    field: &str,
    value: &str,
) {
    result.host_state_plan["artifacts"][index][field] = serde_json::json!(value);
}

fn seed_chain(state: &StateDb, model: &ModelConfig) -> ResolvedResume {
    let invocation_id = state
        .start_invocation(&InvocationStart {
            invocation_uuid: uuid::Uuid::new_v4().to_string(),
            model_name: MODEL.to_string(),
            provider_name: SOURCE_PROVIDER.to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .expect("start invocation");
    state
        .update_session_capture(invocation_id, Some(SOURCE_SESSION), "fixture")
        .expect("capture");
    state
        .mint_chain_for_invocation_session(invocation_id)
        .expect("chain");
    state
        .ingest_session_turns_batch(
            SOURCE_PROVIDER,
            &[SessionTurnIngest {
                session_id: SOURCE_SESSION.to_string(),
                turn_id: TURN_ID.to_string(),
                timestamp: DateTime::parse_from_rfc3339("2026-05-01T00:00:00Z")
                    .expect("time")
                    .with_timezone(&Utc),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            }],
        )
        .expect("turn");
    let chain_id = state
        .chain_id_for_segment(SOURCE_PROVIDER, SOURCE_SESSION)
        .expect("chain lookup")
        .expect("chain id");
    ResolvedResume {
        chain_id,
        model_name: Some(model.name.clone()),
        model: Some(model.clone()),
        active_provider: SOURCE_PROVIDER.to_string(),
        active_session_id: SOURCE_SESSION.to_string(),
    }
}

fn path_provider_ref(provider_path: &Path) -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: Some(provider_path.display().to_string()),
        crate_name: None,
        version: None,
        binary: None,
        script: None,
    }
}

fn external_model_from_ref(provider: ProviderImplementationRef) -> ModelConfig {
    ModelConfig {
        name: MODEL.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            ProviderConfig::model_provider(SOURCE_PROVIDER, Vec::new()),
            ProviderConfig::model_provider(TARGET_PROVIDER, Vec::new()),
        ],
        inputs: Vec::new(),
        provider: Some(provider),
    }
}

fn provider_wrapper(dir: &Path, mode: &str) -> PathBuf {
    let source = workspace_root()
        .join("crates/oulipoly-provider/tests/fixtures/provider_client/fake_provider.rs");
    let binary = dir.join("fake-provider-bin");
    let status = Command::new("rustc")
        .arg("--edition=2024")
        .arg(source)
        .arg("-o")
        .arg(&binary)
        .status()
        .expect("rustc fake provider");
    assert!(status.success());
    let wrapper = dir.join("fake-provider-wrapper");
    std::fs::write(
        &wrapper,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nexport FAKE_PROVIDER_MODE={}\nexec {} \"$@\"\n",
            shell_quote(mode),
            shell_quote(&binary.display().to_string())
        ),
    )
    .expect("wrapper");
    make_executable(&wrapper);
    wrapper
}

fn rewrite_wrapper_env(
    wrapper: &str,
    artifact_path: &Path,
    record_path: &Path,
    count_path: &Path,
    chain_id: &str,
) {
    let body = std::fs::read_to_string(wrapper).expect("wrapper body");
    let insert = format!(
        "export S7C_ARTIFACT_PATH={}\nexport S7C_CHAIN_ID={}\nexport S7C_SOURCE_PROVIDER={}\nexport S7C_TARGET_PROVIDER={}\nexport S7C_SOURCE_SESSION_ID={}\nexport S7C_TARGET_SESSION_ID={}\nexport FAKE_PROVIDER_RECORD_PATH={}\nexport FAKE_PROVIDER_COUNT_PATH={}\n",
        shell_quote(&artifact_path.display().to_string()),
        shell_quote(chain_id),
        shell_quote(SOURCE_PROVIDER),
        shell_quote(TARGET_PROVIDER),
        shell_quote(SOURCE_SESSION),
        shell_quote(TARGET_SESSION),
        shell_quote(&record_path.display().to_string()),
        shell_quote(&count_path.display().to_string()),
    );
    std::fs::write(wrapper, body.replace("exec ", &(insert + "exec "))).expect("rewrite wrapper");
}

fn full_rows(conn: &rusqlite::Connection, sql: &str) -> Vec<Vec<String>> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let column_count = stmt.column_count();
    stmt.query_map([], |row| {
        let mut fields = Vec::new();
        for index in 0..column_count {
            fields.push(row_value(row, index)?);
        }
        Ok(fields)
    })
    .expect("query")
    .collect::<Result<Vec<_>, _>>()
    .expect("rows")
}

fn collect_tree(root: &Path, dir: &Path, entries: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|error| panic!("read {dir:?}: {error}")) {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            collect_tree(root, &path, entries);
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("workspace relative path")
                .display()
                .to_string();
            let len = std::fs::metadata(&path).expect("metadata").len();
            entries.push(format!("{relative}:{len}"));
        }
    }
}

fn row_value(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<String> {
    let value = row.get_ref(index)?;
    Ok(match value {
        rusqlite::types::ValueRef::Null => "<NULL>".to_string(),
        rusqlite::types::ValueRef::Integer(value) => value.to_string(),
        rusqlite::types::ValueRef::Real(value) => value.to_string(),
        rusqlite::types::ValueRef::Text(value) => String::from_utf8_lossy(value).into_owned(),
        rusqlite::types::ValueRef::Blob(value) => format!("{value:?}"),
    })
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("chmod");
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn read_source(relative: &str) -> String {
    std::fs::read_to_string(workspace_root().join(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

fn read_sources(relative_paths: &[&str]) -> String {
    relative_paths
        .iter()
        .map(|relative| read_source(relative))
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_contains(context: &str, source: &str, needle: &str) {
    assert!(
        source.contains(needle),
        "{context} must contain {needle:?} for AGE-245 S7c external rotation contract"
    );
}

fn assert_not_contains(context: &str, source: &str, needle: &str) {
    assert!(
        !source.contains(needle),
        "{context} must not contain {needle:?} for AGE-245 S7c no-fail-open contract"
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .parent()
        .expect("repo root")
        .to_path_buf()
}
