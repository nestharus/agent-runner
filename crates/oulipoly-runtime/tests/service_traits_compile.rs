use oulipoly_config::{ModelConfig, PromptMode, ProvidersConfig, SessionsConfig};
use oulipoly_runtime::services::error::ServiceError;
use oulipoly_runtime::services::*;
use oulipoly_runtime::trace::TraceOptions;
use oulipoly_state::{InvocationStart, ModelStore, ResolvedResume, StateDb};
use std::path::Path;
use std::sync::Arc;

struct StubService;

impl ConfigServicePort for StubService {
    fn load_config(
        &self,
        _request: ConfigServiceRequest,
    ) -> Result<ConfigServiceOutput, ServiceError> {
        unimplemented!()
    }
}

impl ExecutorServicePort for StubService {
    fn execute(
        &self,
        _request: ExecutorServiceRequest,
    ) -> Result<ExecutorServiceOutput, ServiceError> {
        unimplemented!()
    }
}

impl LauncherServicePort for StubService {
    fn launch(
        &self,
        _request: LauncherServiceRequest,
    ) -> Result<LauncherServiceOutput, ServiceError> {
        unimplemented!()
    }
}

impl QuotaServicePort for StubService {
    fn refresh_quota(
        &self,
        _request: QuotaServiceRequest,
    ) -> Result<QuotaServiceOutput, ServiceError> {
        unimplemented!()
    }
}

impl RoutingServicePort for StubService {
    fn select_route(
        &self,
        _request: RoutingServiceRequest<'_>,
    ) -> Result<RoutingServiceOutput, ServiceError> {
        unimplemented!()
    }
}

impl InvocationLifecycleServicePort for StubService {
    fn start_invocation(
        &self,
        _request: InvocationLifecycleStartRequest<'_>,
    ) -> Result<InvocationLifecycleStartOutput, ServiceError> {
        unimplemented!()
    }

    fn finalize_invocation(
        &self,
        _request: InvocationLifecycleFinalizeRequest<'_>,
    ) -> Result<InvocationLifecycleFinalizeOutput, ServiceError> {
        unimplemented!()
    }
}

impl SessionLifecycleServicePort for StubService {
    fn ingest_session(
        &self,
        _request: SessionLifecycleRequest<'_>,
    ) -> Result<SessionLifecycleOutput, ServiceError> {
        unimplemented!()
    }
}

impl ResumeServicePort for StubService {
    fn resolve_resume(
        &self,
        _request: ResumeServiceRequest<'_>,
    ) -> Result<ResumeServiceOutput, ServiceError> {
        unimplemented!()
    }

    fn record_acceptance(
        &self,
        _request: ResumeAcceptanceRequest<'_>,
    ) -> Result<ResumeAcceptanceOutput, ServiceError> {
        unimplemented!()
    }
}

impl DiagnosticsServicePort for StubService {
    fn diagnose(
        &self,
        _request: DiagnosticsServiceRequest,
    ) -> Result<DiagnosticsServiceOutput, ServiceError> {
        unimplemented!()
    }
}

impl MigrationServicePort for StubService {
    fn migrate(
        &self,
        _request: MigrationServiceRequest<'_>,
    ) -> Result<MigrationServiceOutput, ServiceError> {
        unimplemented!()
    }
}

impl TraceServicePort for StubService {
    fn trace(&self, _request: TraceServiceRequest<'_>) -> Result<TraceServiceOutput, ServiceError> {
        unimplemented!()
    }
}

impl SessionExportServicePort for StubService {
    fn export_session(
        &self,
        _request: SessionExportServiceRequest,
    ) -> Result<SessionExportServiceOutput, ServiceError> {
        unimplemented!()
    }
}

impl SessionReplaceServicePort for StubService {
    fn replace_session(
        &self,
        _request: SessionReplaceServiceRequest,
    ) -> Result<SessionReplaceServiceOutput, ServiceError> {
        unimplemented!()
    }
}

impl SessionLockServicePort for StubService {
    fn lock_session(
        &self,
        _request: SessionLockServiceRequest,
    ) -> Result<SessionLockServiceOutput, ServiceError> {
        unimplemented!()
    }
}

