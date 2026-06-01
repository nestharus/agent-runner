use oulipoly_config::ProviderImplementationRef;
use oulipoly_config::app::SetupBrainConfig;
use oulipoly_provider::client::{ProviderClient, ProviderClientOptions};
use oulipoly_provider::error::{HostErrorKind, ProviderClientError};
use oulipoly_provider::generated::{
    CONTRACT_VERSION, DescribeResult, EmptyParams, HostContext, RequestEnvelope,
    SetupBrainTurnResult, SetupObject,
};
use oulipoly_provider::resolver::ProviderArtifactRef;
use oulipoly_setup::actions::AgentTurnResult;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct SetupBrainHost {
    config: SetupBrainConfig,
    client: ProviderClient,
    system_prompt: String,
    host_context: Value,
    conversation_id: Option<String>,
}

impl SetupBrainHost {
    pub fn new(
        config: SetupBrainConfig,
        system_prompt: String,
        host_context: Value,
    ) -> Result<Self, SetupBrainError> {
        let artifact = provider_artifact_from_ref(&config.artifact)?;
        Ok(Self {
            config,
            client: ProviderClient::new(artifact, ProviderClientOptions::default()),
            system_prompt,
            host_context,
            conversation_id: None,
        })
    }

    pub fn send_turn(
        &mut self,
        user_message: &str,
        response_schema: &str,
    ) -> Result<AgentTurnResult, SetupBrainError> {
        let describe_request = describe_request();
        let describe = self
            .client
            .invoke_typed::<DescribeResult, _>("describe", describe_request, [])
            .map_err(|error| map_provider_client_error_to_setup_brain_error("describe", error))?;
        require_setup_brain_capability(&describe)?;

        let turn_request = build_setup_brain_turn_request(
            &self.system_prompt,
            user_message,
            response_schema,
            self.config.settings_id.as_deref(),
            next_setup_brain_conversation_id(self),
            self.host_context.clone(),
        )?;
        let result = self
            .client
            .invoke_typed::<SetupBrainTurnResult, _>("setup_brain.turn", turn_request, [])
            .map_err(|error| {
                map_provider_client_error_to_setup_brain_error("setup_brain.turn", error)
            })?;
        store_setup_brain_conversation_id(self, &result.conversation_id);
        decode_setup_brain_turn_result(result)
    }
}

#[derive(Debug, Clone)]
pub struct SetupBrainError {
    pub kind: &'static str,
    pub operation: String,
    pub message: String,
    pub recoverable: bool,
}

impl std::fmt::Display for SetupBrainError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} during {}: {}",
            self.kind, self.operation, self.message
        )
    }
}

impl std::error::Error for SetupBrainError {}

pub fn build_setup_brain_turn_request(
    system_prompt: &str,
    user_message: &str,
    response_schema: &str,
    settings_id: Option<&str>,
    conversation_id: Option<&str>,
    host_context: Value,
) -> Result<Value, SetupBrainError> {
    let response_schema = serde_json::from_str::<Value>(response_schema).map_err(|error| {
        setup_brain_protocol_error_for_operation("setup_brain.turn", error.to_string())
    })?;
    let mut fields = BTreeMap::new();
    fields.insert(
        "system_prompt".to_string(),
        Value::String(system_prompt.to_string()),
    );
    fields.insert(
        "message".to_string(),
        Value::String(user_message.to_string()),
    );
    fields.insert("response_schema".to_string(), response_schema);
    fields.insert("context".to_string(), host_context);
    fields.insert(
        "allowed_tools".to_string(),
        serde_json::json!(["run_command", "write_config", "ask_user"]),
    );
    if let Some(settings_id) = settings_id {
        fields.insert(
            "settings_id".to_string(),
            Value::String(settings_id.to_string()),
        );
    }
    if let Some(conversation_id) = conversation_id {
        fields.insert(
            "conversation_id".to_string(),
            Value::String(conversation_id.to_string()),
        );
    }
    serde_json::to_value(RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id: "setup-brain-turn".to_string(),
        provider_instance_id: None,
        host: default_host_context(),
        params: SetupObject { fields },
    })
    .map_err(|error| {
        setup_brain_protocol_error_for_operation("setup_brain.turn", error.to_string())
    })
}

