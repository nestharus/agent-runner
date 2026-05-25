use super::formatter::shell_word_arg;
use super::parser::{split_migration_command, split_optional_command, turn_script_storage_parts};
use super::predicate::provider_has_runtime_blocks;
use super::validator::{
    provider_table_from_value, take_optional_string_array, take_provider_command,
    take_string_array, validate_old_top_level_provider,
};
use std::path::Path;

pub(crate) fn old_top_level_provider_table(table: &mut toml::Table) -> Result<toml::Value, String> {
    let provider = take_old_top_level_provider_fields(table);
    validate_old_top_level_provider(&provider)?;
    Ok(toml::Value::Table(provider))
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
    let mut provider = provider_table_from_value(provider_value, path)?;
    let original_provider = provider.clone();
    let has_runtime_blocks = provider_has_runtime_blocks(&provider);
    let command = take_provider_command(&mut provider, path)?;
    let model_args = take_string_array(&mut provider, "args")?;
    let model_interactive_args = take_optional_string_array(&mut provider, "interactive_args")?;
    let provider_name =
        derive_migrated_provider_name(&mut provider, command.as_deref(), &model_args);
    Ok(ProviderMigrationDraft {
        provider,
        original_provider,
        has_runtime_blocks,
        command,
        model_args,
        model_interactive_args,
        provider_name,
    })
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
    let (runtime_args, model_args) =
        combine_command_runtime_args(command_runtime_args.clone(), model_args);
    let (runtime_interactive_args, model_interactive_args) =
        partition_optional_model_specific_args(model_interactive_args);
    ProviderRuntimeParts {
        runtime_command,
        command_runtime_args,
        runtime_args,
        model_args,
        runtime_interactive_args,
        model_interactive_args,
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
        .unwrap_or_else(|| toml::Value::String("stdin".to_string()))
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
    reduced.insert("name".to_string(), toml::Value::String(provider_name));
    reduced.insert("args".to_string(), string_array_value(model_args));
    if let Some(interactive_args) = model_interactive_args {
        reduced.insert(
            "interactive_args".to_string(),
            string_array_value(interactive_args),
        );
    }
    toml::Value::Table(reduced)
}

pub(crate) fn session_storage_from_entry(entry: &toml::Value) -> Option<toml::Table> {
    entry
        .as_table()
        .and_then(|table| table.get("turn_script"))
        .and_then(toml::Value::as_str)
        .and_then(storage_from_turn_script)
}

pub(crate) fn storage_from_turn_script(turn_script: &str) -> Option<toml::Table> {
    let (adapter, storage_root) = turn_script_storage_parts(turn_script)?;
    let adapter_name = Path::new(&adapter).file_name()?.to_str()?;
    let adapter = turn_script_storage_adapter(adapter_name)?;
    Some(storage_table_from_turn_script(&storage_root, adapter))
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
    let storage_root = shell_word_arg(storage_root);
    let mut storage = toml::Table::new();
    storage.insert(
        "kind".to_string(),
        toml::Value::String("script".to_string()),
    );
    storage.insert(
        "cwd_script".to_string(),
        toml::Value::String(format!("{} {storage_root}", adapter.cwd_adapter)),
    );
    storage.insert(
        "transcript_script".to_string(),
        toml::Value::String(format!("{} {storage_root}", adapter.transcript_adapter)),
    );
    storage.insert(
        "storage_type".to_string(),
        toml::Value::String(adapter.storage_type.to_string()),
    );
    storage
}

pub(crate) fn string_array_value(values: Vec<String>) -> toml::Value {
    toml::Value::Array(values.into_iter().map(toml::Value::String).collect())
}

pub(crate) fn derive_migration_provider_name(command: &str, args: &[String]) -> String {
    let command_parts = split_migration_command(command);
    let Some(command) = command_parts.first() else {
        return command.to_string();
    };
    oulipoly_config::derive_provider_name(command, &migration_provider_args(&command_parts, args))
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
        match arg.as_str() {
            "--model" | "-m" => {
                model_specific.push(arg);
                if let Some(value) = iter.next() {
                    model_specific.push(value);
                }
            }
            "-c" => {
                if let Some(value) = iter.next() {
                    if value
                        .split_once('=')
                        .is_some_and(|(key, _)| key.starts_with("model_"))
                    {
                        model_specific.push(arg);
                        model_specific.push(value);
                    } else {
                        runtime.push(arg);
                        runtime.push(value);
                    }
                } else {
                    runtime.push(arg);
                }
            }
            _ => runtime.push(arg),
        }
    }
    (runtime, model_specific)
}
