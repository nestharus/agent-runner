//! ## Declared roles
//!
//! Roles: formatter, mapper, orchestration, parser.
//!
//! - orchestration: records verified child process identity in the independent
//!   PID sidecar after successful provider spawns.
//! - mapper: derives sidecar metadata from the existing parent-invocation env
//!   payload threaded through executor launches.

use oulipoly_state::CompositeInvocationId;
use oulipoly_state::mailbox::{
    AdvanceRuntimeGenerationDrain, AttachRuntimeGenerationSession, BindRuntimeGenerationRunning,
    CreateRuntimeGeneration, DrainAdvanceResult, DrainFinishResult, DrainHandoff, DrainRequestId,
    DrainRequestResult, ExactProcessEvidence, ExitRuntimeGenerationNonOrderly,
    FinishRuntimeGenerationDrain, GenerationMutation, MailboxDb, RequestRuntimeGenerationDrain,
    RuntimeGenerationFence, RuntimeGenerationId, RuntimeLifecycleState, RuntimeTerminalReason,
};
use oulipoly_state::pid_identity::{
    self, ProcessIdentity, ProcessIdentityObservation, observe_live_process_identity,
};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

const AUTO_WAKE_ENV: &str = "OULIPOLY_AUTO_WAKE";
pub(crate) const RUNNER_PRIVATE_AUTO_WAKE_ENV_NAMES: [&str; 5] = [
    AUTO_WAKE_ENV,
    "OULIPOLY_AUTO_WAKE_SESSION_ID",
    "OULIPOLY_AUTO_WAKE_TOKEN",
    "OULIPOLY_AUTO_WAKE_COUNT",
    "OULIPOLY_AUTO_WAKE_RETRY_BASE_MS",
];
const PARENT_INVOCATION_ENV: &str = "OULIPOLY_PARENT_INVOCATION";
const CHILD_CUSTODY_TEST_FAULT_ENV: &str = "OULIPOLY_CHILD_CUSTODY_TEST_FAULT";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpawnRuntimeMode {
    Headless,
    PtyInteractive,
}

impl SpawnRuntimeMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Headless => "headless",
            Self::PtyInteractive => "pty_interactive",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SpawnIdentityContext {
    generation_id: RuntimeGenerationId,
    invocation_uuid: String,
    provider_name: String,
    model_name: Option<String>,
    session_id: Option<String>,
    mode: SpawnRuntimeMode,
    pty_control_path: Option<String>,
    effective_cwd: Option<String>,
    models_dir: Option<String>,
    mailbox_db_path: Option<PathBuf>,
}

impl SpawnIdentityContext {
    pub(super) fn invocation_uuid(&self) -> &str {
        &self.invocation_uuid
    }

    pub(super) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub(super) fn model_name(&self) -> Option<&str> {
        self.model_name.as_deref()
    }

    pub(super) fn effective_cwd(&self) -> Option<&str> {
        self.effective_cwd.as_deref()
    }

    pub(super) fn with_pty_control_path(&self, path: impl Into<String>) -> Self {
        let mut cloned = self.clone();
        cloned.pty_control_path = Some(path.into());
        cloned
    }
}

pub(crate) fn context_from_parent_invocation_env(
    parent_invocation_env: Option<&str>,
    provider_name: &str,
    model_name: Option<&str>,
    session_id: Option<&str>,
    mode: SpawnRuntimeMode,
    effective_cwd: Option<&Path>,
    models_dir: Option<&Path>,
) -> Option<SpawnIdentityContext> {
    let invocation = parse_parent_invocation_env(parent_invocation_env)?;
    Some(spawn_identity_context_from_invocation(
        invocation,
        provider_name,
        model_name,
        session_id,
        mode,
        effective_cwd,
        models_dir,
    ))
}

