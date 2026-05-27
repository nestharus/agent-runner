//! ## Declared roles
//!
//! `mapper`, `validator`, `predicate`, `parser`, `filter`, `formatter`

use super::formatter::{
    default_prompt_mode, format_storage_command, provider_args_key, provider_interactive_args_key,
    provider_name_key, providers_key, session_storage_key, shell_word_arg, storage_cwd_script_key,
    storage_kind_key, storage_kind_value, storage_transcript_script_key, storage_type_key,
    storage_type_value,
};
use super::orchestration::ConfigMigrationReport;
use super::parser::{
    model_specific_config_key, split_migration_command, split_optional_command,
    turn_script_storage_parts,
};
use super::predicate::provider_has_runtime_blocks;
use super::validator::{
    provider_table_from_value, take_optional_string_array, take_provider_command,
    take_string_array, validate_old_top_level_provider,
};
use crate::cli::paths::default_models_dir;
use std::iter::Peekable;
use std::path::{Path, PathBuf};

pub(crate) fn migrate_config_paths(models_dir_override: Option<&Path>) -> (PathBuf, PathBuf) {
    let models_dir = models_dir_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_models_dir);
    let config_root = models_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let providers_path = config_root.join("providers.toml");
    (models_dir, providers_path)
}

pub(crate) fn config_migration_report(
    providers_touched: usize,
    model_files_rewritten: usize,
    moved_blocks: Vec<String>,
) -> ConfigMigrationReport {
    ConfigMigrationReport {
        providers_touched,
        model_files_rewritten,
        moved_blocks,
    }
}

pub(crate) fn old_top_level_provider_table(table: &mut toml::Table) -> Result<toml::Value, String> {
    let provider = take_old_top_level_provider_fields(table);
    validate_old_top_level_provider(&provider)?;
    Ok(old_top_level_provider_value(provider))
}

pub(crate) fn old_top_level_provider_value(provider: toml::Table) -> toml::Value {
    toml::Value::Table(provider)
}

pub(crate) fn original_provider_value(provider: toml::Table) -> toml::Value {
    toml::Value::Table(provider)
}

pub(crate) fn take_old_top_level_provider_fields(table: &mut toml::Table) -> toml::Table {
    let mut provider = toml::Table::new();
    for key in [
        "command",
        "args",
        "interactive_args",
        "resume",
        "session_capture",
        "session_storage",
        "resume_acceptance",
    ] {
        if let Some(value) = table.remove(key) {
            provider.insert(key.to_string(), value);
        }
    }
    provider
}

pub(crate) struct ProviderMigrationDraft {
    pub(crate) provider: toml::Table,
    pub(crate) original_provider: toml::Table,
    pub(crate) has_runtime_blocks: bool,
    pub(crate) command: Option<String>,
    pub(crate) model_args: Vec<String>,
    pub(crate) model_interactive_args: Option<Vec<String>>,
    pub(crate) provider_name: Option<String>,
}

pub(crate) fn provider_migration_draft(
    provider_value: toml::Value,
    path: &Path,
) -> Result<ProviderMigrationDraft, String> {
    let mut provider = provider_migration_table(provider_value, path)?;
    let original_provider = original_provider_table(&provider);
    let has_runtime_blocks = provider_runtime_block_present(&provider);
    let inputs = take_provider_migration_inputs(&mut provider, path)?;
    let provider_name =
        derive_migrated_provider_name(&mut provider, inputs.command.as_deref(), &inputs.model_args);
    Ok(provider_migration_draft_from_parts(
        provider,
        original_provider,
        has_runtime_blocks,
        inputs,
        provider_name,
    ))
}

pub(crate) fn provider_migration_table(
    provider_value: toml::Value,
    path: &Path,
) -> Result<toml::Table, String> {
    provider_table_from_value(provider_value, path)
}

pub(crate) fn original_provider_table(provider: &toml::Table) -> toml::Table {
    provider.clone()
}

pub(crate) fn provider_runtime_block_present(provider: &toml::Table) -> bool {
    provider_has_runtime_blocks(provider)
}

pub(crate) struct ProviderMigrationInputs {
    pub(crate) command: Option<String>,
    pub(crate) model_args: Vec<String>,
    pub(crate) model_interactive_args: Option<Vec<String>>,
}

pub(crate) fn take_provider_migration_inputs(
    provider: &mut toml::Table,
    path: &Path,
) -> Result<ProviderMigrationInputs, String> {
    Ok(ProviderMigrationInputs {
        command: take_provider_command(provider, path)?,
        model_args: take_string_array(provider, "args")?,
        model_interactive_args: take_optional_string_array(provider, "interactive_args")?,
    })
}

