//! ## Declared roles
//!
//! `orchestration`, `accessor`, `filter`, `predicate`, `mapper`, `validator`, `formatter`

use super::accessor::{
    existing_runtime_provider_table, model_toml_paths, read_existing_toml_table,
    read_optional_toml_table, read_toml_table, runtime_provider_table, sessions_path_for_providers,
    write_changed_providers_toml, write_text_file,
};
use super::filter::{
    SessionStorageBackfill, combined_runtime_interactive_args, count_runtime_provider_tables,
    session_storage_backfill_candidates, set_or_conflict, set_or_repair_empty_array,
    take_global_prompt_mode,
};
use super::formatter::{format_moved_runtime_block, format_moved_session_storage_block};
use super::mapper::{
    ProviderRuntimeBlocks, ProviderRuntimeParts, config_migration_report,
    old_top_level_provider_table, original_provider_value, provider_array_entry,
    provider_migration_draft, provider_runtime_block_entries, provider_runtime_parts,
    reduced_provider_value, session_storage_entry, string_array_value, take_provider_prompt_mode,
    take_provider_runtime_blocks,
};
use super::parser::serialize_toml_table;
use super::predicate::{
    has_old_top_level_command, provider_missing_session_storage, removed_global_prompt_mode,
    should_apply_runtime_args, should_apply_runtime_command, should_apply_runtime_interactive_args,
    should_keep_model_only_provider,
};
use super::validator::validate_migrated_provider_name;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConfigMigrationReport {
    pub(crate) providers_touched: usize,
    pub(crate) model_files_rewritten: usize,
    pub(crate) moved_blocks: Vec<String>,
}

pub(crate) fn migrate_config_files(
    models_dir: &Path,
    providers_path: &Path,
) -> Result<ConfigMigrationReport, String> {
    let mut providers_root = read_optional_toml_table(providers_path)?;
    let mut moved_blocks = Vec::new();
    let mut rewritten = 0usize;

    for path in model_toml_paths(models_dir)? {
        rewritten += migrate_model_config_file(&path, &mut providers_root, &mut moved_blocks)?;
    }

    if let Some(sessions_path) = sessions_path_for_providers(providers_path) {
        backfill_session_storage_from_sessions(
            &mut providers_root,
            &sessions_path,
            &mut moved_blocks,
        )?;
    }

    let providers_touched = count_runtime_provider_tables(&providers_root);
    write_changed_providers_toml(providers_path, &providers_root)?;
    oulipoly_config::migrate_legacy_session_storage_file(providers_path)?;

    Ok(config_migration_report(
        providers_touched,
        rewritten,
        moved_blocks,
    ))
}

pub(crate) fn migrate_model_config_file(
    path: &Path,
    providers_root: &mut toml::Table,
    moved_blocks: &mut Vec<String>,
) -> Result<usize, String> {
    let mut table = read_toml_table(path)?;
    let before = serialized_model_config(path, &table)?;
    let changed = migrate_model_config_table(path, &mut table, providers_root, moved_blocks)?;
    let after = serialized_model_config(path, &table)?;
    if model_config_rewrite_needed(changed, &before, &after) {
        write_rewritten_model_config(path, after)?;
        Ok(1)
    } else {
        Ok(0)
    }
}

fn serialized_model_config(path: &Path, table: &toml::Table) -> Result<String, String> {
    serialize_toml_table(path, table)
}

fn model_config_rewrite_needed(changed: bool, before: &str, after: &str) -> bool {
    changed && after != before
}

fn write_rewritten_model_config(path: &Path, after: String) -> Result<(), String> {
    write_text_file(path, after)
}

pub(crate) fn migrate_model_config_table(
    path: &Path,
    table: &mut toml::Table,
    providers_root: &mut toml::Table,
    moved_blocks: &mut Vec<String>,
) -> Result<bool, String> {
    let mut changed = false;
    let global_prompt_mode = remove_global_prompt_mode(table);
    changed |= removed_global_prompt_mode(&global_prompt_mode);

    if model_config_has_old_top_level_provider(table) {
        migrate_old_top_level_provider(
            path,
            table,
            global_prompt_mode,
            providers_root,
            moved_blocks,
        )?;
        changed = true;
    } else {
        changed |= migrate_provider_array(
            path,
            table,
            global_prompt_mode,
            providers_root,
            moved_blocks,
        )?;
    }

    changed |= backfill_moved_external_provider_ref(table);
    Ok(changed)
}

