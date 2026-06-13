//! ## Declared roles
//!
//! - accessor
//! - filter
//! - formatter
//! - mapper
//! - orchestration
//! - parser
//! - predicate
//! - validator
//!
//! Role set: { accessor, filter, formatter, mapper, orchestration, parser, predicate, validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/providers/loader.rs
//!     role: intrinsic-surface
//!     Domain: providers-loading-and-migration-pipeline
//!     Owns:
//!       - providers TOML parse, defaulting, validation, construction, and migration orchestration
//!       - RawEntry to ProviderEntry conversion subordinate to validated provider loading
//!       - legacy session_storage file rewrite and shell word formatting for provider migrations
//! ```
//!
use crate::claude_tool_filter::{ClaudeToolFilterShape, validate_proxy_claude_filter_shape};
use crate::model::{InvocationMode, PromptMode, ToolRestrictionKind};
use std::fs;
use std::path::Path;

use super::entry::{ProviderEntry, provider_family, shell_split};
use super::error::LoadError;
use super::raw_entry::{RawEntry, RawProvidersToml};

struct LegacyStorageMapping {
    legacy_dir_key: &'static str,
    cwd_command: &'static str,
    transcript_command: &'static str,
    storage_type: &'static str,
}

struct ScriptStorageDescriptor<'a> {
    cwd_command: &'static str,
    transcript_command: &'static str,
    storage_dir: &'a str,
    storage_type: &'static str,
}

#[derive(Clone)]
struct ScriptStorageFields {
    cwd_script: String,
    transcript_script: String,
    storage_type: String,
}

struct LegacySessionStorageRecord {
    provider_name: String,
    fields: ScriptStorageFields,
}

#[allow(dead_code)]
pub(super) fn parse_providers_toml(text: &str) -> Result<RawProvidersToml, LoadError> {
    toml::from_str(text).map_err(|err| LoadError::Toml(err.to_string()))
}

#[allow(dead_code)]
pub(super) fn apply_defaults_to_raw_providers(mut raw: RawProvidersToml) -> RawProvidersToml {
    for entry in raw.values_mut() {
        entry
            .invocation_mode
            .get_or_insert_with(|| "headless".to_string());
    }
    raw
}

#[allow(dead_code)]
pub(super) fn validate_providers_config(raw: &RawProvidersToml) -> Result<(), LoadError> {
    for (name, raw_entry) in raw {
        let invocation_mode = validate_provider_invocation_mode_values(name, raw_entry)?;
        validate_provider_argv_shapes(name, raw_entry, invocation_mode)?;
        let entry = raw_entry_to_entry(raw_entry, invocation_mode);
        validate_provider_entry_shape(name, &entry)?;
    }
    Ok(())
}

fn validate_provider_invocation_mode_values(
    name: &str,
    raw: &RawEntry,
) -> Result<InvocationMode, LoadError> {
    validate_invocation_mode(name, raw.invocation_mode.as_deref())
}

pub(super) fn validate_invocation_mode(
    name: &str,
    raw_mode: Option<&str>,
) -> Result<InvocationMode, LoadError> {
    let value = raw_mode.unwrap_or("headless");
    match parse_invocation_mode(value) {
        Some(mode) => Ok(mode),
        None => Err(invocation_mode_load_error(name, value)),
    }
}

pub(super) fn parse_invocation_mode(value: &str) -> Option<InvocationMode> {
    match value {
        "headless" => Some(InvocationMode::Headless),
        "proxy" => Some(InvocationMode::Proxy),
        _ => None,
    }
}

fn invocation_mode_load_error(name: &str, value: &str) -> LoadError {
    LoadError::InvocationMode {
        provider: name.to_string(),
        value: value.to_string(),
    }
}

pub(super) fn raw_entry_to_entry(raw: &RawEntry, invocation_mode: InvocationMode) -> ProviderEntry {
    ProviderEntry {
        quota_script: raw.quota_script.clone(),
        auth_refresh_command: raw.auth_refresh_command.clone(),
        command: raw.command.clone(),
        args: raw.args.clone(),
        interactive_args: raw.interactive_args.clone(),
        prompt_mode: raw
            .prompt_mode
            .as_deref()
            .map(parse_prompt_mode)
            .unwrap_or(PromptMode::Stdin),
        resume: raw.resume.clone(),
        session_capture: raw.session_capture.clone(),
        resume_acceptance: raw.resume_acceptance.clone(),
        session_storage: raw.session_storage.clone(),
        system_prompt_override: raw.system_prompt_override.clone(),
        tool_restrictions: raw.tool_restrictions.clone(),
        invocation_mode,
    }
}

fn validate_provider_entry_shape(name: &str, entry: &ProviderEntry) -> Result<(), LoadError> {
    entry.validate(name).map_err(LoadError::Other)
}

fn validate_provider_argv_shapes(
    name: &str,
    raw: &RawEntry,
    mode: InvocationMode,
) -> Result<(), LoadError> {
    validate_proxy_claude_argv_shape(name, raw, mode)
}

