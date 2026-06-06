use crate::provider_registry::{ProviderRegistry, ProviderRegistryError, ProviderRegistryOptions};
use oulipoly_config::ModelConfig;
use oulipoly_provider::client::ProviderEnv;
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::{
    CONTRACT_VERSION, DescribeResult, Diagnostic, EmptyParams, ErrorCategory, HostContext,
    ProcessStatus, RequestEnvelope, SchemaParams, SchemaResult, SettingsCreateParams,
    SettingsDeleteParams, SettingsDeleteResult, SettingsGetParams, SettingsGetResult,
    SettingsListResult, SettingsMigrateParams, SettingsMigrateResult, SettingsUpdateParams,
    SettingsValidateParams, SettingsValidateResult, SettingsValues, SettingsWriteResult,
};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug)]
pub struct ProviderSettingsHost {
    registry: ProviderRegistry,
    options: ProviderSettingsHostOptions,
    target_cache: Mutex<HashMap<String, ProviderSettingsTarget>>,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderSettingsHostOptions {
    registry: ProviderRegistryOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSettingsTarget {
    pub model_name: String,
    pub provider_id: String,
    pub display_name: String,
    pub settings_supported: bool,
    pub schema_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderSettingsError {
    category: Box<str>,
    code: Option<Box<str>>,
    message: Box<str>,
    retryable: Option<bool>,
    details: Box<Value>,
    diagnostics: Box<[Diagnostic]>,
    process_status: Option<Box<ProviderSettingsProcessStatus>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSettingsProcessStatus {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub kind: String,
}

impl ProviderSettingsHostOptions {
    pub fn with_path_entries_from_process_path(mut self) -> Self {
        self.registry = self.registry.with_path_entries_from_process_path();
        self
    }

    pub fn with_config_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.registry = self.registry.with_config_root(root);
        self
    }

    pub fn with_data_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.registry = self.registry.with_data_root(root);
        self
    }
}

impl ProviderSettingsHost {
    pub fn from_model_configs(
        models: &[ModelConfig],
        options: ProviderSettingsHostOptions,
    ) -> Result<Self, ProviderSettingsError> {
        let registry = ProviderRegistry::from_model_configs(models, options.registry.clone())?;
        Ok(Self {
            registry,
            options,
            target_cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn rebuild_from_model_configs(
        &mut self,
        models: &[ModelConfig],
    ) -> Result<(), ProviderSettingsError> {
        self.registry =
            ProviderRegistry::from_model_configs(models, self.options.registry.clone())?;
        self.target_cache
            .lock()
            .expect("settings target cache should not be poisoned")
            .clear();
        Ok(())
    }

    pub fn describe_settings_target(
        &self,
        model_name: &str,
    ) -> Result<ProviderSettingsTarget, ProviderSettingsError> {
        if let Some(target) = self
            .target_cache
            .lock()
            .expect("settings target cache should not be poisoned")
            .get(model_name)
            .cloned()
        {
            return Ok(target);
        }
        let description = self.describe_provider(model_name)?;
        let target = ProviderSettingsTarget {
            model_name: model_name.to_string(),
            provider_id: description.provider_id,
            display_name: description.display_name,
            settings_supported: description.capabilities.settings,
            schema_id: description.settings_schema_id,
        };
        self.target_cache
            .lock()
            .expect("settings target cache should not be poisoned")
            .insert(model_name.to_string(), target.clone());
        Ok(target)
    }

    pub fn configured_model_names(&self) -> Vec<String> {
        self.registry.configured_model_names()
    }

    fn describe_provider(&self, model_name: &str) -> Result<DescribeResult, ProviderSettingsError> {
        let artifact = self.registry.enabled_artifact_for_model(model_name)?;
        let client = self.registry.client_factory().client_for(artifact);
        let request = self.request(EmptyParams {})?;
        client
            .invoke_typed::<DescribeResult, _>("describe", request, NoProviderEnv)
            .map_err(ProviderSettingsError::from)
    }

    pub fn settings_schema(
        &self,
        model_name: &str,
        schema_id: &str,
    ) -> Result<SchemaResult, ProviderSettingsError> {
        self.ensure_settings_supported(model_name)?;
        self.call_provider(
            model_name,
            SettingsOperation::Schema,
            SchemaParams {
                schema_id: schema_id.to_string(),
            },
        )
    }

    pub fn settings_list(
        &self,
        model_name: &str,
    ) -> Result<SettingsListResult, ProviderSettingsError> {
        self.ensure_settings_supported(model_name)?;
        self.call_provider(model_name, SettingsOperation::List, EmptyParams {})
    }

    pub fn settings_get(
        &self,
        model_name: &str,
        id: &str,
    ) -> Result<SettingsGetResult, ProviderSettingsError> {
        self.ensure_settings_supported(model_name)?;
        self.call_provider(
            model_name,
            SettingsOperation::Get,
            SettingsGetParams { id: id.to_string() },
        )
    }

    pub fn settings_create(
        &self,
        model_name: &str,
        display_name: Option<String>,
        values: SettingsValues,
    ) -> Result<SettingsWriteResult, ProviderSettingsError> {
        self.ensure_settings_supported(model_name)?;
        self.call_provider(
            model_name,
            SettingsOperation::Create,
            SettingsCreateParams {
                display_name,
                values,
            },
        )
    }

    pub fn settings_update(
        &self,
        model_name: &str,
        id: &str,
        version: &str,
        values: SettingsValues,
    ) -> Result<SettingsWriteResult, ProviderSettingsError> {
        self.ensure_settings_supported(model_name)?;
        self.call_provider(
            model_name,
            SettingsOperation::Update,
            SettingsUpdateParams {
                id: id.to_string(),
                version: version.to_string(),
                values,
            },
        )
    }

    pub fn settings_delete(
        &self,
        model_name: &str,
        id: &str,
        version: &str,
    ) -> Result<SettingsDeleteResult, ProviderSettingsError> {
        self.ensure_settings_supported(model_name)?;
        self.call_provider(
            model_name,
            SettingsOperation::Delete,
            SettingsDeleteParams {
                id: id.to_string(),
                version: version.to_string(),
            },
        )
    }

    pub fn settings_validate(
        &self,
        model_name: &str,
        values: SettingsValues,
    ) -> Result<SettingsValidateResult, ProviderSettingsError> {
        self.ensure_settings_supported(model_name)?;
        self.call_provider(
            model_name,
            SettingsOperation::Validate,
            SettingsValidateParams { values },
        )
    }

    pub fn settings_migrate(
        &self,
        model_name: &str,
        dry_run: bool,
        legacy: Value,
    ) -> Result<SettingsMigrateResult, ProviderSettingsError> {
        self.ensure_settings_supported(model_name)?;
        self.call_provider(
            model_name,
            SettingsOperation::Migrate,
            SettingsMigrateParams { dry_run, legacy },
        )
    }

    fn ensure_settings_supported(&self, model_name: &str) -> Result<(), ProviderSettingsError> {
        let target = self.describe_settings_target(model_name)?;
        if target.settings_supported {
            Ok(())
        } else {
            Err(ProviderSettingsError::unsupported())
        }
    }

    fn call_provider<R, Params>(
        &self,
        model_name: &str,
        operation: SettingsOperation,
        params: Params,
    ) -> Result<R, ProviderSettingsError>
    where
        R: serde::de::DeserializeOwned,
        Params: Serialize,
    {
        let artifact = self.registry.enabled_artifact_for_model(model_name)?;
        let client = self.registry.client_factory().client_for(artifact);
        let request = self.request(params)?;
        client
            .invoke_typed::<R, _>(operation.as_provider_name(), request, NoProviderEnv)
            .map_err(ProviderSettingsError::from)
    }

    fn request<Params>(&self, params: Params) -> Result<Value, ProviderSettingsError>
    where
        Params: Serialize,
    {
        let mut value = serde_json::to_value(RequestEnvelope {
            contract: CONTRACT_VERSION.to_string(),
            request_id: format!("provider-settings-{}", uuid::Uuid::new_v4()),
            provider_instance_id: Some("provider-settings".to_string()),
            host: host_context(self.registry.host_options()),
            params,
        })
        .map_err(schema_request_error)?;
        if let Some(host) = value.get_mut("host").and_then(Value::as_object_mut) {
            host.entry("env".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy)]
enum SettingsOperation {
    Schema,
    List,
    Get,
    Create,
    Update,
    Delete,
    Validate,
    Migrate,
}

impl SettingsOperation {
    fn as_provider_name(self) -> &'static str {
        match self {
            Self::Schema => "schema",
            Self::List => "settings.list",
            Self::Get => "settings.get",
            Self::Create => "settings.create",
            Self::Update => "settings.update",
            Self::Delete => "settings.delete",
            Self::Validate => "settings.validate",
            Self::Migrate => "settings.migrate",
        }
    }
}

fn host_context(options: &crate::provider_registry::DescribeHostOptions) -> HostContext {
    HostContext {
        app: "oulipoly-agent-runner".to_string(),
        app_version: None,
        platform: None,
        working_directory: None,
        config_root: options
            .config_root
            .as_ref()
            .map(|path| path.display().to_string()),
        data_root: options
            .data_root
            .as_ref()
            .map(|path| path.display().to_string()),
        env: Default::default(),
        deadline_unix_ms: None,
    }
}

fn schema_request_error(error: serde_json::Error) -> ProviderSettingsError {
    ProviderSettingsError {
        category: "protocol".into(),
        code: Some("schema_invalid_request".into()),
        message: error.to_string().into_boxed_str(),
        retryable: Some(false),
        details: Box::new(Value::Object(Map::new())),
        diagnostics: Vec::new().into_boxed_slice(),
        process_status: None,
    }
}

impl ProviderSettingsError {
    fn unsupported() -> Self {
        Self {
            category: "unsupported".into(),
            code: Some("settings_unsupported".into()),
            message: "provider settings are not supported for this model".into(),
            retryable: Some(false),
            details: Box::new(Value::Object(Map::new())),
            diagnostics: Vec::new().into_boxed_slice(),
            process_status: None,
        }
    }

    pub fn category(&self) -> &str {
        &self.category
    }

    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn retryable(&self) -> Option<bool> {
        self.retryable
    }

    pub fn details(&self) -> &Value {
        self.details.as_ref()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn process_status(&self) -> Option<&ProviderSettingsProcessStatus> {
        self.process_status.as_deref()
    }
}

impl From<ProviderRegistryError> for ProviderSettingsError {
    fn from(error: ProviderRegistryError) -> Self {
        match error {
            ProviderRegistryError::ProviderTransport { kind, source }
            | ProviderRegistryError::ProviderProtocol { kind, source } => {
                ProviderSettingsError::from_client_kind(kind, *source)
            }
            ProviderRegistryError::ProviderDescribeFailed { source, .. } => (*source).into(),
            other => Self {
                category: "unavailable".into(),
                code: Some("settings_target_unavailable".into()),
                message: other.to_string().into_boxed_str(),
                retryable: Some(false),
                details: Box::new(Value::Object(Map::new())),
                diagnostics: Vec::new().into_boxed_slice(),
                process_status: None,
            },
        }
    }
}

impl From<ProviderClientError> for ProviderSettingsError {
    fn from(error: ProviderClientError) -> Self {
        match error {
            ProviderClientError::ProviderCapability(capability) => Self {
                category: error_category_text(&capability.error().category).into(),
                code: Some(capability.error().code.clone().into_boxed_str()),
                message: capability.error().message.clone().into_boxed_str(),
                retryable: Some(capability.error().retryable),
                details: Box::new(
                    capability
                        .error()
                        .details
                        .clone()
                        .and_then(|details| serde_json::to_value(details).ok())
                        .unwrap_or_else(|| Value::Object(Map::new())),
                ),
                diagnostics: capability.error().diagnostics.clone().into_boxed_slice(),
                process_status: provider_error_status(
                    capability
                        .provider_reported_process_status()
                        .or_else(|| capability.process_status()),
                )
                .map(Box::new),
            },
            other => {
                let kind = other.transport_kind().to_string();
                Self::from_client_kind(kind, other)
            }
        }
    }
}

impl ProviderSettingsError {
    fn from_client_kind(kind: String, error: ProviderClientError) -> Self {
        let message = client_error_message(&error);
        Self {
            category: transport_category(&error).into(),
            code: Some(kind.into_boxed_str()),
            message: message.into_boxed_str(),
            retryable: Some(false),
            details: Box::new(Value::Object(Map::new())),
            diagnostics: Vec::new().into_boxed_slice(),
            process_status: provider_error_status(error.process_status()).map(Box::new),
        }
    }
}

fn transport_category(error: &ProviderClientError) -> &'static str {
    match error {
        ProviderClientError::Transport { .. } => "transport",
        ProviderClientError::Protocol { .. } => "protocol",
        ProviderClientError::ProviderCapability(_) => "provider",
    }
}

fn client_error_message(error: &ProviderClientError) -> String {
    match error {
        ProviderClientError::Transport { description, .. }
        | ProviderClientError::Protocol { description, .. } => description.clone(),
        ProviderClientError::ProviderCapability(capability) => capability.error().message.clone(),
    }
}

fn error_category_text(category: &ErrorCategory) -> &'static str {
    match category {
        ErrorCategory::Unsupported => "unsupported",
        ErrorCategory::InvalidRequest => "invalid_request",
        ErrorCategory::InvalidSettings => "invalid_settings",
        ErrorCategory::Unavailable => "unavailable",
        ErrorCategory::Timeout => "timeout",
        ErrorCategory::Conflict => "conflict",
        ErrorCategory::Failed => "failed",
    }
}

fn provider_error_status(status: Option<&ProcessStatus>) -> Option<ProviderSettingsProcessStatus> {
    status.map(|status| match status {
        ProcessStatus::Exited { code } => ProviderSettingsProcessStatus {
            exit_code: Some(*code),
            signal: None,
            kind: "exited".to_string(),
        },
        ProcessStatus::SignalTerminated { signal } => ProviderSettingsProcessStatus {
            exit_code: None,
            signal: Some(*signal),
            kind: "signal_terminated".to_string(),
        },
        ProcessStatus::SpawnError { .. } => ProviderSettingsProcessStatus {
            exit_code: None,
            signal: None,
            kind: "spawn_error".to_string(),
        },
        ProcessStatus::ProlongedSilence { .. } => ProviderSettingsProcessStatus {
            exit_code: None,
            signal: None,
            kind: "prolonged_silence".to_string(),
        },
        ProcessStatus::Cancelled => ProviderSettingsProcessStatus {
            exit_code: None,
            signal: None,
            kind: "cancelled".to_string(),
        },
        ProcessStatus::Unknown => ProviderSettingsProcessStatus {
            exit_code: None,
            signal: None,
            kind: "unknown".to_string(),
        },
    })
}

struct NoProviderEnv;

impl ProviderEnv for NoProviderEnv {
    fn into_env_vec(self) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
        Vec::new()
    }
}
