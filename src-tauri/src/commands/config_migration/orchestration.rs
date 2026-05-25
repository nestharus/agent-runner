use super::accessor::{
    model_toml_paths, read_optional_toml_table, read_toml_table, runtime_provider_table,
    write_changed_providers_toml, write_text_file,
};
use super::filter::{
    combined_runtime_interactive_args, count_runtime_provider_tables, set_or_conflict,
    set_or_repair_empty_array, take_global_prompt_mode,
};
use super::formatter::format_moved_runtime_block;
use super::mapper::{
    ProviderRuntimeBlocks, ProviderRuntimeParts, old_top_level_provider_table,
    provider_migration_draft, provider_runtime_block_entries, provider_runtime_parts,
    reduced_provider_value, session_storage_from_entry, string_array_value,
    take_provider_prompt_mode, take_provider_runtime_blocks,
};
use super::parser::serialize_toml_table;
use super::predicate::{
    has_old_top_level_command, removed_global_prompt_mode, runtime_parts_has_interactive_args,
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

    if let Some(config_root) = providers_path.parent() {
        backfill_session_storage_from_sessions(
            &mut providers_root,
            &config_root.join("sessions.toml"),
            &mut moved_blocks,
        )?;
    }

    let providers_touched = count_runtime_provider_tables(&providers_root);
    write_changed_providers_toml(providers_path, &providers_root)?;
    oulipoly_config::migrate_legacy_session_storage_file(providers_path)?;

    Ok(ConfigMigrationReport {
        providers_touched,
        model_files_rewritten: rewritten,
        moved_blocks,
    })
}

pub(crate) fn migrate_model_config_file(
    path: &Path,
    providers_root: &mut toml::Table,
    moved_blocks: &mut Vec<String>,
) -> Result<usize, String> {
    let mut table = read_toml_table(path)?;
    let before = serialize_toml_table(path, &table)?;
    let changed = migrate_model_config_table(path, &mut table, providers_root, moved_blocks)?;
    let after = serialize_toml_table(path, &table)?;
    if changed && after != before {
        write_text_file(path, after)?;
        Ok(1)
    } else {
        Ok(0)
    }
}

pub(crate) fn migrate_model_config_table(
    path: &Path,
    table: &mut toml::Table,
    providers_root: &mut toml::Table,
    moved_blocks: &mut Vec<String>,
) -> Result<bool, String> {
    let mut changed = false;
    let global_prompt_mode = take_global_prompt_mode(table);
    changed |= removed_global_prompt_mode(&global_prompt_mode);

    if has_old_top_level_command(table) {
        let provider_table = old_top_level_provider_table(table)?;
        let migrated = migrate_provider_table(
            provider_table,
            global_prompt_mode,
            providers_root,
            path,
            moved_blocks,
        )?;
        table.insert("providers".to_string(), toml::Value::Array(vec![migrated]));
        return Ok(true);
    }

    changed |= migrate_provider_array(
        path,
        table,
        global_prompt_mode,
        providers_root,
        moved_blocks,
    )?;
    Ok(changed)
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
    if migrated != *provider {
        *provider = migrated;
        Ok(true)
    } else {
        Ok(false)
    }
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
        return Ok(toml::Value::Table(draft.original_provider));
    }
    let provider_name = validate_migrated_provider_name(draft.provider_name, path)?;
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

    Ok(reduced_provider_value(
        provider_name,
        runtime_parts.model_args,
        runtime_parts.model_interactive_args,
    ))
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
    if let Some(runtime_command) = &runtime_parts.runtime_command {
        set_or_repair_empty_array(
            runtime,
            "command",
            toml::Value::String(runtime_command.clone()),
            provider_name,
            path,
        )?;
    }
    Ok(())
}

pub(crate) fn apply_runtime_args(
    runtime: &mut toml::Table,
    has_runtime_blocks: bool,
    runtime_parts: &ProviderRuntimeParts,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if has_runtime_blocks || !runtime_parts.runtime_args.is_empty() {
        set_or_repair_empty_array(
            runtime,
            "args",
            string_array_value(runtime_parts.runtime_args.clone()),
            provider_name,
            path,
        )?;
    }
    Ok(())
}

pub(crate) fn apply_runtime_interactive_args(
    runtime: &mut toml::Table,
    has_runtime_blocks: bool,
    runtime_parts: &ProviderRuntimeParts,
    provider_name: &str,
    path: &Path,
) -> Result<(), String> {
    if has_runtime_blocks || runtime_parts_has_interactive_args(runtime_parts) {
        set_or_repair_empty_array(
            runtime,
            "interactive_args",
            string_array_value(combined_runtime_interactive_args(runtime_parts)),
            provider_name,
            path,
        )?;
    }
    Ok(())
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
        moved_blocks.push(format_moved_runtime_block(path, key, provider_name));
    }
    Ok(())
}

pub(crate) fn backfill_session_storage_from_sessions(
    providers_root: &mut toml::Table,
    sessions_path: &Path,
    moved_blocks: &mut Vec<String>,
) -> Result<(), String> {
    if !sessions_path.exists() {
        return Ok(());
    }
    let sessions = read_toml_table(sessions_path)?;

    for (provider_name, entry) in sessions {
        let Some(storage) = session_storage_from_entry(&entry) else {
            continue;
        };
        let Some(provider) = providers_root
            .get_mut(&provider_name)
            .and_then(toml::Value::as_table_mut)
        else {
            continue;
        };
        if provider.contains_key("session_storage") {
            continue;
        }
        provider.insert("session_storage".to_string(), toml::Value::Table(storage));
        moved_blocks.push(format!(
            "{}[{provider_name}].turn_script -> providers.toml[{provider_name}].session_storage",
            sessions_path.display()
        ));
    }
    Ok(())
}
