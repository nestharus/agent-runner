//! Session ingest CLI marker and warning helpers.
//!
//! ## Declared roles
//!
//! `orchestration`, `mapper`, `formatter`, `accessor`, `filter`, `predicate`, `validator`, `parser`

use std::path::Path;

use oulipoly_config::{ModelConfig, ProvidersConfig};
use oulipoly_runtime::services::{
    ServiceError, SessionLifecycleIngestMode, SessionLifecycleOutput, SessionLifecycleRequest,
    SessionServiceExternalProviderIdentity,
};
use oulipoly_state::{InvocationRecord, StateDb};

use crate::wiring;

pub(crate) enum ResumeIngestMode<'a> {
    Unpinned { capture_method: &'a str },
    Pinned { resume_target: &'a str },
}

pub(crate) struct SessionIngestRequest<'a> {
    pub(crate) state: &'a StateDb,
    pub(crate) sessions_cfg: &'a oulipoly_config::SessionsConfig,
    pub(crate) providers_cfg: Option<&'a ProvidersConfig>,
    pub(crate) provider_name: &'a str,
    pub(crate) external_provider: Option<SessionServiceExternalProviderIdentity>,
    pub(crate) invocation_row_id: i64,
    pub(crate) invocation_uuid: &'a str,
    pub(crate) effective_cwd: Option<&'a Path>,
    pub(crate) mode: ResumeIngestMode<'a>,
}

pub(crate) fn ingest_and_emit_session_id_resume_aware(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    request: SessionIngestRequest<'_>,
) -> SessionLifecycleOutput {
    let mut stderr = std::io::stderr();
    let SessionIngestRequest {
        state,
        sessions_cfg,
        providers_cfg,
        provider_name,
        external_provider,
        invocation_row_id,
        invocation_uuid,
        effective_cwd,
        mode,
    } = request;
    let lifecycle_request = session_lifecycle_request(SessionLifecycleRequestInput {
        state,
        sessions_cfg,
        providers_cfg,
        provider_name,
        external_provider,
        invocation_row_id,
        invocation_uuid,
        effective_cwd,
        mode: session_lifecycle_ingest_mode(mode),
        stderr: &mut stderr,
    });
    emit_session_lifecycle_ingest_result(
        provider_name,
        agent_runtime_services
            .session_lifecycle_service
            .ingest_session(lifecycle_request),
    )
}

fn emit_session_lifecycle_ingest_result(
    provider_name: &str,
    result: Result<SessionLifecycleOutput, ServiceError>,
) -> SessionLifecycleOutput {
    match session_lifecycle_ingest_result(result) {
        SessionLifecycleIngestResult::Emitted(output) => output,
        SessionLifecycleIngestResult::Failed(message) => {
            emit_session_ingest_failure(provider_name, &message);
            session_lifecycle_ingest_failed_result()
        }
    }
}

enum SessionLifecycleIngestResult {
    Emitted(SessionLifecycleOutput),
    Failed(String),
}

fn session_lifecycle_ingest_result(
    result: Result<SessionLifecycleOutput, ServiceError>,
) -> SessionLifecycleIngestResult {
    match result {
        Ok(output) => SessionLifecycleIngestResult::Emitted(output),
        Err(ServiceError::Dependency { message })
        | Err(ServiceError::InvalidRequest { message })
        | Err(ServiceError::Unavailable { message }) => {
            SessionLifecycleIngestResult::Failed(message)
        }
    }
}

fn session_lifecycle_ingest_failed_result() -> SessionLifecycleOutput {
    SessionLifecycleOutput {
        emitted: false,
        session_id: None,
    }
}

struct SessionLifecycleRequestInput<'a> {
    state: &'a StateDb,
    sessions_cfg: &'a oulipoly_config::SessionsConfig,
    providers_cfg: Option<&'a ProvidersConfig>,
    provider_name: &'a str,
    external_provider: Option<SessionServiceExternalProviderIdentity>,
    invocation_row_id: i64,
    invocation_uuid: &'a str,
    effective_cwd: Option<&'a Path>,
    mode: SessionLifecycleIngestMode,
    stderr: &'a mut std::io::Stderr,
}