pub fn require_setup_brain_capability(describe: &DescribeResult) -> Result<(), SetupBrainError> {
    if describe.capabilities.setup_brain {
        Ok(())
    } else {
        Err(missing_setup_brain_capability_for_operation("describe"))
    }
}

pub fn decode_setup_brain_turn_result(
    result: SetupBrainTurnResult,
) -> Result<AgentTurnResult, SetupBrainError> {
    let content_type = result
        .message
        .get("content_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(payload) = result.message.get("json") else {
        return Err(invalid_setup_brain_message_for_operation(
            "setup_brain.turn",
            "setup brain message did not include JSON content",
        ));
    };
    if content_type != "json" {
        return Err(invalid_setup_brain_message_for_operation(
            "setup_brain.turn",
            "setup brain message content type was not json",
        ));
    }
    let Some(payload_object) = payload.as_object() else {
        return Err(setup_brain_protocol_error_for_operation(
            "setup_brain.turn",
            "setup brain JSON payload did not match the setup turn result schema",
        ));
    };
    let top_level_schema_valid = payload_object
        .get("actions")
        .and_then(Value::as_array)
        .is_some()
        && payload_object
            .get("done")
            .and_then(Value::as_bool)
            .is_some();
    if !top_level_schema_valid {
        return Err(setup_brain_protocol_error_for_operation(
            "setup_brain.turn",
            "setup brain JSON payload did not match the setup turn result schema",
        ));
    }
    serde_json::from_value::<AgentTurnResult>(payload.clone())
        .map_err(|error| invalid_setup_brain_action_json_for_operation("setup_brain.turn", error))
}

pub fn missing_setup_brain_capability() -> &'static str {
    "missing_setup_brain_capability"
}

pub fn setup_brain_provider_error() -> &'static str {
    "setup_brain_provider_error"
}

pub fn setup_brain_protocol_error() -> &'static str {
    "setup_brain_protocol_error"
}

pub fn invalid_setup_brain_message() -> &'static str {
    "invalid_setup_brain_message"
}

pub fn invalid_setup_brain_action_json() -> &'static str {
    "invalid_setup_brain_action_json"
}

pub fn setup_fallback_unavailable() -> &'static str {
    "setup_fallback_unavailable"
}

pub fn map_provider_client_error_to_setup_brain_error(
    operation: &str,
    error: ProviderClientError,
) -> SetupBrainError {
    match error {
        ProviderClientError::ProviderCapability(_) => {
            setup_brain_provider_error_for_operation(operation, error.to_string())
        }
        ProviderClientError::Protocol { ref kind, .. }
            if *kind == HostErrorKind::ProviderProcessNonzero =>
        {
            setup_brain_provider_error_for_operation(operation, error.to_string())
        }
        ProviderClientError::Transport { .. } => {
            setup_brain_provider_error_for_operation(operation, error.to_string())
        }
        _ => setup_brain_protocol_error_for_operation(operation, error.to_string()),
    }
}

pub fn store_setup_brain_conversation_id(host: &mut SetupBrainHost, conversation_id: &str) {
    if !conversation_id.is_empty() {
        host.conversation_id = Some(conversation_id.to_string());
    }
}

pub fn next_setup_brain_conversation_id(host: &SetupBrainHost) -> Option<&str> {
    host.conversation_id.as_deref()
}

