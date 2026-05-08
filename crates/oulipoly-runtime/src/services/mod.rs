pub mod error;

pub use error::ServiceError;

macro_rules! service_dto {
    ($($name:ident),+ $(,)?) => {
        $(
            #[derive(Debug, Clone, Default, PartialEq, Eq)]
            pub struct $name;
        )+
    };
}

service_dto!(
    ConfigServiceRequest,
    ConfigServiceOutput,
    ExecutorServiceRequest,
    ExecutorServiceOutput,
    LauncherServiceRequest,
    LauncherServiceOutput,
    QuotaServiceRequest,
    QuotaServiceOutput,
    RoutingServiceRequest,
    RoutingServiceOutput,
    InvocationLifecycleRequest,
    InvocationLifecycleOutput,
    SessionLifecycleRequest,
    SessionLifecycleOutput,
    ResumeServiceRequest,
    ResumeServiceOutput,
    DiagnosticsServiceRequest,
    DiagnosticsServiceOutput,
    MigrationServiceRequest,
    MigrationServiceOutput,
    TraceServiceRequest,
    TraceServiceOutput,
    SessionExportServiceRequest,
    SessionExportServiceOutput,
    SessionReplaceServiceRequest,
    SessionReplaceServiceOutput,
    SessionLockServiceRequest,
    SessionLockServiceOutput,
    MigrationMaintenanceServiceRequest,
    MigrationMaintenanceServiceOutput,
);

pub trait ConfigServicePort: Send + Sync {
    fn load_config(
        &self,
        request: ConfigServiceRequest,
    ) -> Result<ConfigServiceOutput, ServiceError>;
}

pub trait ExecutorServicePort: Send + Sync {
    fn execute(
        &self,
        request: ExecutorServiceRequest,
    ) -> Result<ExecutorServiceOutput, ServiceError>;
}

pub trait LauncherServicePort: Send + Sync {
    fn launch(
        &self,
        request: LauncherServiceRequest,
    ) -> Result<LauncherServiceOutput, ServiceError>;
}

pub trait QuotaServicePort: Send + Sync {
    fn refresh_quota(
        &self,
        request: QuotaServiceRequest,
    ) -> Result<QuotaServiceOutput, ServiceError>;
}

pub trait RoutingServicePort: Send + Sync {
    fn select_route(
        &self,
        request: RoutingServiceRequest,
    ) -> Result<RoutingServiceOutput, ServiceError>;
}

pub trait InvocationLifecycleServicePort: Send + Sync {
    fn record_invocation(
        &self,
        request: InvocationLifecycleRequest,
    ) -> Result<InvocationLifecycleOutput, ServiceError>;
}

pub trait SessionLifecycleServicePort: Send + Sync {
    fn ingest_session(
        &self,
        request: SessionLifecycleRequest,
    ) -> Result<SessionLifecycleOutput, ServiceError>;
}

pub trait ResumeServicePort: Send + Sync {
    fn resolve_resume(
        &self,
        request: ResumeServiceRequest,
    ) -> Result<ResumeServiceOutput, ServiceError>;
}

pub trait DiagnosticsServicePort: Send + Sync {
    fn diagnose(
        &self,
        request: DiagnosticsServiceRequest,
    ) -> Result<DiagnosticsServiceOutput, ServiceError>;
}

pub trait MigrationServicePort: Send + Sync {
    fn migrate(
        &self,
        request: MigrationServiceRequest,
    ) -> Result<MigrationServiceOutput, ServiceError>;
}

pub trait TraceServicePort: Send + Sync {
    fn trace(&self, request: TraceServiceRequest) -> Result<TraceServiceOutput, ServiceError>;
}

pub trait SessionExportServicePort: Send + Sync {
    fn export_session(
        &self,
        request: SessionExportServiceRequest,
    ) -> Result<SessionExportServiceOutput, ServiceError>;
}

pub trait SessionReplaceServicePort: Send + Sync {
    fn replace_session(
        &self,
        request: SessionReplaceServiceRequest,
    ) -> Result<SessionReplaceServiceOutput, ServiceError>;
}

pub trait SessionLockServicePort: Send + Sync {
    fn lock_session(
        &self,
        request: SessionLockServiceRequest,
    ) -> Result<SessionLockServiceOutput, ServiceError>;
}

pub trait MigrationMaintenanceServicePort: Send + Sync {
    fn run_maintenance(
        &self,
        request: MigrationMaintenanceServiceRequest,
    ) -> Result<MigrationMaintenanceServiceOutput, ServiceError>;
}