impl MigrationMaintenanceServicePort for StubService {
    fn run_maintenance(
        &self,
        _request: MigrationMaintenanceServiceRequest,
    ) -> Result<MigrationMaintenanceServiceOutput, ServiceError> {
        unimplemented!()
    }
}

#[test]
fn service_port_traits_are_send_sync_trait_object_usable() {
    let _: Box<dyn ConfigServicePort + Send + Sync> = Box::new(StubService);
    let _: Box<dyn ExecutorServicePort + Send + Sync> = Box::new(StubService);
    let _: Box<dyn LauncherServicePort + Send + Sync> = Box::new(StubService);
    let _: Box<dyn QuotaServicePort + Send + Sync> = Box::new(StubService);
    let _: Box<dyn RoutingServicePort + Send + Sync> = Box::new(StubService);
    let _: Box<dyn InvocationLifecycleServicePort + Send + Sync> = Box::new(StubService);
    let _: Box<dyn SessionLifecycleServicePort + Send + Sync> = Box::new(StubService);
    let _: Box<dyn ResumeServicePort + Send + Sync> = Box::new(StubService);
    let _: Box<dyn DiagnosticsServicePort + Send + Sync> = Box::new(StubService);
    let _: Box<dyn MigrationServicePort + Send + Sync> = Box::new(StubService);
    let _: Box<dyn TraceServicePort + Send + Sync> = Box::new(StubService);
    let _: Box<dyn SessionExportServicePort + Send + Sync> = Box::new(StubService);
    let _: Box<dyn SessionReplaceServicePort + Send + Sync> = Box::new(StubService);
    let _: Box<dyn SessionLockServicePort + Send + Sync> = Box::new(StubService);
    let _: Box<dyn MigrationMaintenanceServicePort + Send + Sync> = Box::new(StubService);
}

// Risk: R-A1 / proposal T9 - DTO expansion must keep the existing port method
// names object-safe, and production adapters must implement those named ports.
// Level: unit.
// Source: AGE-34 contract "Adapter signatures" and proposal assumption A1.
#[test]
fn age_34_runtime_service_adapters_are_send_sync_trait_object_usable() {
    let _: Box<dyn ExecutorServicePort + Send + Sync> =
        Box::new(oulipoly_runtime::executor::RuntimeExecutorService::default());
    let _: Box<dyn LauncherServicePort + Send + Sync> =
        Box::new(oulipoly_runtime::repl_default_provider::RuntimeLauncherService);
    let _: Box<dyn QuotaServicePort + Send + Sync> =
        Box::new(oulipoly_runtime::quota::RuntimeQuotaService);
    let _: Box<dyn DiagnosticsServicePort + Send + Sync> =
        Box::new(oulipoly_runtime::diagnostics::RuntimeDiagnosticsService::default());
}