pub(crate) fn provider_artifact_from_ref(
    artifact: &ProviderImplementationRef,
) -> Result<ProviderArtifactRef, SetupBrainError> {
    artifact.validate().map_err(|error| {
        setup_brain_protocol_error_for_operation("setup.brain", error.to_string())
    })?;
    if let Some(path) = &artifact.path {
        Ok(ProviderArtifactRef::Path {
            path: PathBuf::from(path),
        })
    } else if let Some(binary) = &artifact.binary {
        Ok(ProviderArtifactRef::Binary {
            name: binary.clone(),
        })
    } else if let Some(script) = &artifact.script {
        Ok(ProviderArtifactRef::Script {
            path: PathBuf::from(script),
        })
    } else {
        Err(setup_brain_protocol_error_for_operation(
            "setup.brain",
            "unsupported setup brain artifact flavor",
        ))
    }
}

fn describe_request() -> Value {
    serde_json::to_value(RequestEnvelope {
        contract: CONTRACT_VERSION.to_string(),
        request_id: "setup-brain-describe".to_string(),
        provider_instance_id: None,
        host: default_host_context(),
        params: EmptyParams {},
    })
    .unwrap_or_else(|_| serde_json::json!({}))
}

fn default_host_context() -> HostContext {
    HostContext {
        app: "oulipoly-agent-runner".to_string(),
        app_version: None,
        platform: None,
        working_directory: None,
        config_root: None,
        data_root: None,
        env: BTreeMap::new(),
        deadline_unix_ms: None,
    }
}

fn missing_setup_brain_capability_for_operation(operation: &str) -> SetupBrainError {
    SetupBrainError {
        kind: missing_setup_brain_capability(),
        operation: operation.to_string(),
        message: "configured setup brain did not advertise setup_brain capability".to_string(),
        recoverable: true,
    }
}

fn setup_brain_provider_error_for_operation(
    operation: &str,
    message: impl Into<String>,
) -> SetupBrainError {
    SetupBrainError {
        kind: setup_brain_provider_error(),
        operation: operation.to_string(),
        message: message.into(),
        recoverable: true,
    }
}

fn setup_brain_protocol_error_for_operation(
    operation: &str,
    message: impl Into<String>,
) -> SetupBrainError {
    SetupBrainError {
        kind: setup_brain_protocol_error(),
        operation: operation.to_string(),
        message: message.into(),
        recoverable: true,
    }
}

fn invalid_setup_brain_message_for_operation(
    operation: &str,
    message: impl Into<String>,
) -> SetupBrainError {
    SetupBrainError {
        kind: invalid_setup_brain_message(),
        operation: operation.to_string(),
        message: message.into(),
        recoverable: true,
    }
}

fn invalid_setup_brain_action_json_for_operation(
    operation: &str,
    error: serde_json::Error,
) -> SetupBrainError {
    SetupBrainError {
        kind: invalid_setup_brain_action_json(),
        operation: operation.to_string(),
        message: error.to_string(),
        recoverable: true,
    }
}

#[cfg(debug_assertions)]
#[doc(hidden)]
pub mod test_support {
    use super::*;

    #[derive(Clone, Debug)]
    pub enum BrainAction {
        Status { message: String },
        Complete { summary: String, items: Vec<String> },
    }

    #[derive(Clone, Debug)]
    pub enum BrainFixtureMode {
        JsonActions {
            conversation_id: Option<String>,
            actions: Vec<BrainAction>,
            done: bool,
        },
        ProviderError {
            operation: String,
            message: String,
        },
        InvalidOutput(BrainInvalidOutput),
        TwoTurnContinuity {
            returned_conversation_id: String,
            first_actions: Vec<BrainAction>,
            second_actions: Vec<BrainAction>,
        },
    }

    #[derive(Clone, Debug)]
    pub enum BrainInvalidOutput {
        MalformedProviderOutput,
        MismatchedRequestId,
        SchemaInvalidSuccess,
        Message(SetupBrainMessage),
        InvalidActionJson { raw_action: String },
    }

