use oulipoly_config::{ModelConfig, PromptMode};
use oulipoly_runtime::services::error::ServiceError;
use oulipoly_runtime::services::*;
use oulipoly_state::{InvocationStart, StateDb};
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
        _request: SessionLifecycleRequest,
    ) -> Result<SessionLifecycleOutput, ServiceError> {
        unimplemented!()
    }
}

impl ResumeServicePort for StubService {
    fn resolve_resume(
        &self,
        _request: ResumeServiceRequest,
    ) -> Result<ResumeServiceOutput, ServiceError> {
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
        _request: MigrationServiceRequest,
    ) -> Result<MigrationServiceOutput, ServiceError> {
        unimplemented!()
    }
}

impl TraceServicePort for StubService {
    fn trace(&self, _request: TraceServiceRequest) -> Result<TraceServiceOutput, ServiceError> {
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
        Box::new(oulipoly_runtime::executor::RuntimeExecutorService);
    let _: Box<dyn LauncherServicePort + Send + Sync> =
        Box::new(oulipoly_runtime::repl_default_provider::RuntimeLauncherService);
    let _: Box<dyn QuotaServicePort + Send + Sync> =
        Box::new(oulipoly_runtime::quota::RuntimeQuotaService);
    let _: Box<dyn DiagnosticsServicePort + Send + Sync> =
        Box::new(oulipoly_runtime::diagnostics::RuntimeDiagnosticsService);
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
