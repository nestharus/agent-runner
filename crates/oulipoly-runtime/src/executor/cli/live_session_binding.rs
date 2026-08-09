//! Provider-neutral live interactive session binding.

use super::spawn_identity::{
    RunningRuntimeGeneration, SpawnIdentityContext, backfill_captured_session_id,
};
use crate::provider_registry::ProviderRegistry;
use crate::services::emit_live_session_marker;
use crate::session_provider::{
    SessionProviderIdentity, SessionProviderLiveCaptureRequest, capture_live_report,
};
use oulipoly_state::{InvocationStatus, ProviderSessionBinding, StateDb};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const SOCKET_ENV: &str = "OULIPOLY_LIVE_SESSION_BIND_SOCKET";
const TOKEN_ENV: &str = "OULIPOLY_LIVE_SESSION_BIND_TOKEN";
const PROTOCOL_VERSION: u8 = 1;
const CAPTURE_METHOD: &str = "provider_live_report";
const MAX_LIVE_SESSION_MESSAGE_BYTES: usize = 16 * 1024;
const ACCEPT_POLL: Duration = Duration::from_millis(10);
const IO_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(crate) struct InteractiveLiveSessionBinding {
    pub registry: Arc<ProviderRegistry>,
    pub identity: SessionProviderIdentity,
    pub state_db_path: PathBuf,
    pub invocation_row_id: i64,
    pub invocation_uuid: String,
    pub effective_cwd: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LiveSessionReport {
    schema_version: u8,
    token: String,
    invocation_uuid: String,
    provider_session_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct LiveSessionResponse {
    ok: bool,
    session_id: Option<String>,
    error: Option<String>,
}

#[cfg(unix)]
pub(crate) struct LiveSessionBindingServer {
    listener: Option<UnixListener>,
    socket_path: PathBuf,
    token: String,
    context: InteractiveLiveSessionBinding,
    session_id: Arc<Mutex<Option<String>>>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

#[cfg(unix)]
impl LiveSessionBindingServer {
    pub(crate) fn bind(context: InteractiveLiveSessionBinding) -> Result<Self, String> {
        let dir = live_binding_socket_dir(&context.state_db_path);
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("Failed to create live-session socket directory: {err}"))?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|err| format!("Failed to secure live-session socket directory: {err}"))?;
        let token = uuid::Uuid::new_v4().to_string();
        let socket_path = dir.join(format!("{}.sock", uuid::Uuid::new_v4()));
        let listener = UnixListener::bind(&socket_path)
            .map_err(|err| format!("Failed to bind live-session socket: {err}"))?;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("Failed to secure live-session socket: {err}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|err| format!("Failed to configure live-session socket: {err}"))?;
        Ok(Self {
            listener: Some(listener),
            socket_path,
            token,
            context,
            session_id: Arc::new(Mutex::new(None)),
            shutdown: Arc::new(AtomicBool::new(false)),
            worker: None,
        })
    }

    pub(crate) fn configure_command(&self, command: &mut Command) {
        command.env(SOCKET_ENV, &self.socket_path);
        command.env(TOKEN_ENV, &self.token);
    }

    pub(crate) fn session_state(&self) -> Arc<Mutex<Option<String>>> {
        Arc::clone(&self.session_id)
    }

    pub(crate) fn start(
        &mut self,
        spawn_context: SpawnIdentityContext,
        generation: RunningRuntimeGeneration,
    ) -> Result<(), String> {
        let listener = self
            .listener
            .take()
            .ok_or_else(|| "Live-session binding server was already started".to_string())?;
        let context = self.context.clone();
        let token = self.token.clone();
        let session_id = Arc::clone(&self.session_id);
        let shutdown = Arc::clone(&self.shutdown);
        self.worker = Some(thread::spawn(move || {
            serve_live_session_reports(
                listener,
                context,
                token,
                session_id,
                shutdown,
                spawn_context,
                generation,
            );
        }));
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for LiveSessionBindingServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

#[cfg(unix)]
fn serve_live_session_reports(
    listener: UnixListener,
    context: InteractiveLiveSessionBinding,
    token: String,
    session_state: Arc<Mutex<Option<String>>>,
    shutdown: Arc<AtomicBool>,
    spawn_context: SpawnIdentityContext,
    generation: RunningRuntimeGeneration,
) {
    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let result = handle_live_session_report(
                    &mut stream,
                    &context,
                    &token,
                    &session_state,
                    &spawn_context,
                    &generation,
                );
                let response = match result {
                    Ok(session_id) => LiveSessionResponse {
                        ok: true,
                        session_id: Some(session_id),
                        error: None,
                    },
                    Err(error) => LiveSessionResponse {
                        ok: false,
                        session_id: None,
                        error: Some(error),
                    },
                };
                let _ = write_response(&mut stream, &response);
                if response.ok {
                    return;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(_) => return,
        }
    }
}

#[cfg(unix)]
fn handle_live_session_report(
    stream: &mut UnixStream,
    context: &InteractiveLiveSessionBinding,
    token: &str,
    session_state: &Arc<Mutex<Option<String>>>,
    spawn_context: &SpawnIdentityContext,
    generation: &RunningRuntimeGeneration,
) -> Result<String, String> {
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|err| format!("Failed to configure live-session report read timeout: {err}"))?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|err| format!("Failed to configure live-session report write timeout: {err}"))?;
    let report = read_report(stream)?;
    validate_report(&report, context, token)?;
    let capture = capture_live_report(SessionProviderLiveCaptureRequest {
        registry: context.registry.as_ref(),
        identity: context.identity.clone(),
        invocation_uuid: &context.invocation_uuid,
        provider_session_id: &report.provider_session_id,
        effective_cwd: context.effective_cwd.as_deref(),
    })
    .map_err(|err| format!("Provider rejected live session report: {err}"))?;
    let captured = capture
        .provider_session_id
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Provider accepted a live report without a session identity".to_string())?;
    if captured != report.provider_session_id {
        return Err(format!(
            "Provider live-session identity mismatch: reported {}, captured {}",
            report.provider_session_id, captured
        ));
    }
    persist_live_binding(context, &captured)?;
    backfill_captured_session_id(Some(spawn_context), Some(generation), &captured)?;
    set_shared_session(session_state, &captured)?;
    emit_live_session_marker(
        &StateDb::open(&context.state_db_path)?,
        context.invocation_row_id,
        &context.invocation_uuid,
        &captured,
        CAPTURE_METHOD,
    )?;
    Ok(captured)
}

#[cfg(unix)]
fn read_report(stream: &mut UnixStream) -> Result<LiveSessionReport, String> {
    let mut line = String::new();
    BufReader::new(stream)
        .take(MAX_LIVE_SESSION_MESSAGE_BYTES as u64 + 1)
        .read_line(&mut line)
        .map_err(|err| format!("Failed to read live-session report: {err}"))?;
    if line.len() > MAX_LIVE_SESSION_MESSAGE_BYTES {
        return Err("Live-session report exceeded the size limit".to_string());
    }
    serde_json::from_str(&line).map_err(|err| format!("Invalid live-session report: {err}"))
}

fn validate_report(
    report: &LiveSessionReport,
    context: &InteractiveLiveSessionBinding,
    token: &str,
) -> Result<(), String> {
    if report.schema_version != PROTOCOL_VERSION {
        return Err("Unsupported live-session report protocol version".to_string());
    }
    if report.token != token {
        return Err("Live-session report token mismatch".to_string());
    }
    if report.invocation_uuid != context.invocation_uuid {
        return Err("Live-session report invocation mismatch".to_string());
    }
    if report.provider_session_id.trim().is_empty() {
        return Err("Live-session report session ID is empty".to_string());
    }
    Ok(())
}

fn persist_live_binding(
    context: &InteractiveLiveSessionBinding,
    provider_session_id: &str,
) -> Result<(), String> {
    let state = StateDb::open(&context.state_db_path)?;
    let record = state
        .get_invocation_by_uuid(&context.invocation_uuid)?
        .ok_or_else(|| "Live-session invocation does not exist".to_string())?;
    if record.id != context.invocation_row_id {
        return Err("Live-session invocation row changed".to_string());
    }
    if record.provider_name.as_deref() != Some(context.identity.provider_name.as_str()) {
        return Err("Live-session provider does not match the invocation".to_string());
    }
    if record.status != InvocationStatus::Running {
        return Err("Live-session invocation is no longer running".to_string());
    }
    state.bind_invocation_provider_session_start(
        context.invocation_row_id,
        &ProviderSessionBinding {
            provider_session_id: provider_session_id.to_string(),
            capture_method: CAPTURE_METHOD,
            resume_input_id: None,
            provider_session_resolved_account: Some(context.identity.settings_id.clone()),
        },
    )
}

fn set_shared_session(
    session_state: &Arc<Mutex<Option<String>>>,
    provider_session_id: &str,
) -> Result<(), String> {
    let mut state = session_state
        .lock()
        .map_err(|_| "Live-session state lock was poisoned".to_string())?;
    if let Some(existing) = state.as_deref()
        && existing != provider_session_id
    {
        return Err(format!(
            "Live-session state is already bound to {existing}; refusing {provider_session_id}"
        ));
    }
    *state = Some(provider_session_id.to_string());
    Ok(())
}

#[cfg(unix)]
fn write_response(stream: &mut UnixStream, response: &LiveSessionResponse) -> Result<(), String> {
    serde_json::to_writer(&mut *stream, response)
        .map_err(|err| format!("Failed to encode live-session response: {err}"))?;
    stream
        .write_all(b"\n")
        .map_err(|err| format!("Failed to write live-session response: {err}"))
}

fn live_binding_socket_dir(state_db_path: &Path) -> PathBuf {
    state_db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("runtime/live-session")
}

#[cfg(unix)]
pub fn report_live_session_binding_from_env(
    invocation_uuid: &str,
    provider_session_id: &str,
) -> Result<bool, String> {
    let socket = std::env::var_os(SOCKET_ENV);
    let token = std::env::var(TOKEN_ENV).ok();
    match (socket, token) {
        (None, None) => Ok(false),
        (Some(_), None) | (None, Some(_)) => {
            Err("Live-session binding environment is incomplete".to_string())
        }
        (Some(socket), Some(token)) => report_live_session_binding(
            Path::new(&socket),
            &token,
            invocation_uuid,
            provider_session_id,
        ),
    }
}

#[cfg(unix)]
fn report_live_session_binding(
    socket: &Path,
    token: &str,
    invocation_uuid: &str,
    provider_session_id: &str,
) -> Result<bool, String> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|err| format!("Failed to connect to live-session binding owner: {err}"))?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|err| format!("Failed to configure live-session response timeout: {err}"))?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|err| format!("Failed to configure live-session request timeout: {err}"))?;
    let report = LiveSessionReport {
        schema_version: PROTOCOL_VERSION,
        token: token.to_string(),
        invocation_uuid: invocation_uuid.to_string(),
        provider_session_id: provider_session_id.to_string(),
    };
    serde_json::to_writer(&mut stream, &report)
        .map_err(|err| format!("Failed to encode live-session report: {err}"))?;
    stream
        .write_all(b"\n")
        .map_err(|err| format!("Failed to write live-session report: {err}"))?;
    let response: LiveSessionResponse = serde_json::from_reader(
        BufReader::new(stream).take(MAX_LIVE_SESSION_MESSAGE_BYTES as u64 + 1),
    )
    .map_err(|err| format!("Invalid live-session binding response: {err}"))?;
    if response.ok && response.session_id.as_deref() == Some(provider_session_id) {
        return Ok(true);
    }
    Err(response
        .error
        .unwrap_or_else(|| "Live-session binding was rejected".to_string()))
}