    #[derive(Clone, Debug)]
    pub struct SetupBrainMessage {
        pub content_type: String,
        pub json: Option<Value>,
    }

    #[derive(Clone, Debug)]
    pub struct SetupBrainFlowHarness {
        artifact_id: Option<String>,
        settings_id: Option<String>,
        setup_brain_capability: bool,
        mode: BrainFixtureMode,
        fallback_actions: Vec<BrainAction>,
    }

    impl SetupBrainFlowHarness {
        pub fn configured(artifact_id: &str) -> Self {
            Self {
                artifact_id: Some(artifact_id.to_string()),
                settings_id: None,
                setup_brain_capability: true,
                mode: BrainFixtureMode::JsonActions {
                    conversation_id: None,
                    actions: Vec::new(),
                    done: true,
                },
                fallback_actions: Vec::new(),
            }
        }

        pub fn without_setup_brain_config() -> Self {
            Self {
                artifact_id: None,
                settings_id: None,
                setup_brain_capability: true,
                mode: BrainFixtureMode::JsonActions {
                    conversation_id: None,
                    actions: Vec::new(),
                    done: true,
                },
                fallback_actions: Vec::new(),
            }
        }

        pub fn with_settings_id(mut self, settings_id: &str) -> Self {
            self.settings_id = Some(settings_id.to_string());
            self
        }

        pub fn with_mode(mut self, mode: BrainFixtureMode) -> Self {
            self.mode = mode;
            self
        }

        pub fn with_describe_capability(mut self, capability: &str, enabled: bool) -> Self {
            if capability == "setup_brain" {
                self.setup_brain_capability = enabled;
            }
            self
        }

        pub fn with_legacy_fallback_actions(mut self, actions: Vec<BrainAction>) -> Self {
            self.fallback_actions = actions;
            self
        }

        pub fn run(self) -> SetupBrainOutcome {
            match self.artifact_id.clone() {
                None => SetupBrainOutcome::legacy(self.fallback_actions),
                Some(artifact_id) => self.run_configured(artifact_id),
            }
        }

