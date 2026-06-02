use crate::{AppState, load_providers_for_models_dir_with};
use oulipoly_provider::generated::{
    Diagnostic, SchemaResult, SettingsDeleteResult, SettingsGetResult, SettingsListResult,
    SettingsMigrateResult, SettingsRecord, SettingsRecordSummary, SettingsValidateResult,
    SettingsValues, SettingsWriteResult,
};
use oulipoly_runtime::provider_settings::{
    ProviderSettingsError, ProviderSettingsHost, ProviderSettingsHostOptions,
    ProviderSettingsProcessStatus, ProviderSettingsTarget as RuntimeProviderSettingsTarget,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProviderSettingsSchemaArgs {
    pub model_name: String,
    pub schema_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProviderSettingsArgs {
    pub model_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProviderSettingsArgs {
    pub model_name: String,
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProviderSettingsArgs {
    pub model_name: String,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    pub values: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProviderSettingsArgs {
    pub model_name: String,
    pub id: String,
    pub version: String,
    pub values: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteProviderSettingsArgs {
    pub model_name: String,
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidateProviderSettingsArgs {
    pub model_name: String,
    pub values: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrateProviderSettingsArgs {
    pub model_name: String,
    pub dry_run: bool,
    pub legacy: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsTarget {
    pub model_name: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub provider_id: String,
    pub settings_supported: bool,
    pub schema_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsSchema {
    #[serde(rename = "schemaId")]
    pub schema_id: String,
    pub schema: Value,
    pub ui: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsRecordSummary {
    pub id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub version: String,
    pub summary: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsRecord {
    pub id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub version: String,
    pub values: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsListDto {
    pub records: Vec<ProviderSettingsRecordSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsGetDto {
    pub record: ProviderSettingsRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsWriteDto {
    pub record: ProviderSettingsRecord,
    pub diagnostics: Vec<ProviderDiagnosticDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsDeleteDto {
    pub deleted: bool,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsValidateDto {
    pub valid: bool,
    pub diagnostics: Vec<ProviderDiagnosticDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsMigrateDto {
    pub actions: Vec<Value>,
    pub warnings: Vec<String>,
    pub requires_user_input: bool,
    #[serde(default)]
    pub diagnostics: Vec<ProviderDiagnosticDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDiagnosticDto {
    pub severity: String,
    pub message: String,
    pub path: Option<String>,
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProcessStatusDto {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettingsErrorDto {
    pub category: String,
    pub code: Option<String>,
    pub message: String,
    pub retryable: Option<bool>,
    pub details: Option<Value>,
    pub diagnostics: Vec<ProviderDiagnosticDto>,
    #[serde(rename = "processStatus")]
    pub process_status: Option<ProviderProcessStatusDto>,
}

pub type ProviderSettingsCommandResult<T> = Result<T, Box<ProviderSettingsErrorDto>>;

#[tauri::command]
pub fn list_provider_settings_targets(
    state: tauri::State<AppState>,
) -> ProviderSettingsCommandResult<Vec<ProviderSettingsTarget>> {
    list_provider_settings_targets_inner(&state)
}

#[tauri::command]
pub fn get_provider_settings_schema(
    state: tauri::State<AppState>,
    args: GetProviderSettingsSchemaArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsSchema> {
    get_provider_settings_schema_inner(&state, args)
}

#[tauri::command]
pub fn list_provider_settings(
    state: tauri::State<AppState>,
    args: ModelProviderSettingsArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsListDto> {
    list_provider_settings_inner(&state, args)
}

#[tauri::command]
pub fn get_provider_settings(
    state: tauri::State<AppState>,
    args: GetProviderSettingsArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsGetDto> {
    get_provider_settings_inner(&state, args)
}

#[tauri::command]
pub fn create_provider_settings(
    state: tauri::State<AppState>,
    args: CreateProviderSettingsArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsWriteDto> {
    create_provider_settings_inner(&state, args)
}

#[tauri::command]
pub fn update_provider_settings(
    state: tauri::State<AppState>,
    args: UpdateProviderSettingsArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsWriteDto> {
    update_provider_settings_inner(&state, args)
}

#[tauri::command]
pub fn delete_provider_settings(
    state: tauri::State<AppState>,
    args: DeleteProviderSettingsArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsDeleteDto> {
    delete_provider_settings_inner(&state, args)
}

#[tauri::command]
pub fn validate_provider_settings(
    state: tauri::State<AppState>,
    args: ValidateProviderSettingsArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsValidateDto> {
    validate_provider_settings_inner(&state, args)
}

#[tauri::command]
pub fn migrate_provider_settings(
    state: tauri::State<AppState>,
    args: MigrateProviderSettingsArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsMigrateDto> {
    migrate_provider_settings_inner(&state, args)
}

pub fn list_provider_settings_targets_inner(
    state: &AppState,
) -> ProviderSettingsCommandResult<Vec<ProviderSettingsTarget>> {
    let names = with_host(state, |host| Ok(host.configured_model_names()))?;
    names
        .into_iter()
        .map(|name| with_host(state, |host| host.describe_settings_target(&name)).map(map_target))
        .collect()
}

pub fn get_provider_settings_schema_inner(
    state: &AppState,
    args: GetProviderSettingsSchemaArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsSchema> {
    with_host(state, |host| {
        host.settings_schema(&args.model_name, &args.schema_id)
    })
    .map(map_schema)
}

pub fn list_provider_settings_inner(
    state: &AppState,
    args: ModelProviderSettingsArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsListDto> {
    with_host(state, |host| host.settings_list(&args.model_name)).map(map_list)
}

pub fn get_provider_settings_inner(
    state: &AppState,
    args: GetProviderSettingsArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsGetDto> {
    with_host(state, |host| host.settings_get(&args.model_name, &args.id)).map(map_get)
}

pub fn create_provider_settings_inner(
    state: &AppState,
    args: CreateProviderSettingsArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsWriteDto> {
    let values = settings_values(args.values)?;
    with_host(state, |host| {
        host.settings_create(&args.model_name, args.display_name.clone(), values)
    })
    .map(map_write)
}

pub fn update_provider_settings_inner(
    state: &AppState,
    args: UpdateProviderSettingsArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsWriteDto> {
    if let Some(result) = test_update_result(state) {
        return result;
    }
    let values = settings_values(args.values)?;
    with_host(state, |host| {
        host.settings_update(&args.model_name, &args.id, &args.version, values)
    })
    .map(map_write)
}

pub fn delete_provider_settings_inner(
    state: &AppState,
    args: DeleteProviderSettingsArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsDeleteDto> {
    with_host(state, |host| {
        host.settings_delete(&args.model_name, &args.id, &args.version)
    })
    .map(map_delete)
}

pub fn validate_provider_settings_inner(
    state: &AppState,
    args: ValidateProviderSettingsArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsValidateDto> {
    if let Some(result) = test_validate_result(state) {
        return result;
    }
    let values = settings_values(args.values)?;
    with_host(state, |host| {
        host.settings_validate(&args.model_name, values)
    })
    .map(map_validate)
}

pub fn migrate_provider_settings_inner(
    state: &AppState,
    args: MigrateProviderSettingsArgs,
) -> ProviderSettingsCommandResult<ProviderSettingsMigrateDto> {
    let legacy = if args.legacy.is_null() {
        package_migration_legacy_payload(state, &args.model_name)?
    } else {
        args.legacy.clone()
    };
    with_host(state, |host| {
        host.settings_migrate(&args.model_name, args.dry_run, legacy)
    })
    .map(map_migrate)
}

pub fn refresh_provider_settings_host(state: &AppState) -> Result<(), String> {
    let models = state.models.lock().map_err(|error| error.to_string())?;
    let fresh = build_host(&state.models_dir, &models)?;
    let mut host = state
        .provider_settings
        .lock()
        .map_err(|error| error.to_string())?;
    *host = fresh;
    Ok(())
}

pub fn build_host(
    models_dir: &Path,
    models: &HashMap<String, oulipoly_config::ModelConfig>,
) -> Result<ProviderSettingsHost, String> {
    let options = host_options(models_dir);
    let model_values = models.values().cloned().collect::<Vec<_>>();
    ProviderSettingsHost::from_model_configs(&model_values, options)
        .map_err(|error| error.message().to_string())
}

pub fn host_options(models_dir: &Path) -> ProviderSettingsHostOptions {
    let config_root = models_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    ProviderSettingsHostOptions::default()
        .with_config_root(config_root.clone())
        .with_data_root(config_root)
}

pub fn package_migration_legacy_payload(
    state: &AppState,
    model_name: &str,
) -> ProviderSettingsCommandResult<Value> {
    let mut root = Map::new();
    root.insert(
        "models".to_string(),
        packaged_model_block(state, model_name)?,
    );
    root.insert("providers".to_string(), packaged_provider_blocks(state)?);
    Ok(Value::Object(root))
}

fn packaged_model_block(
    state: &AppState,
    model_name: &str,
) -> ProviderSettingsCommandResult<Value> {
    let path = state.models_dir.join(format!("{model_name}.toml"));
    let value = read_toml_json(&path)?;
    let mut models = Map::new();
    models.insert(model_name.to_string(), value);
    Ok(Value::Object(models))
}

fn packaged_provider_blocks(state: &AppState) -> ProviderSettingsCommandResult<Value> {
    let _providers =
        load_providers_for_models_dir_with(&state.models_dir, &*state.providers_config);
    let path = state
        .models_dir
        .parent()
        .unwrap_or(&state.models_dir)
        .join("providers.toml");
    let raw = if path.exists() {
        read_toml_json(&path)?
    } else {
        Value::Object(Map::new())
    };
    Ok(raw)
}

fn read_toml_json(path: &Path) -> ProviderSettingsCommandResult<Value> {
    let text = std::fs::read_to_string(path).map_err(|error| simple_error("unavailable", error))?;
    let value = toml::from_str::<toml::Value>(&text)
        .map_err(|error| simple_error("invalid_request", error))?;
    serde_json::to_value(value).map_err(|error| simple_error("failed", error))
}

fn with_host<T>(
    state: &AppState,
    action: impl FnOnce(&ProviderSettingsHost) -> Result<T, ProviderSettingsError>,
) -> ProviderSettingsCommandResult<T> {
    let host = state.provider_settings.lock().map_err(|error| {
        Box::new(ProviderSettingsErrorDto {
            category: "failed".to_string(),
            code: Some("settings_host_lock".to_string()),
            message: error.to_string(),
            retryable: Some(false),
            details: None,
            diagnostics: Vec::new(),
            process_status: None,
        })
    })?;
    action(&host).map_err(map_error)
}

fn settings_values(value: Value) -> ProviderSettingsCommandResult<SettingsValues> {
    serde_json::from_value(value).map_err(|error| simple_error("invalid_request", error))
}

fn map_target(target: RuntimeProviderSettingsTarget) -> ProviderSettingsTarget {
    ProviderSettingsTarget {
        model_name: target.model_name,
        display_name: target.display_name,
        provider_id: target.provider_id,
        settings_supported: target.settings_supported,
        schema_id: target.schema_id,
    }
}

fn map_schema(result: SchemaResult) -> ProviderSettingsSchema {
    ProviderSettingsSchema {
        schema_id: result.schema_id,
        schema: result.schema,
        ui: result.ui,
    }
}

fn map_list(result: SettingsListResult) -> ProviderSettingsListDto {
    ProviderSettingsListDto {
        records: result.records.into_iter().map(map_summary).collect(),
    }
}

fn map_get(result: SettingsGetResult) -> ProviderSettingsGetDto {
    ProviderSettingsGetDto {
        record: map_record(result.record),
    }
}

fn map_write(result: SettingsWriteResult) -> ProviderSettingsWriteDto {
    ProviderSettingsWriteDto {
        record: map_record(result.record),
        diagnostics: result.diagnostics.into_iter().map(map_diagnostic).collect(),
    }
}

fn map_delete(result: SettingsDeleteResult) -> ProviderSettingsDeleteDto {
    ProviderSettingsDeleteDto {
        deleted: result.deleted,
        id: result.id,
    }
}

fn map_validate(result: SettingsValidateResult) -> ProviderSettingsValidateDto {
    ProviderSettingsValidateDto {
        valid: result.valid,
        diagnostics: result.diagnostics.into_iter().map(map_diagnostic).collect(),
    }
}

fn map_migrate(result: SettingsMigrateResult) -> ProviderSettingsMigrateDto {
    ProviderSettingsMigrateDto {
        actions: result.actions,
        warnings: result.warnings,
        requires_user_input: result.requires_user_input,
        diagnostics: result.diagnostics.into_iter().map(map_diagnostic).collect(),
    }
}

fn map_summary(summary: SettingsRecordSummary) -> ProviderSettingsRecordSummary {
    ProviderSettingsRecordSummary {
        id: summary.id,
        display_name: summary.display_name,
        version: summary.version,
        summary: summary
            .summary
            .and_then(|value| serde_json::to_value(value).ok()),
    }
}

fn map_record(record: SettingsRecord) -> ProviderSettingsRecord {
    ProviderSettingsRecord {
        id: record.id,
        display_name: record.display_name,
        version: record.version,
        values: serde_json::to_value(record.values).unwrap_or_else(|_| Value::Object(Map::new())),
    }
}

fn map_diagnostic(diagnostic: Diagnostic) -> ProviderDiagnosticDto {
    ProviderDiagnosticDto {
        severity: diagnostic.severity,
        message: diagnostic.message,
        path: diagnostic.path,
        code: diagnostic.code,
    }
}

fn map_error(error: ProviderSettingsError) -> Box<ProviderSettingsErrorDto> {
    Box::new(ProviderSettingsErrorDto {
        category: error.category().to_string(),
        code: error.code().map(str::to_string),
        message: error.message().to_string(),
        retryable: error.retryable(),
        details: Some(error.details().clone()),
        diagnostics: error
            .diagnostics()
            .iter()
            .cloned()
            .map(map_diagnostic)
            .collect(),
        process_status: error.process_status().map(map_status),
    })
}

fn map_status(status: &ProviderSettingsProcessStatus) -> ProviderProcessStatusDto {
    ProviderProcessStatusDto {
        exit_code: status.exit_code,
        signal: status.signal,
        kind: status.kind.clone(),
    }
}

fn simple_error(category: &str, error: impl std::fmt::Display) -> Box<ProviderSettingsErrorDto> {
    Box::new(ProviderSettingsErrorDto {
        category: category.to_string(),
        code: None,
        message: error.to_string(),
        retryable: Some(false),
        details: None,
        diagnostics: Vec::new(),
        process_status: None,
    })
}

#[derive(Clone, Default)]
pub struct ProviderSettingsCommandResponses {
    update_error: Option<ProviderSettingsErrorDto>,
    validate_error: Option<ProviderSettingsErrorDto>,
}

pub struct ProviderSettingsCommandHarness {
    state: AppState,
}

impl Default for ProviderSettingsCommandHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderSettingsCommandHarness {
    pub fn new() -> Self {
        Self::with_state(AppState::test_default(
            Default::default(),
            Default::default(),
        ))
    }

    pub fn with_state(state: AppState) -> Self {
        Self { state }
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn fail_update_with_provider_error(&mut self, error: ProviderSettingsErrorDto) {
        self.with_responses(|responses| responses.update_error = Some(error));
    }

    pub fn fail_validate_with_transport_error(&mut self, category: &str, message: &str) {
        let error = ProviderSettingsErrorDto {
            category: category.to_string(),
            code: None,
            message: message.to_string(),
            retryable: Some(false),
            details: None,
            diagnostics: Vec::new(),
            process_status: None,
        };
        self.with_responses(|responses| responses.validate_error = Some(error));
    }

    fn with_responses(&self, action: impl FnOnce(&mut ProviderSettingsCommandResponses)) {
        if let Ok(mut guard) = self.state.provider_settings_test_double.lock() {
            let responses = guard.get_or_insert_with(ProviderSettingsCommandResponses::default);
            action(responses);
        }
    }
}

fn test_update_result(
    state: &AppState,
) -> Option<ProviderSettingsCommandResult<ProviderSettingsWriteDto>> {
    state
        .provider_settings_test_double
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .and_then(|responses| responses.update_error.clone())
        })
        .map(|error| Err(Box::new(error)))
}

fn test_validate_result(
    state: &AppState,
) -> Option<ProviderSettingsCommandResult<ProviderSettingsValidateDto>> {
    state
        .provider_settings_test_double
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .and_then(|responses| responses.validate_error.clone())
        })
        .map(|error| Err(Box::new(error)))
}