pub(crate) fn provider_parent_invocation_env(current: Option<&str>) -> Option<String> {
    let auto_wake = std::env::var(AUTO_WAKE_ENV).ok().as_deref() == Some("1");
    let inherited = std::env::var(PARENT_INVOCATION_ENV).ok();
    provider_parent_invocation_env_for(current, auto_wake, inherited.as_deref())
}

pub(crate) fn split_invocation_launch_environment(
    value: &str,
) -> Result<(String, Option<String>), String> {
    let mut parsed: serde_json::Value = serde_json::from_str(value)
        .map_err(|error| format!("Invalid invocation launch environment: {error}"))?;
    let authority = parsed
        .as_object_mut()
        .and_then(|object| {
            object.remove(oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_LAUNCH_FIELD)
        })
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| {
                    "Invocation completion registration authority must be text".to_string()
                })
                .and_then(|secret| {
                    oulipoly_state::CompletionRegistrationAuthority::from_process_environment_value(
                        secret,
                    )
                    .map(|authority| authority.process_environment_value().to_string())
                })
        })
        .transpose()?;
    let identity = if authority.is_some() {
        let invocation = serde_json::from_value::<CompositeInvocationId>(parsed)
            .map_err(|error| format!("Invalid invocation identity environment: {error}"))?;
        serde_json::to_string(&invocation).map_err(|error| {
            format!("Failed to serialize invocation identity environment: {error}")
        })?
    } else {
        value.to_string()
    };
    Ok((identity, authority))
}

fn provider_parent_invocation_env_for(
    current: Option<&str>,
    auto_wake: bool,
    inherited: Option<&str>,
) -> Option<String> {
    if auto_wake
        && let Some(inherited) = inherited
        && CompositeInvocationId::parse_env_value(inherited).is_ok()
    {
        return Some(inherited.to_string());
    }
    current.map(str::to_string)
}

fn parse_parent_invocation_env(
    parent_invocation_env: Option<&str>,
) -> Option<CompositeInvocationId> {
    parent_invocation_env
        .and_then(strip_completion_registration_launch_authority)
        .as_deref()
        .and_then(parse_invocation_env_silent)
}

fn strip_completion_registration_launch_authority(value: &str) -> Option<String> {
    split_invocation_launch_environment(value)
        .ok()
        .map(|(identity, _)| identity)
}

fn spawn_identity_context_from_invocation(
    invocation: CompositeInvocationId,
    provider_name: &str,
    model_name: Option<&str>,
    session_id: Option<&str>,
    mode: SpawnRuntimeMode,
    effective_cwd: Option<&Path>,
    models_dir: Option<&Path>,
) -> SpawnIdentityContext {
    SpawnIdentityContext {
        generation_id: RuntimeGenerationId::new(),
        invocation_uuid: invocation.id,
        provider_name: provider_name.to_string(),
        model_name: model_name.map(str::to_string),
        session_id: session_id.map(str::to_string),
        mode,
        pty_control_path: None,
        effective_cwd: effective_cwd.map(|path| path.to_string_lossy().into_owned()),
        models_dir: models_dir.map(|path| path.to_string_lossy().into_owned()),
        mailbox_db_path: None,
    }
}

impl SpawnIdentityContext {
    pub(super) fn with_mailbox_db_path(mut self, path: PathBuf) -> Self {
        self.mailbox_db_path = Some(path);
        self
    }