fn session_lifecycle_request(
    input: SessionLifecycleRequestInput<'_>,
) -> SessionLifecycleRequest<'_> {
    SessionLifecycleRequest {
        state: input.state,
        sessions_cfg: input.sessions_cfg,
        providers_cfg: input.providers_cfg,
        provider_name: input.provider_name,
        external_provider: input.external_provider,
        invocation_row_id: input.invocation_row_id,
        invocation_uuid: input.invocation_uuid,
        effective_cwd: input.effective_cwd,
        mode: input.mode,
        stderr: input.stderr,
    }
}

pub(crate) fn session_external_provider_identity(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model: Option<&ModelConfig>,
    provider_name: &str,
) -> Option<SessionServiceExternalProviderIdentity> {
    let model = external_provider_model(model)?;
    let describe = describe_external_provider_for_session(agent_runtime_services, &model.name)?;
    Some(session_service_external_provider_identity(
        model,
        provider_name,
        describe,
    ))
}

struct ExternalProviderSessionDescribe {
    provider_instance_id: Option<String>,
    settings_id: String,
}

fn describe_external_provider_for_session(
    agent_runtime_services: &wiring::AgentRuntimeServices,
    model_name: &str,
) -> Option<ExternalProviderSessionDescribe> {
    let registry = session_provider_registry(agent_runtime_services);
    require_external_provider_artifact(&registry, model_name)?;
    let describe = describe_session_provider_model(&registry, model_name);
    Some(external_provider_session_describe(describe.as_ref()))
}

fn session_provider_registry(
    agent_runtime_services: &wiring::AgentRuntimeServices,
) -> std::sync::Arc<oulipoly_runtime::provider_registry::ProviderRegistry> {
    agent_runtime_services.provider_registry_handle.current()
}

fn require_external_provider_artifact(
    registry: &oulipoly_runtime::provider_registry::ProviderRegistry,
    model_name: &str,
) -> Option<()> {
    registry.artifact_key_for_model(model_name)?;
    Some(())
}

fn describe_session_provider_model(
    registry: &oulipoly_runtime::provider_registry::ProviderRegistry,
    model_name: &str,
) -> Option<oulipoly_provider::generated::DescribeResult> {
    registry.describe_model_provider(model_name).ok()
}

fn external_provider_session_describe(
    describe: Option<&oulipoly_provider::generated::DescribeResult>,
) -> ExternalProviderSessionDescribe {
    ExternalProviderSessionDescribe {
        provider_instance_id: provider_instance_id_from_describe(describe),
        settings_id: session_settings_id(describe),
    }
}

fn session_settings_id(describe: Option<&oulipoly_provider::generated::DescribeResult>) -> String {
    settings_id_from_describe(describe)
}

fn provider_instance_id_from_describe(
    describe: Option<&oulipoly_provider::generated::DescribeResult>,
) -> Option<String> {
    describe.map(|result| format_provider_instance_id(&result.provider_id))
}

fn format_provider_instance_id(provider_id: &str) -> String {
    format!("{provider_id}-instance")
}

fn settings_id_from_describe(
    describe: Option<&oulipoly_provider::generated::DescribeResult>,
) -> String {
    describe
        .and_then(|result| result.settings_schema_id.clone())
        .unwrap_or_else(default_session_settings_id)
}

fn default_session_settings_id() -> String {
    oulipoly_runtime::session_provider::S7A_NEUTRAL_SETTINGS_ID.to_string()
}

fn session_service_external_provider_identity(
    model: &ModelConfig,
    provider_name: &str,
    describe: ExternalProviderSessionDescribe,
) -> SessionServiceExternalProviderIdentity {
    SessionServiceExternalProviderIdentity {
        model_name: model.name.clone(),
        provider_name: provider_name.to_string(),
        provider_instance_id: describe.provider_instance_id,
        settings_id: describe.settings_id,
    }
}

fn external_provider_model(model: Option<&ModelConfig>) -> Option<&ModelConfig> {
    model.filter(|model| model.provider.is_some())
}