fn validate_proxy_claude_argv_shape(
    name: &str,
    raw: &RawEntry,
    mode: InvocationMode,
) -> Result<(), LoadError> {
    if !is_claude_proxy_provider(name, raw) {
        return Ok(());
    }

    let argv = build_provider_argv_tokens(raw);
    let shape =
        ClaudeToolFilterShape::detect_in_argv(&argv).unwrap_or(ClaudeToolFilterShape::NoFilter);
    validate_proxy_claude_filter_shape(mode, shape).map_err(|_| {
        LoadError::UnsafeProxyClaudeToolsRestrict {
            provider: name.to_string(),
        }
    })
}

fn is_claude_proxy_provider(name: &str, raw: &RawEntry) -> bool {
    matches!(
        provider_family(
            name,
            raw.command.as_deref(),
            &raw.args,
            raw.interactive_args.as_deref(),
        ),
        Some(ToolRestrictionKind::Claude)
    )
}

fn build_provider_argv_tokens(raw: &RawEntry) -> Vec<String> {
    let mut argv = Vec::new();
    if let Some(command) = raw.command.as_deref() {
        argv.extend(shell_split(command));
    }
    argv.extend(raw.args.iter().cloned());
    if let Some(interactive_args) = raw.interactive_args.as_ref() {
        argv.extend(interactive_args.iter().cloned());
    }
    argv
}
pub fn migrate_legacy_session_storage_file(path: &Path) -> Result<bool, String> {
    let Some(content) = read_optional_session_storage_file(path)? else {
        return Ok(false);
    };
    migrate_legacy_session_storage_content(path, &content)
}

/// Read a provider file for legacy migration, returning `None` when absent.
fn read_optional_session_storage_file(path: &Path) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|e| format_file_read_error(path, &e))
}

fn migrate_legacy_session_storage_content(path: &Path, content: &str) -> Result<bool, String> {
    let mut value = parse_legacy_session_storage_toml(content)
        .map_err(|e| format_toml_parse_error(path, &e))?;
    let records = collect_legacy_session_storage_records(&value);
    if records.is_empty() {
        return Ok(false);
    }
    apply_legacy_session_storage_migration(&mut value, records.as_slice());
    persist_migrated_session_storage_content(path, &value, records.as_slice())?;
    Ok(true)
}

fn parse_legacy_session_storage_toml(content: &str) -> Result<toml::Value, toml::de::Error> {
    toml::from_str(content)
}

fn toml_root_table_mut(value: &mut toml::Value) -> Option<&mut toml::Table> {
    value.as_table_mut()
}

fn toml_root_table(value: &toml::Value) -> Option<&toml::Table> {
    value.as_table()
}

fn collect_legacy_session_storage_records(value: &toml::Value) -> Vec<LegacySessionStorageRecord> {
    toml_root_table(value)
        .map(filter_legacy_session_storage_records)
        .unwrap_or_default()
}

fn filter_legacy_session_storage_records(root: &toml::Table) -> Vec<LegacySessionStorageRecord> {
    root.iter()
        .filter_map(|(provider_name, provider_value)| {
            parse_legacy_session_storage_record(provider_name, provider_value)
        })
        .collect()
}

fn parse_legacy_session_storage_record(
    provider_name: &str,
    provider_value: &toml::Value,
) -> Option<LegacySessionStorageRecord> {
    let storage = provider_session_storage_table(provider_value)?;
    let kind = parse_legacy_storage_kind(storage)?;
    if !is_legacy_session_storage_kind(kind) {
        return None;
    }
    let mapping = map_legacy_storage_kind_to_script_mapping(kind);
    let storage_dir = parse_legacy_storage_dir(storage, mapping.legacy_dir_key)?;
    let descriptor = map_legacy_storage_keys_to_script_descriptor(&mapping, storage_dir);
    let fields = format_script_storage_fields(&descriptor);
    Some(map_legacy_session_storage_record(provider_name, fields))
}

fn map_legacy_session_storage_record(
    provider_name: &str,
    fields: ScriptStorageFields,
) -> LegacySessionStorageRecord {
    LegacySessionStorageRecord {
        provider_name: provider_name.to_string(),
        fields,
    }
}

fn apply_legacy_session_storage_migration(
    value: &mut toml::Value,
    records: &[LegacySessionStorageRecord],
) {
    for record in records {
        apply_legacy_session_storage_record(value, record);
    }
}

fn apply_legacy_session_storage_record(
    value: &mut toml::Value,
    record: &LegacySessionStorageRecord,
) {
    let storage = provider_session_storage_table_by_name_mut(value, &record.provider_name)
        .expect("legacy session storage record should still exist");
    apply_script_storage_fields(storage, record.fields.clone());
}

fn provider_session_storage_table_by_name_mut<'a>(
    value: &'a mut toml::Value,
    provider_name: &str,
) -> Option<&'a mut toml::Table> {
    let provider_value = toml_root_table_mut(value)?.get_mut(provider_name)?;
    provider_session_storage_table_mut(provider_value)
}

fn provider_session_storage_table(provider_value: &toml::Value) -> Option<&toml::Table> {
    provider_value
        .as_table()?
        .get("session_storage")?
        .as_table()
}