    pub(super) fn open_mailbox(&self) -> Result<MailboxDb, String> {
        match self.mailbox_db_path.as_deref() {
            Some(path) => MailboxDb::open(path),
            None => MailboxDb::open_default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunningRuntimeGeneration {
    pub generation_id: RuntimeGenerationId,
    pub spawned_os_pid: i64,
    pub exact_process_identity: ProcessIdentity,
}

pub(crate) struct ChildGenerationCustody<'a> {
    child: Option<Child>,
    context: Option<&'a SpawnIdentityContext>,
    #[cfg(unix)]
    signal_guard: Option<super::terminal_signal::InteractiveSignalGuard>,
    exit_observed: bool,
    generation_completed: bool,
}

impl<'a> ChildGenerationCustody<'a> {
    pub(crate) fn new(
        child: Child,
        context: Option<&'a SpawnIdentityContext>,
    ) -> Result<Self, String> {
        Ok(Self {
            child: Some(child),
            context,
            #[cfg(unix)]
            signal_guard: None,
            exit_observed: false,
            generation_completed: false,
        })
    }

    pub(crate) fn child(&self) -> &Child {
        self.child.as_ref().expect("child custody is armed")
    }

    pub(crate) fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("child custody is armed")
    }

    #[cfg(unix)]
    pub(crate) fn install_signal_forwarding(&mut self) -> Result<(), String> {
        self.signal_guard = Some(
            super::terminal_signal::InteractiveSignalGuard::install_process_group(
                self.child().id(),
            )?,
        );
        Ok(())
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        #[cfg(unix)]
        {
            if !exact_generation_exit_pending(self.child())? {
                return Ok(None);
            }
            self.signal_guard.take();
            let child = self.child.as_mut().expect("child custody is armed");
            // WNOWAIT keeps the exact leader unreaped while descendants are cleaned.
            let status = terminate_and_reap_exact_generation(child)?;
            self.exit_observed = true;
            Ok(Some(status))
        }
        #[cfg(not(unix))]
        {
            let status = self.child_mut().try_wait()?;
            if status.is_some() {
                self.exit_observed = true;
            }
            Ok(status)
        }
    }

    pub(crate) fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub(crate) fn terminate_and_wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        #[cfg(unix)]
        self.signal_guard.take();
        let status = terminate_and_reap_exact_generation(self.child_mut())?;
        self.exit_observed = true;
        Ok(status)
    }

    pub(crate) fn observe_exit(&mut self) -> Result<(), String> {
        self.exit_observed = true;
        Ok(())
    }

    pub(crate) fn complete_orderly(
        mut self,
        exit_code: Option<i32>,
        compatibility_exit_code: Option<i32>,
    ) -> Result<(), String> {
        if !self.exit_observed {
            return Err("Cannot complete child generation before observing exit".to_string());
        }
        mark_runtime_generation_orderly_completed(
            self.context,
            exit_code,
            compatibility_exit_code,
        )?;
        self.generation_completed = true;
        #[cfg(unix)]
        self.signal_guard.take();
        self.child.take();
        Ok(())
    }
}

fn signal_owned_generation(child: &mut Child) {
    #[cfg(target_os = "linux")]
    {
        let process_group = child.id() as libc::pid_t;
        if unsafe { libc::killpg(process_group, libc::SIGKILL) } == 0 {
            return;
        }
    }
    let _ = child.kill();
}

trait ExactGenerationFinalizer {
    type Exit;

    fn terminate_owned_generation(&mut self);
    fn reap_exact_leader(&mut self) -> std::io::Result<Self::Exit>;
}

impl ExactGenerationFinalizer for Child {
    type Exit = std::process::ExitStatus;

    fn terminate_owned_generation(&mut self) {
        signal_owned_generation(self);
    }

    fn reap_exact_leader(&mut self) -> std::io::Result<Self::Exit> {
        self.wait()
    }
}

fn terminate_and_reap_exact_generation<T: ExactGenerationFinalizer>(
    generation: &mut T,
) -> std::io::Result<T::Exit> {
    generation.terminate_owned_generation();
    generation.reap_exact_leader()
}

impl Drop for ChildGenerationCustody<'_> {
    fn drop(&mut self) {
        let mut observed_exit_code = None;
        #[cfg(unix)]
        self.signal_guard.take();
        if let Some(child) = self.child.as_mut() {
            if !self.exit_observed {
                if let Ok(status) = terminate_and_reap_exact_generation(child) {
                    observed_exit_code = status.code();
                    self.exit_observed = true;
                }
            } else if let Ok(status) = child.wait() {
                observed_exit_code = status.code();
                self.exit_observed = true;
            }
        }
        if !self.generation_completed {
            let _ = mark_runtime_generation_exited(self.context, observed_exit_code);
        }
    }
}