fn session_lifecycle_ingest_mode(mode: ResumeIngestMode<'_>) -> SessionLifecycleIngestMode {
    match mode {
        ResumeIngestMode::Unpinned { capture_method } => SessionLifecycleIngestMode::Unpinned {
            capture_method: capture_method.to_string(),
        },
        ResumeIngestMode::Pinned { resume_target } => SessionLifecycleIngestMode::Pinned {
            resume_target: resume_target.to_string(),
        },
    }
}

fn format_session_ingest_failure(provider_name: &str, message: &str) -> String {
    format!("Warning: Session ingest failed for {provider_name}: {message}")
}

fn emit_session_ingest_failure(provider_name: &str, message: &str) {
    eprintln!("{}", format_session_ingest_failure(provider_name, message));
}

pub(crate) fn emit_known_session_id(
    state: &StateDb,
    invocation_row_id: i64,
    invocation_uuid: &str,
    session_id: &str,
    capture_method: &str,
) -> bool {
    if !emit_known_session_capture_update(state, invocation_row_id, session_id, capture_method) {
        return false;
    }
    emit_known_session_marker_for_capture(state, invocation_row_id, invocation_uuid, session_id);
    true
}

fn emit_known_session_marker_for_capture(
    state: &StateDb,
    invocation_row_id: i64,
    invocation_uuid: &str,
    session_id: &str,
) {
    let record = lookup_invocation_record(state, invocation_uuid);
    mint_known_session_chain_if_needed(state, invocation_row_id, record.as_ref());
    emit_known_session_marker(known_session_marker_payload(
        invocation_uuid,
        session_id,
        record.as_ref(),
        known_session_marker_chain_id(state, session_id, record.as_ref()),
    ));
}

fn emit_known_session_capture_update(
    state: &StateDb,
    invocation_row_id: i64,
    session_id: &str,
    capture_method: &str,
) -> bool {
    match update_known_session_capture(state, invocation_row_id, Some(session_id), capture_method) {
        Ok(()) => true,
        Err(err) => {
            emit_known_session_capture_warning(&err);
            false
        }
    }
}

fn emit_known_session_capture_warning(err: &str) {
    eprintln!("Warning: Failed to update invocation session_id: {err}");
}

fn lookup_invocation_record(state: &StateDb, invocation_uuid: &str) -> Option<InvocationRecord> {
    state.get_invocation_by_uuid(invocation_uuid).ok().flatten()
}

fn mint_known_session_chain_if_needed(
    state: &StateDb,
    invocation_row_id: i64,
    record: Option<&InvocationRecord>,
) {
    if should_mint_known_session_chain(record) {
        mint_known_session_chain(state, invocation_row_id)
            .unwrap_or_else(|err| emit_known_session_chain_warning(&err));
    }
}

fn mint_known_session_chain(state: &StateDb, invocation_row_id: i64) -> Result<(), String> {
    state.mint_chain_for_invocation_session(invocation_row_id)
}