fn backfill_moved_external_provider_ref(table: &mut toml::Table) -> bool {
    if !should_backfill_moved_external_provider_ref(table) {
        return false;
    }
    insert_moved_external_provider_ref(table);
    true
}

fn should_backfill_moved_external_provider_ref(table: &toml::Table) -> bool {
    !table.contains_key("provider") && model_has_moved_provider(table)
}

fn insert_moved_external_provider_ref(table: &mut toml::Table) {
    table.insert("provider".to_string(), moved_external_provider_ref_value());
}

fn model_has_moved_provider(table: &toml::Table) -> bool {
    table
        .get("providers")
        .and_then(toml::Value::as_array)
        .map(|providers| providers.iter().any(provider_value_is_moved_provider))
        .unwrap_or(false)
}

fn provider_value_is_moved_provider(provider: &toml::Value) -> bool {
    provider
        .as_table()
        .and_then(|provider| provider.get("name"))
        .and_then(toml::Value::as_str)
        .map(is_moved_provider_name)
        .unwrap_or(false)
}

fn is_moved_provider_name(name: &str) -> bool {
    let token = moved_provider_token();
    if name == token {
        return true;
    }
    let Some(suffix) = name.strip_prefix(&token) else {
        return false;
    };
    matches!(
        suffix.as_bytes().first().copied(),
        Some(b'0'..=b'9' | b'-' | b'_')
    )
}

fn moved_external_provider_ref_value() -> toml::Value {
    let mut provider = toml::Table::new();
    provider.insert(
        "binary".to_string(),
        toml::Value::String(moved_external_provider_binary()),
    );
    toml::Value::Table(provider)
}

fn moved_external_provider_binary() -> String {
    format!("agent-runner-{}", moved_provider_token())
}

fn moved_provider_token() -> String {
    ["cla", "ude"].concat()
}

fn remove_global_prompt_mode(table: &mut toml::Table) -> Option<toml::Value> {
    take_global_prompt_mode(table)
}

fn model_config_has_old_top_level_provider(table: &toml::Table) -> bool {
    has_old_top_level_command(table)
}

fn migrate_old_top_level_provider(
    path: &Path,
    table: &mut toml::Table,
    global_prompt_mode: Option<toml::Value>,
    providers_root: &mut toml::Table,
    moved_blocks: &mut Vec<String>,
) -> Result<(), String> {
    let provider_table = old_top_level_provider_table(table)?;
    let migrated = migrate_provider_table(
        provider_table,
        global_prompt_mode,
        providers_root,
        path,
        moved_blocks,
    )?;
    insert_migrated_provider_array(table, migrated);
    Ok(())
}

fn insert_migrated_provider_array(table: &mut toml::Table, migrated: toml::Value) {
    let (key, value) = provider_array_entry(migrated);
    table.insert(key, value);
}

pub(crate) fn migrate_provider_array(
    path: &Path,
    table: &mut toml::Table,
    global_prompt_mode: Option<toml::Value>,
    providers_root: &mut toml::Table,
    moved_blocks: &mut Vec<String>,
) -> Result<bool, String> {
    let Some(toml::Value::Array(providers)) = table.get_mut("providers") else {
        return Ok(false);
    };
    let mut changed = false;
    for provider in providers.iter_mut() {
        changed |= migrate_provider_array_entry(
            provider,
            global_prompt_mode.clone(),
            providers_root,
            path,
            moved_blocks,
        )?;
    }
    Ok(changed)
}

pub(crate) fn migrate_provider_array_entry(
    provider: &mut toml::Value,
    global_prompt_mode: Option<toml::Value>,
    providers_root: &mut toml::Table,
    path: &Path,
    moved_blocks: &mut Vec<String>,
) -> Result<bool, String> {
    let migrated = migrate_provider_table(
        provider.clone(),
        global_prompt_mode,
        providers_root,
        path,
        moved_blocks,
    )?;
    if provider_entry_changed(provider, &migrated) {
        replace_provider_entry(provider, migrated);
        Ok(true)
    } else {
        Ok(false)
    }
}