fn provider_session_storage_table_mut(
    provider_value: &mut toml::Value,
) -> Option<&mut toml::Table> {
    provider_value
        .as_table_mut()?
        .get_mut("session_storage")?
        .as_table_mut()
}

fn parse_legacy_storage_kind(storage: &toml::Table) -> Option<&str> {
    storage.get("kind").and_then(toml::Value::as_str)
}

fn is_legacy_session_storage_kind(kind: &str) -> bool {
    kind == concat!("clau", "de", "_code") || kind == concat!("co", "dex")
}

fn map_legacy_storage_kind_to_script_mapping(kind: &str) -> LegacyStorageMapping {
    match kind {
        "claude_code" => LegacyStorageMapping {
            legacy_dir_key: "projects_dir",
            cwd_command: "claude-code-cwd",
            transcript_command: "claude-code-locate-transcript",
            storage_type: "claude_code",
        },
        "codex" => LegacyStorageMapping {
            legacy_dir_key: "sessions_dir",
            cwd_command: "codex-cwd",
            transcript_command: "codex-locate-transcript",
            storage_type: "codex_session",
        },
        _ => unreachable!("legacy session storage kind must be preselected"),
    }
}

fn parse_legacy_storage_dir<'a>(storage: &'a toml::Table, key: &str) -> Option<&'a str> {
    storage.get(key).and_then(toml::Value::as_str)
}

fn map_legacy_storage_keys_to_script_descriptor<'a>(
    mapping: &LegacyStorageMapping,
    storage_dir: &'a str,
) -> ScriptStorageDescriptor<'a> {
    ScriptStorageDescriptor {
        cwd_command: mapping.cwd_command,
        transcript_command: mapping.transcript_command,
        storage_dir,
        storage_type: mapping.storage_type,
    }
}

fn format_script_storage_fields(descriptor: &ScriptStorageDescriptor<'_>) -> ScriptStorageFields {
    ScriptStorageFields {
        cwd_script: format_shell_command(descriptor.cwd_command, descriptor.storage_dir),
        transcript_script: format_shell_command(
            descriptor.transcript_command,
            descriptor.storage_dir,
        ),
        storage_type: descriptor.storage_type.to_string(),
    }
}

fn apply_script_storage_fields(storage: &mut toml::Table, fields: ScriptStorageFields) {
    storage.insert(
        "kind".to_string(),
        toml::Value::String("script".to_string()),
    );
    storage.insert(
        "cwd_script".to_string(),
        toml::Value::String(fields.cwd_script),
    );
    storage.insert(
        "transcript_script".to_string(),
        toml::Value::String(fields.transcript_script),
    );
    storage.insert(
        "storage_type".to_string(),
        toml::Value::String(fields.storage_type),
    );
    storage.remove("projects_dir");
    storage.remove("sessions_dir");
}

fn format_shell_command(command: &str, argument: &str) -> String {
    format!("{command} {}", shell_word(argument))
}

fn format_toml_pretty(value: &toml::Value) -> Result<String, toml::ser::Error> {
    toml::to_string_pretty(value)
}

fn persist_migrated_session_storage_content(
    path: &Path,
    value: &toml::Value,
    records: &[LegacySessionStorageRecord],
) -> Result<(), String> {
    let new_content =
        format_toml_pretty(value).map_err(|e| format_toml_serialize_error(path, &e))?;
    fs::write(path, &new_content).map_err(|e| format_file_write_error(path, &e))?;
    tracing::warn!(
        "{}",
        format_migrated_providers_warning(path, &map_legacy_record_provider_names(records))
    );
    Ok(())
}

fn map_legacy_record_provider_names(records: &[LegacySessionStorageRecord]) -> Vec<String> {
    records
        .iter()
        .map(|record| record.provider_name.clone())
        .collect()
}

fn format_toml_parse_error(path: &Path, error: &toml::de::Error) -> String {
    format!("TOML parse error in {}: {error}", path.display())
}

fn format_toml_serialize_error(path: &Path, error: &toml::ser::Error) -> String {
    format!("Failed to serialize migrated {}: {error}", path.display())
}

fn format_file_read_error(path: &Path, error: &std::io::Error) -> String {
    format!("Failed to read {}: {error}", path.display())
}

fn format_file_write_error(path: &Path, error: &std::io::Error) -> String {
    format!("Failed to write migrated {}: {error}", path.display())
}

fn format_migrated_providers_warning(path: &Path, providers: &[String]) -> String {
    format!(
        "[config] warning: migrated legacy session_storage for providers {} to kind = \"script\" + cwd_script in {}",
        providers.join(", "),
        path.display()
    )
}

fn shell_word(input: &str) -> String {
    if is_shell_safe(input) {
        input.to_string()
    } else {
        quote_shell_word(input)
    }
}

fn is_shell_safe(input: &str) -> bool {
    input
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '~'))
}

fn quote_shell_word(input: &str) -> String {
    format!("'{}'", input.replace('\'', r#"'\''"#))
}
pub fn parse_prompt_mode(s: &str) -> PromptMode {
    match s {
        "arg" => PromptMode::Arg,
        _ => PromptMode::Stdin,
    }
}