#[cfg(unix)]
fn exact_generation_exit_pending(child: &Child) -> std::io::Result<bool> {
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    Ok(unsafe { info.si_pid() } != 0)
}

pub(crate) fn child_custody_test_fault(site: &str) -> Result<(), String> {
    if std::env::var(CHILD_CUSTODY_TEST_FAULT_ENV).ok().as_deref() == Some(site) {
        wait_for_child_custody_test_ready()?;
        return Err(format!("injected child custody failure at {site}"));
    }
    Ok(())
}

fn wait_for_child_custody_test_ready() -> Result<(), String> {
    let Some(path) = std::env::var_os("OULIPOLY_CHILD_CUSTODY_TEST_READY_FILE") else {
        return Ok(());
    };
    let path = PathBuf::from(path);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if path.is_file() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Err(format!(
        "timed out waiting for child custody test readiness at {}",
        path.display()
    ))
}

pub(crate) fn register_runtime_generation_starting(
    context: Option<&SpawnIdentityContext>,
) -> Result<(), String> {
    let Some(context) = context else {
        return Ok(());
    };
    let mut db = context.open_mailbox()?;
    recover_stale_session_generations(&mut db, context)?;
    match db
        .runtime_lifecycle()
        .create_runtime_generation(CreateRuntimeGeneration {
            generation_id: &context.generation_id,
            spawn_invocation_uuid: &context.invocation_uuid,
            session_id: context.session_id.as_deref(),
            runtime_mode: context.mode.as_str(),
            provider_name: &context.provider_name,
            model_name: context.model_name.as_deref(),
            pty_control_path: context.pty_control_path.as_deref(),
            models_dir: context.models_dir.as_deref(),
            effective_cwd: context.effective_cwd.as_deref(),
        })
        .map_err(|err| err.to_string())?
    {
        GenerationMutation::Applied(_) | GenerationMutation::AlreadyApplied(_) => Ok(()),
        GenerationMutation::Rejected(rejection) => Err(format!(
            "Runtime generation starting registration rejected: {rejection:?}"
        )),
    }
}

fn recover_stale_session_generations(
    db: &mut MailboxDb,
    context: &SpawnIdentityContext,
) -> Result<(), String> {
    let Some(session_id) = context.session_id() else {
        return Ok(());
    };
    let generations = db
        .runtime_lifecycle_reader()
        .runtime_generation_history(session_id)
        .map_err(|err| err.to_string())?;
    for generation in generations {
        if generation.lifecycle_state == RuntimeLifecycleState::Exited {
            continue;
        }
        let liveness_evidence = if generation.lifecycle_state == RuntimeLifecycleState::Starting {
            &generation.creator_process_evidence
        } else {
            &generation.exact_process_evidence
        };
        let ExactProcessEvidence::Recorded(identity) = liveness_evidence else {
            continue;
        };
        let stale = match observe_live_process_identity(identity.os_pid) {
            ProcessIdentityObservation::ExactLive(live) => live != *identity,
            ProcessIdentityObservation::Dead => true,
            ProcessIdentityObservation::Unsupported | ProcessIdentityObservation::ReadError(_) => {
                false
            }
        };
        if !stale {
            continue;
        }
        let mutation = db
            .runtime_lifecycle()
            .exit_runtime_generation_non_orderly(ExitRuntimeGenerationNonOrderly {
                fence: RuntimeGenerationFence {
                    generation_id: &generation.generation_id,
                    spawn_invocation_uuid: &generation.spawn_invocation_uuid,
                },
                reason: RuntimeTerminalReason::RecoveredDead,
                exit_code: None,
            })
            .map_err(|err| err.to_string())?;
        if let GenerationMutation::Rejected(rejection) = mutation {
            return Err(format!(
                "Stale runtime generation recovery rejected: {rejection:?}"
            ));
        }
    }
    Ok(())
}