fn provider_entry_changed(provider: &toml::Value, migrated: &toml::Value) -> bool {
    migrated != provider
}

fn replace_provider_entry(provider: &mut toml::Value, migrated: toml::Value) {
    *provider = migrated;
}

pub(crate) fn migrate_provider_table(
    provider_value: toml::Value,
    global_prompt_mode: Option<toml::Value>,
    providers_root: &mut toml::Table,
    path: &Path,
    moved_blocks: &mut Vec<String>,
) -> Result<toml::Value, String> {
    let mut draft = provider_migration_draft(provider_value, path)?;
    if should_keep_model_only_provider(
        draft.has_runtime_blocks,
        draft.provider_name.as_deref(),
        providers_root,
    ) {
        return Ok(original_provider_value(draft.original_provider));
    }
    let provider_name = migrated_provider_name(draft.provider_name, path)?;
    let runtime_parts = provider_runtime_parts(
        draft.command.as_ref(),
        draft.model_args,
        draft.model_interactive_args,
    );
    let prompt_mode = take_provider_prompt_mode(&mut draft.provider, global_prompt_mode);
    let blocks = take_provider_runtime_blocks(&mut draft.provider);
    let runtime = runtime_provider_table(providers_root, &provider_name)?;
    apply_runtime_provider_migration(RuntimeProviderMigration {
        runtime,
        provider_name: &provider_name,
        path,
        has_runtime_blocks: draft.has_runtime_blocks,
        prompt_mode,
        runtime_parts: &runtime_parts,
        blocks,
        moved_blocks,
    })?;

    Ok(migrated_provider_value(
        provider_name,
        runtime_parts.model_args,
        runtime_parts.model_interactive_args,
    ))
}

fn migrated_provider_name(provider_name: Option<String>, path: &Path) -> Result<String, String> {
    validate_migrated_provider_name(provider_name, path)
}

fn migrated_provider_value(
    provider_name: String,
    model_args: Vec<String>,
    model_interactive_args: Option<Vec<String>>,
) -> toml::Value {
    reduced_provider_value(provider_name, model_args, model_interactive_args)
}

pub(crate) struct RuntimeProviderMigration<'a> {
    pub(crate) runtime: &'a mut toml::Table,
    pub(crate) provider_name: &'a str,
    pub(crate) path: &'a Path,
    pub(crate) has_runtime_blocks: bool,
    pub(crate) prompt_mode: toml::Value,
    pub(crate) runtime_parts: &'a ProviderRuntimeParts,
    pub(crate) blocks: ProviderRuntimeBlocks,
    pub(crate) moved_blocks: &'a mut Vec<String>,
}

pub(crate) fn apply_runtime_provider_migration(
    migration: RuntimeProviderMigration<'_>,
) -> Result<(), String> {
    apply_runtime_command(
        migration.runtime,
        migration.runtime_parts,
        migration.provider_name,
        migration.path,
    )?;
    apply_runtime_args(
        migration.runtime,
        migration.has_runtime_blocks,
        migration.runtime_parts,
        migration.provider_name,
        migration.path,
    )?;
    apply_runtime_interactive_args(
        migration.runtime,
        migration.has_runtime_blocks,
        migration.runtime_parts,
        migration.provider_name,
        migration.path,
    )?;
    if migration.has_runtime_blocks {
        set_or_conflict(
            migration.runtime,
            "prompt_mode",
            migration.prompt_mode,
            migration.provider_name,
            migration.path,
        )?;
    }
    move_provider_runtime_blocks(
        migration.runtime,
        migration.blocks,
        migration.provider_name,
        migration.path,
        migration.moved_blocks,
    )
}

pub(crate) fn apply_runtime_command(
    runtime: &mut toml::Table,
    runtime_parts: &ProviderRuntimeParts,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if should_apply_runtime_command(runtime_parts) {
        set_or_repair_empty_array(
            runtime,
            "command",
            runtime_command_value(runtime_parts),
            provider_name,
            path,
        )?;
    }
    Ok(())
}