pub(crate) fn provider_migration_draft_from_parts(
    provider: toml::Table,
    original_provider: toml::Table,
    has_runtime_blocks: bool,
    inputs: ProviderMigrationInputs,
    provider_name: Option<String>,
) -> ProviderMigrationDraft {
    ProviderMigrationDraft {
        provider,
        original_provider,
        has_runtime_blocks,
        command: inputs.command,
        model_args: inputs.model_args,
        model_interactive_args: inputs.model_interactive_args,
        provider_name,
    }
}

pub(crate) struct ProviderRuntimeParts {
    pub(crate) runtime_command: Option<String>,
    pub(crate) command_runtime_args: Vec<String>,
    pub(crate) runtime_args: Vec<String>,
    pub(crate) model_args: Vec<String>,
    pub(crate) runtime_interactive_args: Option<Vec<String>>,
    pub(crate) model_interactive_args: Option<Vec<String>>,
}

pub(crate) fn provider_runtime_parts(
    command: Option<&String>,
    model_args: Vec<String>,
    model_interactive_args: Option<Vec<String>>,
) -> ProviderRuntimeParts {
    let command_parts = split_optional_command(command.map(String::as_str));
    let runtime_command = runtime_command_from_parts(command, &command_parts);
    let command_runtime_args = command_runtime_args_from_parts(&command_parts);
    let partitioned = provider_runtime_arg_partitions(
        command_runtime_args.clone(),
        model_args,
        model_interactive_args,
    );
    provider_runtime_parts_from_partitions(runtime_command, command_runtime_args, partitioned)
}

pub(crate) struct ProviderRuntimeArgPartitions {
    pub(crate) runtime_args: Vec<String>,
    pub(crate) model_args: Vec<String>,
    pub(crate) runtime_interactive_args: Option<Vec<String>>,
    pub(crate) model_interactive_args: Option<Vec<String>>,
}

pub(crate) fn provider_runtime_arg_partitions(
    command_runtime_args: Vec<String>,
    model_args: Vec<String>,
    model_interactive_args: Option<Vec<String>>,
) -> ProviderRuntimeArgPartitions {
    let (runtime_args, model_args) = combine_command_runtime_args(command_runtime_args, model_args);
    let (runtime_interactive_args, model_interactive_args) =
        partition_optional_model_specific_args(model_interactive_args);
    ProviderRuntimeArgPartitions {
        runtime_args,
        model_args,
        runtime_interactive_args,
        model_interactive_args,
    }
}

pub(crate) fn provider_runtime_parts_from_partitions(
    runtime_command: Option<String>,
    command_runtime_args: Vec<String>,
    partitions: ProviderRuntimeArgPartitions,
) -> ProviderRuntimeParts {
    ProviderRuntimeParts {
        runtime_command,
        command_runtime_args,
        runtime_args: partitions.runtime_args,
        model_args: partitions.model_args,
        runtime_interactive_args: partitions.runtime_interactive_args,
        model_interactive_args: partitions.model_interactive_args,
    }
}

pub(crate) fn combine_command_runtime_args(
    command_runtime_args: Vec<String>,
    model_args: Vec<String>,
) -> (Vec<String>, Vec<String>) {
    let (runtime_args, model_args) = partition_model_specific_args(model_args);
    if command_runtime_args.is_empty() {
        return (runtime_args, model_args);
    }
    let mut combined = command_runtime_args;
    combined.extend(runtime_args);
    (combined, model_args)
}

pub(crate) fn partition_optional_model_specific_args(
    args: Option<Vec<String>>,
) -> (Option<Vec<String>>, Option<Vec<String>>) {
    args.map(partition_model_specific_args)
        .map(|(runtime, model)| (Some(runtime), Some(model)))
        .unwrap_or((None, None))
}

pub(crate) struct ProviderRuntimeBlocks {
    pub(crate) resume: Option<toml::Value>,
    pub(crate) session_capture: Option<toml::Value>,
    pub(crate) session_storage: Option<toml::Value>,
    pub(crate) resume_acceptance: Option<toml::Value>,
}

pub(crate) fn take_provider_prompt_mode(
    provider: &mut toml::Table,
    global_prompt_mode: Option<toml::Value>,
) -> toml::Value {
    provider
        .remove("prompt_mode")
        .or(global_prompt_mode)
        .unwrap_or_else(default_prompt_mode_value)
}

pub(crate) fn default_prompt_mode_value() -> toml::Value {
    toml::Value::String(default_prompt_mode())
}