#[cfg(not(unix))]
pub fn report_live_session_binding_from_env(
    _invocation_uuid: &str,
    _provider_session_id: &str,
) -> Result<bool, String> {
    Ok(false)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::executor::cli::spawn_identity::{
        SpawnRuntimeMode, context_from_parent_invocation_env, record_child_identity,
        register_runtime_generation_starting,
    };
    use crate::provider_registry::ProviderRegistryOptions;
    use oulipoly_config::provider_implementation_ref::ProviderImplementationRef;
    use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
    use oulipoly_state::mailbox::MailboxDb;
    use oulipoly_state::pid_identity::PidIdentityDb;
    use oulipoly_state::{CompositeInvocationId, InvocationStart};
    use std::os::unix::fs::PermissionsExt;

    const INVOCATION_UUID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const SESSION_ID: &str = "session-live-fixture";
    const PROVIDER_NAME: &str = "fixture-account";
    const MODEL_NAME: &str = "fixture-model";

    #[test]
    fn exact_live_report_binds_state_chain_and_runtime_generation() {
        let fixture = LiveBindingFixture::new();

        let wrong_invocation = report_live_session_binding(
            &fixture.server.socket_path,
            &fixture.server.token,
            "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
            SESSION_ID,
        )
        .expect_err("wrong invocation must fail closed");
        assert!(wrong_invocation.contains("invocation mismatch"));

        assert!(
            report_live_session_binding(
                &fixture.server.socket_path,
                &fixture.server.token,
                INVOCATION_UUID,
                SESSION_ID,
            )
            .expect("exact report should bind")
        );

        let state = StateDb::open(&fixture.state_path).unwrap();
        let invocation = state
            .get_invocation_by_uuid(INVOCATION_UUID)
            .unwrap()
            .unwrap();
        assert_eq!(invocation.provider_session_id.as_deref(), Some(SESSION_ID));
        assert_eq!(
            invocation.provider_session_capture_method.as_deref(),
            Some(CAPTURE_METHOD)
        );
        assert!(
            state
                .chain_id_for_segment(PROVIDER_NAME, SESSION_ID)
                .unwrap()
                .is_some()
        );

        let sidecar =
            PidIdentityDb::open(&MailboxDb::path_for_state_db(&fixture.state_path)).unwrap();
        let runtime = sidecar
            .lookup_by_identity(&fixture.process_identity)
            .unwrap()
            .unwrap();
        assert_eq!(runtime.invocation_uuid, INVOCATION_UUID);
        assert_eq!(runtime.session_id.as_deref(), Some(SESSION_ID));
    }

    #[test]
    fn invalid_token_does_not_consume_the_binding_socket() {
        let fixture = LiveBindingFixture::new();

        let error = report_live_session_binding(
            &fixture.server.socket_path,
            "wrong-token",
            INVOCATION_UUID,
            SESSION_ID,
        )
        .expect_err("wrong token must fail closed");
        assert!(error.contains("token mismatch"));

        assert!(
            report_live_session_binding(
                &fixture.server.socket_path,
                &fixture.server.token,
                INVOCATION_UUID,
                SESSION_ID,
            )
            .expect("valid retry should bind")
        );
    }

    #[test]
    fn concurrent_conflicting_reports_only_bind_the_provider_validated_session() {
        let fixture = LiveBindingFixture::new();
        let socket_path = fixture.server.socket_path.clone();
        let token = fixture.server.token.clone();
        let barrier = Arc::new(std::sync::Barrier::new(3));

        let valid_barrier = Arc::clone(&barrier);
        let valid_path = socket_path.clone();
        let valid_token = token.clone();
        let valid = std::thread::spawn(move || {
            valid_barrier.wait();
            report_live_session_binding(&valid_path, &valid_token, INVOCATION_UUID, SESSION_ID)
        });

        let conflicting_barrier = Arc::clone(&barrier);
        let conflicting = std::thread::spawn(move || {
            conflicting_barrier.wait();
            report_live_session_binding(
                &socket_path,
                &token,
                INVOCATION_UUID,
                "session-conflicting-fixture",
            )
        });

        barrier.wait();
        assert!(valid.join().unwrap().expect("valid report should bind"));
        assert!(conflicting.join().unwrap().is_err());

        let invocation = StateDb::open(&fixture.state_path)
            .unwrap()
            .get_invocation_by_uuid(INVOCATION_UUID)
            .unwrap()
            .unwrap();
        assert_eq!(invocation.provider_session_id.as_deref(), Some(SESSION_ID));
    }

    #[test]
    fn multi_kilobyte_session_id_round_trips_without_truncation() {
        let session_id = format!("ses_{}", "x".repeat(4 * 1024));
        let fixture = LiveBindingFixture::new_with_session_id(&session_id);

        assert!(
            report_live_session_binding(
                &fixture.server.socket_path,
                &fixture.server.token,
                INVOCATION_UUID,
                &session_id,
            )
            .expect("multi-kilobyte session ID should bind")
        );

        let invocation = StateDb::open(&fixture.state_path)
            .unwrap()
            .get_invocation_by_uuid(INVOCATION_UUID)
            .unwrap()
            .unwrap();
        assert_eq!(
            invocation.provider_session_id.as_deref(),
            Some(session_id.as_str())
        );
    }

    struct LiveBindingFixture {
        _temp: tempfile::TempDir,
        state_path: PathBuf,
        server: LiveSessionBindingServer,
        process_identity: oulipoly_state::pid_identity::ProcessIdentity,
        child: std::process::Child,
    }

    impl LiveBindingFixture {
        fn new() -> Self {
            Self::new_with_session_id(SESSION_ID)
        }

        fn new_with_session_id(session_id: &str) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let state_path = temp.path().join("state.db");
            let state = StateDb::open(&state_path).unwrap();
            let invocation_row_id = state
                .start_invocation(&InvocationStart {
                    invocation_uuid: INVOCATION_UUID.to_string(),
                    model_name: MODEL_NAME.to_string(),
                    provider_name: PROVIDER_NAME.to_string(),
                    provider_index: 0,
                    parent_invocation_id: None,
                })
                .unwrap();
            let provider = fake_provider(temp.path(), session_id);
            let registry = Arc::new(
                ProviderRegistry::from_model_configs(
                    &[fixture_model(&provider)],
                    ProviderRegistryOptions::default(),
                )
                .unwrap(),
            );
            let context = InteractiveLiveSessionBinding {
                registry,
                identity: SessionProviderIdentity {
                    model_name: MODEL_NAME.to_string(),
                    provider_name: PROVIDER_NAME.to_string(),
                    provider_instance_id: None,
                    settings_id: PROVIDER_NAME.to_string(),
                },
                state_db_path: state_path.clone(),
                invocation_row_id,
                invocation_uuid: INVOCATION_UUID.to_string(),
                effective_cwd: Some(temp.path().to_path_buf()),
            };
            let mut server = LiveSessionBindingServer::bind(context).unwrap();
            let parent = serde_json::to_string(&CompositeInvocationId {
                source: PROVIDER_NAME.to_string(),
                id: INVOCATION_UUID.to_string(),
            })
            .unwrap();
            let spawn_context = context_from_parent_invocation_env(
                Some(&parent),
                PROVIDER_NAME,
                Some(MODEL_NAME),
                None,
                SpawnRuntimeMode::PtyInteractive,
                Some(temp.path()),
                None,
            )
            .unwrap()
            .with_mailbox_db_path(MailboxDb::path_for_state_db(&state_path));
            register_runtime_generation_starting(Some(&spawn_context)).unwrap();
            let child = Command::new("sleep").arg("30").spawn().unwrap();
            let generation = record_child_identity(child.id(), Some(&spawn_context))
                .unwrap()
                .unwrap();
            let process_identity = generation.exact_process_identity.clone().unwrap();
            server.start(spawn_context, generation).unwrap();
            Self {
                _temp: temp,
                state_path,
                server,
                process_identity,
                child,
            }
        }
    }

    impl Drop for LiveBindingFixture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn fixture_model(provider: &Path) -> ModelConfig {
        ModelConfig {
            name: MODEL_NAME.to_string(),
            prompt_mode: PromptMode::Stdin,
            providers: vec![ProviderConfig::model_provider(PROVIDER_NAME, Vec::new())],
            inputs: Vec::new(),
            provider: Some(ProviderImplementationRef {
                path: Some(provider.display().to_string()),
                crate_name: None,
                version: None,
                binary: None,
                script: None,
            }),
        }
    }

    fn fake_provider(dir: &Path, session_id: &str) -> PathBuf {
        let path = dir.join("fake-provider");
        let script = format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
request=$(cat)
request_id=$(printf '%s' "$request" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
case "${{1-}}" in
  describe)
    printf '{{"contract":"oulipoly.provider/v1","request_id":"%s","ok":true,"result":{{"provider_id":"fixture","display_name":"Fixture","contract_versions":["oulipoly.provider/v1"],"preferred_contract":"oulipoly.provider/v1","capabilities":{{"launch":false,"policy":false,"quota":false,"session":true,"session_enumerate":false,"terminal":false,"rotation":false,"discovery":false,"settings":false,"setup_brain":false,"setup":false,"migration":false}}}}}}\n' "$request_id"
    ;;
  session.capture)
    printf '%s' "$request" | grep -F '"live_report"' >/dev/null
    printf '%s' "$request" | grep -F '"provider_session_id":"{session_id}"' >/dev/null
    printf '{{"contract":"oulipoly.provider/v1","request_id":"%s","ok":true,"result":{{"provider_session_id":"{session_id}","state":null,"artifacts":[]}}}}\n' "$request_id"
    ;;
  *) exit 64 ;;
esac
"#
        );
        std::fs::write(&path, script).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }
}