fn emit_known_session_chain_warning(err: &str) {
    eprintln!("Warning: Failed to mint session chain: {err}");
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use oulipoly_config::provider_implementation_ref::ProviderImplementationRef;
    use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig};
    use oulipoly_runtime::provider_registry::{
        ProviderRegistry, ProviderRegistryHandle, ProviderRegistryOptions,
    };
    use oulipoly_runtime::services::ProductionSessionLifecycleService;
    use oulipoly_state::InvocationStart;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    const MODEL: &str = "provider-a-model";
    const PROVIDER: &str = "provider-a-account";
    const SESSION: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const INVOCATION: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

    #[test]
    fn production_lifecycle_ingest_routes_external_session_read_and_capture() {
        let fixture = ProductionSessionFixture::new();
        let row_id = fixture.seed_finalized_invocation();
        let model = external_model(&fixture.provider_path);
        let services = fixture.services(&model);
        let sessions_cfg = oulipoly_config::SessionsConfig::default();
        let providers_cfg = ProvidersConfig::default();

        let output = ingest_and_emit_session_id_resume_aware(
            &services,
            SessionIngestRequest {
                state: &fixture.state,
                sessions_cfg: &sessions_cfg,
                providers_cfg: Some(&providers_cfg),
                provider_name: PROVIDER,
                external_provider: session_external_provider_identity(
                    &services,
                    Some(&model),
                    PROVIDER,
                ),
                invocation_row_id: row_id,
                invocation_uuid: INVOCATION,
                effective_cwd: Some(fixture.dir.path()),
                mode: ResumeIngestMode::Pinned {
                    resume_target: SESSION,
                },
            },
        );

        assert!(output.emitted);
        assert_eq!(output.session_id.as_deref(), Some(SESSION));
        assert_eq!(
            fixture.provider_subcommands(),
            vec!["describe", "session.read_turns", "session.capture"]
        );
        assert_eq!(fixture.session_turn_count(), 1);
    }

    #[test]
    fn production_identity_is_none_for_builtin_models() {
        let fixture = ProductionSessionFixture::new();
        let services = fixture.services(&external_model(&fixture.provider_path));
        let builtin = ModelConfig {
            name: "provider-a-builtin-model".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider(PROVIDER, Vec::new())],
            inputs: Vec::new(),
            provider: None,
        };

        assert_eq!(
            session_external_provider_identity(&services, Some(&builtin), PROVIDER),
            None
        );
    }

    #[test]
    fn production_session_settings_id_uses_provider_described_schema_id() {
        let describe = describe_with_settings_schema("opencode.settings/v1");

        assert_eq!(session_settings_id(Some(&describe)), "opencode.settings/v1");
        assert_eq!(
            session_settings_id(None),
            oulipoly_runtime::session_provider::S7A_NEUTRAL_SETTINGS_ID
        );
    }

    struct ProductionSessionFixture {
        dir: tempfile::TempDir,
        state: StateDb,
        state_path: PathBuf,
        provider_path: PathBuf,
        record_path: PathBuf,
    }

    impl ProductionSessionFixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let state_path = dir.path().join("state.db");
            let state = StateDb::open(&state_path).expect("state db");
            let record_path = dir.path().join("provider-records.jsonl");
            let provider_path = write_provider_script(dir.path(), &record_path);
            Self {
                dir,
                state,
                state_path,
                provider_path,
                record_path,
            }
        }

        fn seed_finalized_invocation(&self) -> i64 {
            let row_id = self
                .state
                .start_invocation(&InvocationStart {
                    invocation_uuid: INVOCATION.to_string(),
                    model_name: MODEL.to_string(),
                    provider_name: PROVIDER.to_string(),
                    provider_index: 0,
                    parent_invocation_id: None,
                })
                .expect("start invocation");
            self.state
                .finalize_invocation(row_id, true, 0, None, Some("completed"))
                .expect("finalize invocation");
            row_id
        }

        fn services(&self, model: &ModelConfig) -> wiring::AgentRuntimeServices {
            let registry = Arc::new(
                ProviderRegistry::from_model_configs(
                    std::slice::from_ref(model),
                    ProviderRegistryOptions::default()
                        .with_config_root(self.dir.path().join("config-root"))
                        .with_data_root(self.dir.path().join("data-root")),
                )
                .expect("registry"),
            );
            let handle = ProviderRegistryHandle::new(registry.clone());
            let mut services = wiring::AgentRuntimeServices::cli_defaults();
            services.provider_registry = registry;
            services.provider_registry_handle = handle.clone();
            services.session_lifecycle_service = Arc::new(
                ProductionSessionLifecycleService::with_registry_handle(handle),
            );
            services
        }

        fn provider_subcommands(&self) -> Vec<String> {
            provider_subcommands_from_lines(&provider_record_lines(&provider_records_text(
                &self.record_path,
            )))
        }

        fn session_turn_count(&self) -> i64 {
            rusqlite::Connection::open(&self.state_path)
                .expect("sqlite")
                .query_row("SELECT COUNT(*) FROM session_turns", [], |row| row.get(0))
                .expect("turn count")
        }
    }

    fn provider_records_text(record_path: &Path) -> String {
        fs::read_to_string(record_path).unwrap_or_default()
    }

    fn provider_record_lines(records: &str) -> Vec<&str> {
        records
            .lines()
            .filter(|line| provider_record_line_is_non_empty(line))
            .collect()
    }

    fn provider_record_line_is_non_empty(line: &str) -> bool {
        !line.trim().is_empty()
    }

    fn provider_subcommands_from_lines(lines: &[&str]) -> Vec<String> {
        lines
            .iter()
            .map(|line| parse_provider_record(line))
            .map(|record| provider_record_subcommand(&record))
            .collect()
    }

    fn parse_provider_record(line: &str) -> serde_json::Value {
        serde_json::from_str(line).expect("json")
    }

    fn provider_record_subcommand(record: &serde_json::Value) -> String {
        record["subcommand"].as_str().unwrap().to_string()
    }

    fn external_model(provider_path: &Path) -> ModelConfig {
        ModelConfig {
            name: MODEL.to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider(PROVIDER, Vec::new())],
            inputs: Vec::new(),
            provider: Some(ProviderImplementationRef {
                path: Some(provider_path.display().to_string()),
                crate_name: None,
                version: None,
                binary: None,
                script: None,
            }),
        }
    }

    fn describe_with_settings_schema(
        schema_id: &str,
    ) -> oulipoly_provider::generated::DescribeResult {
        oulipoly_provider::generated::DescribeResult {
            provider_id: "opencode".to_string(),
            display_name: "OpenCode".to_string(),
            contract_versions: vec![oulipoly_provider::generated::CONTRACT_VERSION.to_string()],
            preferred_contract: oulipoly_provider::generated::CONTRACT_VERSION.to_string(),
            capabilities: oulipoly_provider::generated::DescribeCapabilities {
                launch: true,
                policy: true,
                quota: true,
                session: true,
                terminal: true,
                rotation: true,
                discovery: true,
                settings: true,
                setup_brain: false,
                setup: true,
                migration: true,
            },
            settings_schema_id: Some(schema_id.to_string()),
            concurrency: None,
        }
    }

    fn write_provider_script(dir: &Path, record_path: &Path) -> PathBuf {
        fs::write(record_path, "").expect("record init");
        let script = dir.join("provider-a-session.py");
        fs::write(&script, provider_script(record_path)).expect("script");
        let mut perms = fs::metadata(&script).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).expect("chmod");
        script
    }

    fn provider_script(record_path: &Path) -> String {
        format!(
            r#"#!/usr/bin/env python3
import json
import pathlib
import sys

CONTRACT = "oulipoly.provider/v1"
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
request = json.loads(sys.stdin.read() or "{{}}")
with pathlib.Path({record_path}).open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({{"subcommand": subcommand, "request": request}}, sort_keys=True) + "\n")

def envelope(result):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-age243"),
        "ok": True,
        "result": result,
    }}