pub(crate) fn record_child_identity(
    child_id: u32,
    context: Option<&SpawnIdentityContext>,
) -> Result<Option<RunningRuntimeGeneration>, String> {
    let Some(context) = context else {
        return Ok(None);
    };
    child_custody_test_fault("identity_capture")?;
    let os_pid = i64::from(child_id);
    let exact_process_identity = match pid_identity::read_live_process_identity(os_pid) {
        Ok(Some(identity)) => identity,
        Ok(None) => {
            let err = format!("Spawned child process {os_pid} is not live during identity binding");
            warn_child_identity_record_failed(context, child_id, &err);
            return Err(err);
        }
        Err(err) => {
            warn_child_identity_record_failed(context, child_id, &err);
            return Err(err);
        }
    };
    let mut db = context.open_mailbox()?;
    let mutation = db
        .runtime_lifecycle()
        .bind_runtime_generation_running(BindRuntimeGenerationRunning {
            fence: generation_fence(context),
            spawned_os_pid: os_pid,
            exact_process_identity: &exact_process_identity,
            os_pgid: None,
        })
        .map_err(|err| err.to_string())?;
    match mutation {
        GenerationMutation::Applied(_) | GenerationMutation::AlreadyApplied(_) => {
            Ok(Some(RunningRuntimeGeneration {
                generation_id: context.generation_id.clone(),
                spawned_os_pid: os_pid,
                exact_process_identity,
            }))
        }
        GenerationMutation::Rejected(rejection) => Err(format!(
            "Runtime generation child binding rejected: {rejection:?}"
        )),
    }
}

pub(crate) fn backfill_captured_session_id(
    context: Option<&SpawnIdentityContext>,
    generation: Option<&RunningRuntimeGeneration>,
    session_id: &str,
) -> Result<(), String> {
    let (Some(context), Some(generation)) = (context, generation) else {
        return Ok(());
    };
    if generation.generation_id != context.generation_id {
        return Err("Captured session generation does not match its spawn context".to_string());
    }
    let mut db = context.open_mailbox()?;
    match db
        .runtime_lifecycle()
        .attach_runtime_generation_session(AttachRuntimeGenerationSession {
            fence: generation_fence(context),
            session_id,
        })
        .map_err(|err| err.to_string())?
    {
        GenerationMutation::Applied(_) | GenerationMutation::AlreadyApplied(_) => {}
        GenerationMutation::Rejected(rejection) => {
            return Err(format!(
                "Runtime generation session attachment rejected: {rejection:?}"
            ));
        }
    }
    Ok(())
}

fn warn_child_identity_record_failed(context: &SpawnIdentityContext, child_id: u32, err: &str) {
    tracing::warn!(
        invocation_uuid = %context.invocation_uuid,
        child_pid = child_id,
        "Failed to record PID identity sidecar row: {err}"
    );
}

pub(crate) fn mark_runtime_generation_spawn_failed(
    context: Option<&SpawnIdentityContext>,
) -> Result<(), String> {
    exit_runtime_generation(context, RuntimeTerminalReason::StartupFailed, None)
}

pub(crate) fn mark_runtime_generation_exited(
    context: Option<&SpawnIdentityContext>,
    exit_code: Option<i32>,
) -> Result<(), String> {
    exit_runtime_generation(
        context,
        RuntimeTerminalReason::AbnormalTermination,
        exit_code,
    )
}

