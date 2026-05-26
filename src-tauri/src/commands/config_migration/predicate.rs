use super::mapper::ProviderRuntimeParts;
use std::path::Path;

pub(crate) fn path_exists(path: &Path) -> bool {
    path.exists()
}

pub(crate) fn is_toml_path(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "toml")
}

pub(crate) fn removed_global_prompt_mode(global_prompt_mode: &Option<toml::Value>) -> bool {
    global_prompt_mode.is_some()
}

pub(crate) fn has_old_top_level_command(table: &toml::Table) -> bool {
    table.contains_key("command")
}

pub(crate) fn provider_has_runtime_blocks(provider: &toml::Table) -> bool {
    provider.contains_key("command")
        || provider.contains_key("resume")
        || provider.contains_key("session_capture")
        || provider.contains_key("session_storage")
        || provider.contains_key("resume_acceptance")
        || provider.contains_key("prompt_mode")
}

pub(crate) fn should_keep_model_only_provider(
    has_runtime_blocks: bool,
    provider_name: Option<&str>,
    providers_root: &toml::Table,
) -> bool {
    !has_runtime_blocks
        && provider_name
            .and_then(|name| providers_root.get(name))
            .is_none()
}

pub(crate) fn runtime_parts_has_interactive_args(runtime_parts: &ProviderRuntimeParts) -> bool {
    runtime_parts
        .runtime_interactive_args
        .as_ref()
        .is_some_and(|args| !args.is_empty())
}

pub(crate) fn should_repair_empty_array(
    existing: Option<&toml::Value>,
    value: &toml::Value,
) -> bool {
    matches!(existing, Some(toml::Value::Array(existing)) if existing.is_empty())
        && !matches!(value, toml::Value::Array(value) if value.is_empty())
}