if subcommand == "describe":
    response = envelope({{
        "provider_id": "provider-a",
        "display_name": "Provider A",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {{
            "launch": False,
            "policy": False,
            "quota": False,
            "session": True,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        }},
        "settings_schema_id": "provider-a-test-settings",
    }})
elif subcommand == "session.read_turns":
    response = envelope({{
        "turns": [{{
            "session_id": {session_id},
            "turn_id": "turn-1",
            "timestamp": "2026-05-01T00:00:00Z",
            "role": "assistant",
            "body": [{{"type": "text", "text": "ok"}}],
        }}],
        "turn_count": 1,
        "complete": True,
    }})
elif subcommand == "session.capture":
    response = envelope({{
        "provider_session_id": {session_id},
        "state": {{"captured": True}},
        "artifacts": [],
    }})
else:
    response = {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-age243"),
        "ok": False,
        "error": {{"category": "unsupported", "code": "unsupported_subcommand", "message": subcommand, "retryable": False}},
    }}
print(json.dumps(response))
"#,
            record_path = serde_json::to_string(&record_path.display().to_string()).unwrap(),
            session_id = serde_json::to_string(SESSION).unwrap(),
        )
    }
}

fn emit_known_session_marker(payload: oulipoly_state::SessionMarkerPayload) {
    eprint!("{}", payload.stderr_line());
}