pub(crate) fn mark_runtime_generation_orderly_completed(
    context: Option<&SpawnIdentityContext>,
    exit_code: Option<i32>,
    compatibility_exit_code: Option<i32>,
) -> Result<(), String> {
    let Some(context) = context else {
        return Ok(());
    };
    let drain_request_id = DrainRequestId::new();
    let mut db = context.open_mailbox()?;
    let handoff = match db
        .runtime_lifecycle()
        .request_runtime_generation_drain(RequestRuntimeGenerationDrain {
            fence: generation_fence(context),
            drain_request_id: &drain_request_id,
            requested_by_invocation_uuid: &context.invocation_uuid,
        })
        .map_err(|err| err.to_string())?
    {
        DrainRequestResult::Installed(_, handoff)
        | DrainRequestResult::AlreadyInstalled(_, handoff) => handoff,
        DrainRequestResult::Rejected(rejection) => {
            return Err(format!(
                "Runtime generation drain request rejected: {rejection:?}"
            ));
        }
    };
    if matches!(handoff, DrainHandoff::ClaimOutstanding { .. }) {
        drop(db);
        return exit_runtime_generation(
            Some(context),
            RuntimeTerminalReason::AbnormalTermination,
            exit_code,
        );
    }
    match db
        .runtime_lifecycle()
        .advance_runtime_generation_drain(AdvanceRuntimeGenerationDrain {
            fence: generation_fence(context),
            drain_request_id: &drain_request_id,
        })
        .map_err(|err| err.to_string())?
    {
        DrainAdvanceResult::Advanced(_) | DrainAdvanceResult::AlreadyDraining(_) => {}
        DrainAdvanceResult::WaitingOnClaim(_) => {
            drop(db);
            return exit_runtime_generation(
                Some(context),
                RuntimeTerminalReason::AbnormalTermination,
                exit_code,
            );
        }
        DrainAdvanceResult::AlreadyExited(_) => return Ok(()),
        DrainAdvanceResult::Rejected(rejection) => {
            return Err(format!(
                "Runtime generation drain advance rejected: {rejection:?}"
            ));
        }
    }
    match db
        .runtime_lifecycle()
        .finish_runtime_generation_drain(FinishRuntimeGenerationDrain {
            fence: generation_fence(context),
            drain_request_id: &drain_request_id,
            exit_code,
            compatibility_exit_code,
        })
        .map_err(|err| err.to_string())?
    {
        DrainFinishResult::Finished(_) | DrainFinishResult::AlreadyExited(_) => Ok(()),
        DrainFinishResult::NotDraining(actual) => Err(format!(
            "Runtime generation was {actual:?} while finishing orderly drain"
        )),
        DrainFinishResult::Rejected(rejection) => Err(format!(
            "Runtime generation drain finish rejected: {rejection:?}"
        )),
    }
}

fn exit_runtime_generation(
    context: Option<&SpawnIdentityContext>,
    reason: RuntimeTerminalReason,
    exit_code: Option<i32>,
) -> Result<(), String> {
    let Some(context) = context else {
        return Ok(());
    };
    let mut db = context.open_mailbox()?;
    match db
        .runtime_lifecycle()
        .exit_runtime_generation_non_orderly(ExitRuntimeGenerationNonOrderly {
            fence: generation_fence(context),
            reason,
            exit_code,
        })
        .map_err(|err| err.to_string())?
    {
        GenerationMutation::Applied(_) | GenerationMutation::AlreadyApplied(_) => Ok(()),
        GenerationMutation::Rejected(rejection) => {
            Err(format!("Runtime generation exit rejected: {rejection:?}"))
        }
    }
}

fn generation_fence(context: &SpawnIdentityContext) -> RuntimeGenerationFence<'_> {
    RuntimeGenerationFence {
        generation_id: &context.generation_id,
        spawn_invocation_uuid: &context.invocation_uuid,
    }
}