pub(crate) fn take_provider_runtime_blocks(provider: &mut toml::Table) -> ProviderRuntimeBlocks {
    ProviderRuntimeBlocks {
        resume: provider.remove("resume"),
        session_capture: provider.remove("session_capture"),
        session_storage: provider.remove("session_storage"),
        resume_acceptance: provider.remove("resume_acceptance"),
    }
}

pub(crate) fn provider_runtime_block_entries(
    blocks: ProviderRuntimeBlocks,
) -> Vec<(&'static str, toml::Value)> {
    [
        ("resume", blocks.resume),
        ("session_capture", blocks.session_capture),
        ("session_storage", blocks.session_storage),
        ("resume_acceptance", blocks.resume_acceptance),
    ]
    .into_iter()
    .filter_map(|(key, value)| value.map(|value| (key, value)))
    .collect()
}

pub(crate) fn derive_migrated_provider_name(
    provider: &mut toml::Table,
    command: Option<&str>,
    model_args: &[String],
) -> Option<String> {
    provider
        .remove("name")
        .and_then(|value| value.as_str().map(ToString::to_string))
        .or_else(|| command.map(|command| derive_migration_provider_name(command, model_args)))
}

pub(crate) fn runtime_command_from_parts(
    command: Option<&String>,
    command_parts: &[String],
) -> Option<String> {
    command.map(|command| {
        command_parts
            .first()
            .cloned()
            .unwrap_or_else(|| command.clone())
    })
}

pub(crate) fn command_runtime_args_from_parts(command_parts: &[String]) -> Vec<String> {
    command_parts.iter().skip(1).cloned().collect()
}

pub(crate) fn reduced_provider_value(
    provider_name: String,
    model_args: Vec<String>,
    model_interactive_args: Option<Vec<String>>,
) -> toml::Value {
    let mut reduced = toml::Table::new();
    reduced.insert(provider_name_key(), toml::Value::String(provider_name));
    reduced.insert(provider_args_key(), string_array_value(model_args));
    if let Some(interactive_args) = model_interactive_args {
        reduced.insert(
            provider_interactive_args_key(),
            string_array_value(interactive_args),
        );
    }
    toml::Value::Table(reduced)
}

pub(crate) fn provider_array_value(migrated: toml::Value) -> toml::Value {
    toml::Value::Array(vec![migrated])
}

pub(crate) fn session_storage_value(storage: toml::Table) -> toml::Value {
    toml::Value::Table(storage)
}

pub(crate) fn provider_array_entry(migrated: toml::Value) -> (String, toml::Value) {
    (providers_key(), provider_array_value(migrated))
}

pub(crate) fn session_storage_entry(storage: toml::Table) -> (String, toml::Value) {
    (session_storage_key(), session_storage_value(storage))
}

pub(crate) fn session_storage_from_entry(entry: &toml::Value) -> Option<toml::Table> {
    entry
        .as_table()
        .and_then(|table| table.get("turn_script"))
        .and_then(toml::Value::as_str)
        .and_then(storage_from_turn_script)
}

pub(crate) fn storage_from_turn_script(turn_script: &str) -> Option<toml::Table> {
    let spec = turn_script_storage_spec(turn_script)?;
    Some(storage_table_from_turn_script(
        &spec.storage_root,
        spec.adapter,
    ))
}

pub(crate) struct TurnScriptStorageSpec {
    pub(crate) adapter: TurnScriptStorageAdapter,
    pub(crate) storage_root: String,
}

pub(crate) fn turn_script_storage_spec(turn_script: &str) -> Option<TurnScriptStorageSpec> {
    let (adapter, storage_root) = turn_script_storage_parts(turn_script)?;
    let adapter_name = Path::new(&adapter).file_name()?.to_str()?;
    let adapter = turn_script_storage_adapter(adapter_name)?;
    Some(TurnScriptStorageSpec {
        adapter,
        storage_root,
    })
}

pub(crate) struct TurnScriptStorageAdapter {
    pub(crate) cwd_adapter: &'static str,
    pub(crate) transcript_adapter: &'static str,
    pub(crate) storage_type: &'static str,
}

pub(crate) fn turn_script_storage_adapter(adapter_name: &str) -> Option<TurnScriptStorageAdapter> {
    match adapter_name {
        "claude-code-turns" => Some(TurnScriptStorageAdapter {
            cwd_adapter: "claude-code-cwd",
            transcript_adapter: "claude-code-locate-transcript",
            storage_type: "claude_code",
        }),
        "codex-turns" => Some(TurnScriptStorageAdapter {
            cwd_adapter: "codex-cwd",
            transcript_adapter: "codex-locate-transcript",
            storage_type: "codex_session",
        }),
        _ => None,
    }
}