#[test]
fn age_35_routing_and_invocation_lifecycle_services_are_object_safe_with_contract_dtos() {
    let routing: Arc<dyn RoutingServicePort + Send + Sync> = Arc::new(StubService);
    let lifecycle: Arc<dyn InvocationLifecycleServicePort + Send + Sync> = Arc::new(StubService);
    let _production_routing: Arc<dyn RoutingServicePort + Send + Sync> =
        Arc::new(ProductionRoutingService);
    let _production_lifecycle: Arc<dyn InvocationLifecycleServicePort + Send + Sync> =
        Arc::new(ProductionInvocationLifecycleService);

    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = ModelConfig {
        name: "age35-compile".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![],
        inputs: vec![],
        provider: None,
    };
    let start = InvocationStart {
        invocation_uuid: "11111111-1111-4111-8111-111111111111".to_string(),
        model_name: model.name.clone(),
        provider_name: "compile-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };

    let route_request = RoutingServiceRequest {
        model: &model,
        state: &db,
        ctx: None,
    };
    let lifecycle_start = InvocationLifecycleStartRequest {
        state: &db,
        start: &start,
    };
    let lifecycle_finalize = InvocationLifecycleFinalizeRequest {
        state: &db,
        invocation_row_id: 1,
        success: true,
        exit_code: 0,
        error_category: None,
        terminal_reason: Some("compile_only"),
    };

    if false {
        let _: Result<RoutingServiceOutput, ServiceError> = routing.select_route(route_request);
        let _: Result<InvocationLifecycleStartOutput, ServiceError> =
            lifecycle.start_invocation(lifecycle_start);
        let _: Result<InvocationLifecycleFinalizeOutput, ServiceError> =
            lifecycle.finalize_invocation(lifecycle_finalize);
    }
}

#[test]
fn age299_s1_runtime_boundary_can_carry_ownership_authority_without_service_behavior_changes() {
    fn carry_snapshot(
        snapshot: oulipoly_state::OwnershipAuthoritySnapshot,
    ) -> oulipoly_state::OwnershipAuthoritySnapshot {
        snapshot
    }

    let snapshot = carry_snapshot(oulipoly_state::OwnershipAuthoritySnapshot {
        invocation_uuid: "11111111-1111-4111-8111-111111111111".to_string(),
        event_id: "ab_age299_compile".to_string(),
        sidecar_generation: oulipoly_state::SidecarGenerationState::ExpectedButUnobserved {
            expected: "22222222-2222-4222-8222-222222222222".to_string(),
        },
        event_state: oulipoly_state::OwnedCompletionEventState::Pending,
        owner_invocation_uuid: "11111111-1111-4111-8111-111111111111".to_string(),
        owner_relationship: oulipoly_state::OwnerLineageRelationship::ExactOwner,
        listener_settlement: oulipoly_state::ListenerSettlementClass::PendingOrUnsettled,
        recovery_disposition: oulipoly_state::RecoveryDisposition::NotRecorded,
    });
    assert_eq!(snapshot.event_id, "ab_age299_compile");
    let disposition = oulipoly_state::EffectiveTerminalDisposition {
        success: false,
        exit_code: 1,
        error_category: Some("process_integrity".to_string()),
        terminal_reason: Some("compile-only typed representation".to_string()),
    };
    assert!(!disposition.success);

    let start_request_type = std::any::type_name::<InvocationLifecycleStartRequest<'static>>();
    let finalize_request_type =
        std::any::type_name::<InvocationLifecycleFinalizeRequest<'static>>();
    assert!(start_request_type.contains("InvocationLifecycleStartRequest"));
    assert!(finalize_request_type.contains("InvocationLifecycleFinalizeRequest"));
}

#[test]
fn age_36_resume_session_migration_services_are_object_safe_with_contract_dtos() {
    let resume: Arc<dyn ResumeServicePort + Send + Sync> = Arc::new(StubService);
    let session_lifecycle: Arc<dyn SessionLifecycleServicePort + Send + Sync> =
        Arc::new(StubService);
    let migration: Arc<dyn MigrationServicePort + Send + Sync> = Arc::new(StubService);
    let production_resume: Arc<dyn ResumeServicePort + Send + Sync> =
        Arc::new(ProductionResumeService::new());
    let _production_session_lifecycle: Arc<dyn SessionLifecycleServicePort + Send + Sync> =
        Arc::new(ProductionSessionLifecycleService::new());
    let _production_migration: Arc<dyn MigrationServicePort + Send + Sync> =
        Arc::new(ProductionMigrationService::new());

    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = ModelConfig {
        name: "age36-compile".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![],
        inputs: vec![],
        provider: None,
    };
    let mut models = ModelStore::new();
    models.insert(model.name.clone(), model.clone());
    let sessions_cfg = SessionsConfig::default();
    let providers_cfg = ProvidersConfig::default();
    let resolved = ResolvedResume {
        chain_id: "11111111-1111-4111-8111-111111111111".to_string(),
        model_name: Some(model.name.clone()),
        model: Some(model.clone()),
        active_provider: "compile-provider".to_string(),
        active_session_id: "22222222-2222-4222-8222-222222222222".to_string(),
    };
    let start = InvocationStart {
        invocation_uuid: "33333333-3333-4333-8333-333333333333".to_string(),
        model_name: model.name.clone(),
        provider_name: "compile-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let invocation_row_id = db.start_invocation(&start).unwrap();
    let mut stderr = Vec::new();
    let mut migration_stderr = Vec::new();

    let resume_request = ResumeServiceRequest {
        state: &db,
        models: &models,
        providers_cfg: &providers_cfg,
        input: "33333333-3333-4333-8333-333333333333",
        model_override: Some(&model.name),
    };
    let acceptance_request = ResumeAcceptanceRequest {
        state: &db,
        invocation_row_id,
        status: "accepted",
        evidence: Some("compile-only"),
    };
    let session_request = SessionLifecycleRequest {
        state: &db,
        sessions_cfg: &sessions_cfg,
        providers_cfg: None,
        provider_name: "compile-provider",
        external_provider: None,
        invocation_row_id,
        invocation_uuid: &start.invocation_uuid,
        effective_cwd: None,
        mode: SessionLifecycleIngestMode::Unpinned {
            capture_method: "compile-only".to_string(),
        },
        stderr: &mut stderr,
    };
    let migration_request = MigrationServiceRequest {
        state: &db,
        sessions_cfg: &sessions_cfg,
        resolved: &resolved,
        manual_target: None,
        active_exhausted: false,
        migration_model: &model,
        effective_cwd: Path::new("."),
        stderr: &mut migration_stderr,
    };

    if false {
        let _: Result<ResumeServiceOutput, ServiceError> = resume.resolve_resume(resume_request);
        let _: Result<SessionLifecycleOutput, ServiceError> =
            session_lifecycle.ingest_session(session_request);
        let _: Result<MigrationServiceOutput, ServiceError> = migration.migrate(migration_request);
    }

    let output = production_resume
        .record_acceptance(acceptance_request)
        .expect("production resume service records acceptance");
    assert_eq!(output, ResumeAcceptanceOutput);
}

#[test]
fn age_37_trace_export_replace_lock_services_are_object_safe_with_contract_dtos() {
    let trace: Arc<dyn TraceServicePort + Send + Sync> = Arc::new(StubService);
    let export: Arc<dyn SessionExportServicePort + Send + Sync> = Arc::new(StubService);
    let replace: Arc<dyn SessionReplaceServicePort + Send + Sync> = Arc::new(StubService);
    let lock: Arc<dyn SessionLockServicePort + Send + Sync> = Arc::new(StubService);

    let _production_trace: Arc<dyn TraceServicePort + Send + Sync> =
        Arc::new(ProductionTraceService::default());
    let _production_export: Arc<dyn SessionExportServicePort + Send + Sync> =
        Arc::new(ProductionSessionExportService::default());
    let _production_replace: Arc<dyn SessionReplaceServicePort + Send + Sync> =
        Arc::new(ProductionSessionReplaceService::default());
    let _production_lock: Arc<dyn SessionLockServicePort + Send + Sync> =
        Arc::new(ProductionSessionLockService::default());

    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let sessions_cfg = SessionsConfig::default();

    let trace_request = TraceServiceRequest {
        state: &db,
        sessions_cfg: &sessions_cfg,
        invocation_uuid: "11111111-1111-4111-8111-111111111111",
        options: TraceOptions {
            max_depth: 64,
            json: true,
            inline_transcript: false,
            transcript: false,
        },
    };
    let export_request = SessionExportServiceRequest {
        session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
        external_provider: None,
    };
    let replace_request = SessionReplaceServiceRequest {
        session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
        source: oulipoly_runtime::session_replace::ReplaceSource::Stdin,
        preimage_sha256: Some("0".repeat(64)),
        external_provider: None,
    };
    let acquire_request = SessionLockServiceRequest::Acquire {
        session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
        ttl_ms: 30_000,
    };
    let release_request = SessionLockServiceRequest::Release {
        session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
        token: "pause_00000000000000000000000000000000".to_string(),
    };

    if false {
        let _: Result<TraceServiceOutput, ServiceError> = trace.trace(trace_request);
        let _: Result<SessionExportServiceOutput, ServiceError> =
            export.export_session(export_request);
        let _: Result<SessionReplaceServiceOutput, ServiceError> =
            replace.replace_session(replace_request);
        let _: Result<SessionLockServiceOutput, ServiceError> = lock.lock_session(acquire_request);
        let _: Result<SessionLockServiceOutput, ServiceError> = lock.lock_session(release_request);
    }
}