fn parse_invocation_env_silent(value: &str) -> Option<CompositeInvocationId> {
    CompositeInvocationId::parse_env_value(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURRENT: &str = r#"{"source":"opencode3","id":"11111111-1111-4111-8111-111111111111"}"#;
    const OWNER: &str = r#"{"source":"opencode3","id":"22222222-2222-4222-8222-222222222222"}"#;

    #[test]
    fn auto_wake_provider_keeps_inherited_semantic_owner() {
        assert_eq!(
            provider_parent_invocation_env_for(Some(CURRENT), true, Some(OWNER)).as_deref(),
            Some(OWNER)
        );
    }

    #[test]
    fn ordinary_provider_uses_current_invocation() {
        assert_eq!(
            provider_parent_invocation_env_for(Some(CURRENT), false, Some(OWNER)).as_deref(),
            Some(CURRENT)
        );
    }

    #[test]
    fn auto_wake_rejects_malformed_inherited_owner() {
        assert_eq!(
            provider_parent_invocation_env_for(Some(CURRENT), true, Some("not-json")).as_deref(),
            Some(CURRENT)
        );
    }

    #[test]
    fn failed_child_identity_binding_never_commits_identityless_running_generation() {
        let directory = tempfile::tempdir().unwrap();
        let sidecar_path = directory.path().join("pid-identity.db");
        let context = context_from_parent_invocation_env(
            Some(CURRENT),
            "provider-a",
            Some("model-a"),
            Some("session-a"),
            SpawnRuntimeMode::Headless,
            None,
            None,
        )
        .unwrap()
        .with_mailbox_db_path(sidecar_path.clone());
        register_runtime_generation_starting(Some(&context)).unwrap();

        let mut dead_child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--help")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let dead_child_id = dead_child.id();
        dead_child.wait().unwrap();
        assert!(record_child_identity(dead_child_id, Some(&context)).is_err());

        let db = MailboxDb::open(&sidecar_path).unwrap();
        let starting = db
            .runtime_lifecycle_reader()
            .runtime_generation(&context.generation_id)
            .unwrap()
            .unwrap();
        assert_eq!(starting.lifecycle_state, RuntimeLifecycleState::Starting);
        assert_eq!(
            starting.exact_process_evidence,
            ExactProcessEvidence::NotRecorded
        );
        drop(db);

        mark_runtime_generation_exited(Some(&context), None).unwrap();
        let db = MailboxDb::open(&sidecar_path).unwrap();
        let exited = db
            .runtime_lifecycle_reader()
            .runtime_generation(&context.generation_id)
            .unwrap()
            .unwrap();
        assert_eq!(exited.lifecycle_state, RuntimeLifecycleState::Exited);
        assert_eq!(
            exited.terminal_reason,
            Some(RuntimeTerminalReason::AbnormalTermination)
        );
    }

    #[test]
    fn exact_generation_finalizer_never_signals_after_reap_and_target_substitution() {
        #[derive(Default)]
        struct SubstitutingFinalizer {
            reaped: bool,
            foreign_target_substituted: bool,
            events: Vec<&'static str>,
        }

        impl ExactGenerationFinalizer for SubstitutingFinalizer {
            type Exit = ();

            fn terminate_owned_generation(&mut self) {
                assert!(
                    !self.reaped,
                    "numeric target was signaled after leader reap"
                );
                assert!(!self.foreign_target_substituted);
                self.events.push("terminate_exact_generation");
            }

            fn reap_exact_leader(&mut self) -> std::io::Result<Self::Exit> {
                self.reaped = true;
                self.foreign_target_substituted = true;
                self.events.push("reap_and_substitute_foreign_target");
                Ok(())
            }
        }

        let mut finalizer = SubstitutingFinalizer::default();
        terminate_and_reap_exact_generation(&mut finalizer).unwrap();

        assert!(finalizer.reaped);
        assert!(finalizer.foreign_target_substituted);
        assert_eq!(
            finalizer.events,
            [
                "terminate_exact_generation",
                "reap_and_substitute_foreign_target"
            ]
        );
    }
}