fn update_known_session_capture(
    state: &StateDb,
    invocation_row_id: i64,
    session_id: Option<&str>,
    capture_method: &str,
) -> Result<(), String> {
    state.update_session_capture(invocation_row_id, session_id, capture_method)
}

fn should_mint_known_session_chain(record: Option<&InvocationRecord>) -> bool {
    record.is_none_or(|row| row.resume_input_id.as_deref() != row.provider_session_id.as_deref())
}

fn known_session_marker_chain_id(
    state: &StateDb,
    session_id: &str,
    record: Option<&InvocationRecord>,
) -> Option<String> {
    let fields = known_session_marker_fields(record, session_id);
    lookup_marker_chain_id(state, marker_chain_lookup_key(&fields))
}

struct MarkerChainLookupKey<'a> {
    provider_name: Option<&'a str>,
    provider_session_id: Option<&'a str>,
}

fn marker_chain_lookup_key(fields: &KnownSessionMarkerFields) -> MarkerChainLookupKey<'_> {
    MarkerChainLookupKey {
        provider_name: fields.provider_name.as_deref(),
        provider_session_id: fields.provider_session_id.as_deref(),
    }
}

fn known_session_marker_payload(
    invocation_uuid: &str,
    session_id: &str,
    record: Option<&InvocationRecord>,
    agent_runner_chain_id: Option<String>,
) -> oulipoly_state::SessionMarkerPayload {
    let fields = known_session_marker_fields(record, session_id);
    session_marker_payload_from_parts(marker_payload_parts(
        invocation_uuid,
        session_id,
        fields,
        agent_runner_chain_id,
    ))
}

struct KnownSessionMarkerFields {
    provider_name: Option<String>,
    provider_session_id: Option<String>,
    resume_input_id: Option<String>,
}

fn known_session_marker_fields(
    record: Option<&InvocationRecord>,
    session_id: &str,
) -> KnownSessionMarkerFields {
    let provider_name = record.and_then(|row| row.provider_name.clone());
    let provider_session_id = record
        .and_then(|row| row.provider_session_id.clone())
        .or_else(|| Some(session_id.to_string()));
    KnownSessionMarkerFields {
        provider_name,
        provider_session_id,
        resume_input_id: record.and_then(|row| row.resume_input_id.clone()),
    }
}

fn marker_payload_parts<'a>(
    invocation_uuid: &'a str,
    session_id: &'a str,
    fields: KnownSessionMarkerFields,
    agent_runner_chain_id: Option<String>,
) -> SessionMarkerPayloadParts<'a> {
    SessionMarkerPayloadParts {
        invocation_uuid,
        session_id,
        provider_name: fields.provider_name,
        provider_session_id: fields.provider_session_id,
        agent_runner_chain_id,
        resume_input_id: fields.resume_input_id,
    }
}

struct SessionMarkerPayloadParts<'a> {
    invocation_uuid: &'a str,
    session_id: &'a str,
    provider_name: Option<String>,
    provider_session_id: Option<String>,
    agent_runner_chain_id: Option<String>,
    resume_input_id: Option<String>,
}

fn lookup_marker_chain_id(state: &StateDb, key: MarkerChainLookupKey<'_>) -> Option<String> {
    key.provider_name.and_then(|provider_name| {
        key.provider_session_id.and_then(|provider_session_id| {
            state
                .chain_id_for_segment(provider_name, provider_session_id)
                .ok()
                .flatten()
        })
    })
}

fn session_marker_payload_from_parts(
    parts: SessionMarkerPayloadParts<'_>,
) -> oulipoly_state::SessionMarkerPayload {
    oulipoly_state::SessionMarkerPayload {
        agent_runner_invocation_id: parts.invocation_uuid.to_string(),
        provider_session_id: parts.provider_session_id,
        provider_name: parts.provider_name,
        agent_runner_chain_id: parts.agent_runner_chain_id,
        resume_input_id: parts.resume_input_id,
        legacy_id: parts.invocation_uuid.to_string(),
        legacy_session_id: Some(parts.session_id.to_string()),
    }
}