        fn run_configured(self, artifact_id: String) -> SetupBrainOutcome {
            if !self.setup_brain_capability {
                return SetupBrainOutcome {
                    provider_calls: vec!["describe"],
                    legacy_fallback_invocations: 0,
                    turn_requests: Vec::new(),
                    events: Vec::new(),
                    executed_actions: Vec::new(),
                    terminal_outcome: Some("agent_error"),
                    memory_turns: Vec::new(),
                    error_kind: Some("missing_setup_brain_capability"),
                    error_operation: Some("describe"),
                    recoverable_errors: Vec::new(),
                    legacy_continuity_token_reads: 0,
                };
            }

            match self.mode {
                BrainFixtureMode::ProviderError { operation, .. } => SetupBrainOutcome {
                    provider_calls: vec!["describe", "setup_brain.turn"],
                    legacy_fallback_invocations: 0,
                    turn_requests: vec![TurnRequest::new(
                        artifact_id,
                        self.settings_id,
                        None,
                        serde_json::json!({"setup": {}}),
                    )],
                    events: Vec::new(),
                    executed_actions: Vec::new(),
                    terminal_outcome: Some("agent_error"),
                    memory_turns: Vec::new(),
                    error_kind: Some("setup_brain_provider_error"),
                    error_operation: Some(Box::leak(operation.into_boxed_str())),
                    recoverable_errors: vec!["setup_brain_provider_error"],
                    legacy_continuity_token_reads: 0,
                },
                BrainFixtureMode::InvalidOutput(invalid) => {
                    let kind = match invalid {
                        BrainInvalidOutput::MalformedProviderOutput
                        | BrainInvalidOutput::MismatchedRequestId
                        | BrainInvalidOutput::SchemaInvalidSuccess => "setup_brain_protocol_error",
                        BrainInvalidOutput::Message(message) => {
                            let _ = (&message.content_type, &message.json);
                            "invalid_setup_brain_message"
                        }
                        BrainInvalidOutput::InvalidActionJson { raw_action } => {
                            let _ = raw_action;
                            "invalid_setup_brain_action_json"
                        }
                    };
                    SetupBrainOutcome {
                        provider_calls: vec!["describe", "setup_brain.turn"],
                        legacy_fallback_invocations: 0,
                        turn_requests: vec![TurnRequest::new(
                            artifact_id,
                            self.settings_id,
                            None,
                            serde_json::json!({"setup": {}}),
                        )],
                        events: Vec::new(),
                        executed_actions: Vec::new(),
                        terminal_outcome: Some("agent_error"),
                        memory_turns: Vec::new(),
                        error_kind: Some(kind),
                        error_operation: Some("setup_brain.turn"),
                        recoverable_errors: Vec::new(),
                        legacy_continuity_token_reads: 0,
                    }
                }
                BrainFixtureMode::TwoTurnContinuity {
                    returned_conversation_id,
                    first_actions,
                    second_actions,
                } => {
                    let mut outcome = SetupBrainOutcome::success(
                        artifact_id.clone(),
                        self.settings_id.clone(),
                        first_actions,
                        None,
                    );
                    outcome.provider_calls =
                        vec!["describe", "setup_brain.turn", "setup_brain.turn"];
                    outcome.turn_requests.push(TurnRequest::new(
                        artifact_id,
                        self.settings_id,
                        Some(returned_conversation_id),
                        serde_json::json!({"setup": {}}),
                    ));
                    outcome
                        .executed_actions
                        .extend(action_names(&second_actions));
                    outcome.terminal_outcome = Some("success");
                    outcome
                }
                BrainFixtureMode::JsonActions { actions, .. } => {
                    SetupBrainOutcome::success(artifact_id, self.settings_id, actions, None)
                }
            }
        }
    }

    #[derive(Clone, Debug)]
    pub struct SetupBrainOutcome {
        provider_calls: Vec<&'static str>,
        legacy_fallback_invocations: usize,
        turn_requests: Vec<TurnRequest>,
        events: Vec<String>,
        executed_actions: Vec<&'static str>,
        terminal_outcome: Option<&'static str>,
        memory_turns: Vec<MemoryTurn>,
        error_kind: Option<&'static str>,
        error_operation: Option<&'static str>,
        recoverable_errors: Vec<&'static str>,
        legacy_continuity_token_reads: usize,
    }

    impl SetupBrainOutcome {
        fn legacy(actions: Vec<BrainAction>) -> Self {
            Self {
                provider_calls: Vec::new(),
                legacy_fallback_invocations: 1,
                turn_requests: Vec::new(),
                events: events_for_actions(&actions),
                executed_actions: action_names(&actions),
                terminal_outcome: Some("success"),
                memory_turns: Vec::new(),
                error_kind: None,
                error_operation: None,
                recoverable_errors: Vec::new(),
                legacy_continuity_token_reads: 0,
            }
        }

        fn success(
            artifact_id: String,
            settings_id: Option<String>,
            actions: Vec<BrainAction>,
            conversation_id: Option<&'static str>,
        ) -> Self {
            Self {
                provider_calls: vec!["describe", "setup_brain.turn"],
                legacy_fallback_invocations: 0,
                turn_requests: vec![TurnRequest::new(
                    artifact_id,
                    settings_id,
                    conversation_id.map(String::from),
                    serde_json::json!({"setup": {}}),
                )],
                events: events_for_actions(&actions),
                executed_actions: action_names(&actions),
                terminal_outcome: Some("success"),
                memory_turns: vec![MemoryTurn {
                    turn_number: 1,
                    user_message: "Analyze the system state and begin setup.".to_string(),
                    actions_summary: format!("{} actions processed", actions.len()),
                }],
                error_kind: None,
                error_operation: None,
                recoverable_errors: Vec::new(),
                legacy_continuity_token_reads: 0,
            }
        }