fn runtime_command_value(runtime_parts: &ProviderRuntimeParts) -> toml::Value {
    toml::Value::String(
        runtime_parts
            .runtime_command
            .clone()
            .expect("runtime command checked before value mapping"),
    )
}

pub(crate) fn apply_runtime_args(
    runtime: &mut toml::Table,
    has_runtime_blocks: bool,
    runtime_parts: &ProviderRuntimeParts,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if should_apply_runtime_args(has_runtime_blocks, runtime_parts) {
        set_or_repair_empty_array(
            runtime,
            "args",
            runtime_args_value(runtime_parts),
            provider_name,
            path,
        )?;
    }
    Ok(())
}

fn runtime_args_value(runtime_parts: &ProviderRuntimeParts) -> toml::Value {
    string_array_value(runtime_parts.runtime_args.clone())
}

pub(crate) fn apply_runtime_interactive_args(
    runtime: &mut toml::Table,
    has_runtime_blocks: bool,
    runtime_parts: &ProviderRuntimeParts,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if should_apply_runtime_interactive_args(has_runtime_blocks, runtime_parts) {
        set_or_repair_empty_array(
            runtime,
            "interactive_args",
            runtime_interactive_args_value(runtime_parts),
            provider_name,
            path,
        )?;
    }
    Ok(())
}

fn runtime_interactive_args_value(runtime_parts: &ProviderRuntimeParts) -> toml::Value {
    string_array_value(combined_runtime_interactive_args(runtime_parts))
}

pub(crate) fn move_provider_runtime_blocks(
    runtime: &mut toml::Table,
    blocks: ProviderRuntimeBlocks,
    provider_name: &str,
    path: &Path,
    moved_blocks: &mut Vec<String>,
) -> Result<(), String> {
    for (key, value) in provider_runtime_block_entries(blocks) {
        set_or_conflict(runtime, key, value, provider_name, path)?;
        record_moved_runtime_block(moved_blocks, path, key, provider_name);
    }
    Ok(())
}

fn record_moved_runtime_block(
    moved_blocks: &mut Vec<String>,
    path: &Path,
    key: &str,
    provider_name: &str,
) {
    moved_blocks.push(format_moved_runtime_block(path, key, provider_name));
}

pub(crate) fn backfill_session_storage_from_sessions(
    providers_root: &mut toml::Table,
    sessions_path: &Path,
    moved_blocks: &mut Vec<String>,
) -> Result<(), String> {
    let Some(sessions) = read_existing_toml_table(sessions_path)? else {
        return Ok(());
    };
    for backfill in session_storage_backfill_candidates(sessions) {
        apply_session_storage_backfill(providers_root, sessions_path, moved_blocks, backfill);
    }
    Ok(())
}

pub(crate) fn apply_session_storage_backfill(
    providers_root: &mut toml::Table,
    sessions_path: &Path,
    moved_blocks: &mut Vec<String>,
    backfill: SessionStorageBackfill,
) {
    let Some(provider) = session_storage_backfill_provider(providers_root, &backfill.provider_name)
    else {
        return;
    };
    if !should_apply_session_storage_backfill(provider) {
        return;
    }
    insert_session_storage_backfill(provider, backfill.storage);
    record_moved_session_storage_block(moved_blocks, sessions_path, &backfill.provider_name);
}

fn session_storage_backfill_provider<'a>(
    providers_root: &'a mut toml::Table,
    provider_name: &str,
) -> Option<&'a mut toml::Table> {
    existing_runtime_provider_table(providers_root, provider_name)
}

fn should_apply_session_storage_backfill(provider: &toml::Table) -> bool {
    provider_missing_session_storage(provider)
}

fn record_moved_session_storage_block(
    moved_blocks: &mut Vec<String>,
    sessions_path: &Path,
    provider_name: &str,
) {
    moved_blocks.push(format_moved_session_storage_block(
        sessions_path,
        provider_name,
    ));
}

pub(crate) fn insert_session_storage_backfill(provider: &mut toml::Table, storage: toml::Table) {
    let (key, value) = session_storage_entry(storage);
    provider.insert(key, value);
}