pub(crate) fn storage_table_from_turn_script(
    storage_root: &str,
    adapter: TurnScriptStorageAdapter,
) -> toml::Table {
    storage_table_from_parts(
        adapter.storage_type,
        turn_script_storage_commands(storage_root, &adapter),
    )
}

pub(crate) struct TurnScriptStorageCommands {
    pub(crate) cwd_script: String,
    pub(crate) transcript_script: String,
}

pub(crate) fn turn_script_storage_commands(
    storage_root: &str,
    adapter: &TurnScriptStorageAdapter,
) -> TurnScriptStorageCommands {
    let storage_root = shell_word_arg(storage_root);
    TurnScriptStorageCommands {
        cwd_script: format_storage_command(adapter.cwd_adapter, &storage_root),
        transcript_script: format_storage_command(adapter.transcript_adapter, &storage_root),
    }
}

pub(crate) fn storage_table_from_parts(
    storage_type: &str,
    commands: TurnScriptStorageCommands,
) -> toml::Table {
    let mut storage = toml::Table::new();
    storage.insert(
        storage_kind_key(),
        toml::Value::String(storage_kind_value()),
    );
    storage.insert(
        storage_cwd_script_key(),
        toml::Value::String(commands.cwd_script),
    );
    storage.insert(
        storage_transcript_script_key(),
        toml::Value::String(commands.transcript_script),
    );
    storage.insert(
        storage_type_key(),
        toml::Value::String(storage_type_value(storage_type)),
    );
    storage
}

pub(crate) fn string_array_value(values: Vec<String>) -> toml::Value {
    toml::Value::Array(values.into_iter().map(toml::Value::String).collect())
}

pub(crate) fn derive_migration_provider_name(command: &str, args: &[String]) -> String {
    let command_parts = split_migration_command(command);
    derive_migration_provider_name_from_parts(command, &command_parts, args)
}

pub(crate) fn derive_migration_provider_name_from_parts(
    fallback_command: &str,
    command_parts: &[String],
    args: &[String],
) -> String {
    let Some(command) = command_parts.first() else {
        return fallback_command.to_string();
    };
    oulipoly_config::derive_provider_name(command, &migration_provider_args(command_parts, args))
}

pub(crate) fn migration_provider_args(command_parts: &[String], args: &[String]) -> Vec<String> {
    let mut derived_args = command_parts.iter().skip(1).cloned().collect::<Vec<_>>();
    derived_args.extend(args.iter().cloned());
    derived_args
}

pub(crate) fn partition_model_specific_args(args: Vec<String>) -> (Vec<String>, Vec<String>) {
    let mut runtime = Vec::new();
    let mut model_specific = Vec::new();
    let mut iter = args.into_iter().peekable();
    while let Some(arg) = iter.next() {
        push_partitioned_arg(
            partition_next_arg(arg, &mut iter),
            &mut runtime,
            &mut model_specific,
        );
    }
    (runtime, model_specific)
}

pub(crate) enum ArgPartition {
    Runtime(Vec<String>),
    ModelSpecific(Vec<String>),
}

pub(crate) fn partition_next_arg<I>(arg: String, iter: &mut Peekable<I>) -> ArgPartition
where
    I: Iterator<Item = String>,
{
    match arg.as_str() {
        "--model" | "-m" => partition_model_flag(arg, iter),
        "-c" => partition_config_arg(arg, iter),
        _ => ArgPartition::Runtime(vec![arg]),
    }
}

pub(crate) fn partition_model_flag<I>(arg: String, iter: &mut Peekable<I>) -> ArgPartition
where
    I: Iterator<Item = String>,
{
    let mut args = vec![arg];
    if let Some(value) = iter.next() {
        args.push(value);
    }
    ArgPartition::ModelSpecific(args)
}

pub(crate) fn partition_config_arg<I>(arg: String, iter: &mut Peekable<I>) -> ArgPartition
where
    I: Iterator<Item = String>,
{
    let Some(value) = iter.next() else {
        return ArgPartition::Runtime(vec![arg]);
    };
    if is_model_specific_config(&value) {
        ArgPartition::ModelSpecific(vec![arg, value])
    } else {
        ArgPartition::Runtime(vec![arg, value])
    }
}

pub(crate) fn is_model_specific_config(value: &str) -> bool {
    model_specific_config_key(value).is_some_and(|key| key.starts_with("model_"))
}

pub(crate) fn push_partitioned_arg(
    partition: ArgPartition,
    runtime: &mut Vec<String>,
    model_specific: &mut Vec<String>,
) {
    match partition {
        ArgPartition::Runtime(args) => runtime.extend(args),
        ArgPartition::ModelSpecific(args) => model_specific.extend(args),
    }
}