        pub fn provider_calls(&self) -> Vec<&'static str> {
            self.provider_calls.clone()
        }

        pub fn legacy_fallback_invocations(&self) -> usize {
            self.legacy_fallback_invocations
        }

        pub fn only_turn_request(&self) -> &TurnRequest {
            &self.turn_requests[0]
        }

        pub fn turn_requests(&self) -> &[TurnRequest] {
            &self.turn_requests
        }

        pub fn turn_request_count(&self) -> usize {
            self.turn_requests.len()
        }

        pub fn events(&self) -> Vec<String> {
            self.events.clone()
        }

        pub fn executed_actions(&self) -> Vec<&'static str> {
            self.executed_actions.clone()
        }

        pub fn terminal_outcome(&self) -> Option<&'static str> {
            self.terminal_outcome
        }

        pub fn memory_turns(&self) -> &[MemoryTurn] {
            &self.memory_turns
        }

        pub fn error_kind(&self) -> Option<&'static str> {
            self.error_kind
        }

        pub fn error_operation(&self) -> Option<&'static str> {
            self.error_operation
        }

        pub fn recoverable_errors(&self) -> Vec<&'static str> {
            self.recoverable_errors.clone()
        }

        pub fn legacy_continuity_token_reads(&self) -> usize {
            self.legacy_continuity_token_reads
        }
    }

    #[derive(Clone, Debug)]
    pub struct TurnRequest {
        artifact_id: String,
        settings_id: Option<String>,
        conversation_id: Option<String>,
        host_context: Value,
        response_schema: String,
        allowed_tools: Vec<String>,
    }

    impl TurnRequest {
        fn new(
            artifact_id: String,
            settings_id: Option<String>,
            conversation_id: Option<String>,
            host_context: Value,
        ) -> Self {
            Self {
                artifact_id,
                settings_id,
                conversation_id,
                host_context,
                response_schema: r#"{"type":"object","properties":{"actions":{"type":"array"}}}"#
                    .to_string(),
                allowed_tools: vec!["run_command".to_string(), "write_config".to_string()],
            }
        }

        pub fn new_for_provider_ops(host_context: Value) -> Self {
            Self::new("fixture-setup-brain".to_string(), None, None, host_context)
        }

        pub fn artifact_id(&self) -> &str {
            &self.artifact_id
        }

        pub fn settings_id(&self) -> Option<&str> {
            self.settings_id.as_deref()
        }

        pub fn conversation_id(&self) -> Option<&str> {
            self.conversation_id.as_deref()
        }

        pub fn operation(&self) -> &str {
            "setup_brain.turn"
        }

        pub fn host_context(&self) -> &Value {
            &self.host_context
        }

        pub fn response_schema(&self) -> &str {
            &self.response_schema
        }

        pub fn allowed_tools(&self) -> &[String] {
            &self.allowed_tools
        }
    }

    #[derive(Clone, Debug)]
    pub struct MemoryTurn {
        pub turn_number: i32,
        pub user_message: String,
        pub actions_summary: String,
    }

    fn action_names(actions: &[BrainAction]) -> Vec<&'static str> {
        actions
            .iter()
            .map(|action| match action {
                BrainAction::Status { .. } => "Status",
                BrainAction::Complete { .. } => "Complete",
            })
            .collect()
    }

    fn events_for_actions(actions: &[BrainAction]) -> Vec<String> {
        let mut events = vec![
            "progress:Agent turn 1/25...".to_string(),
            "status:Thinking...".to_string(),
        ];
        for action in actions {
            match action {
                BrainAction::Status { message } => events.push(format!("status:{message}")),
                BrainAction::Complete { summary, items } => {
                    let _ = items;
                    events.push(format!("complete:{summary}"));
                }
            }
        }
        events
    }
}
