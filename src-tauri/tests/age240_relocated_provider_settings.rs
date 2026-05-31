#[path = "age240_relocated_support.rs"]
mod support;

#[test]
fn provider_settings_command_args_deserialize_camel_case_ipc_payloads() {
    support::provider_settings_command_args_deserialize_camel_case_ipc_payloads();
}

#[test]
fn provider_settings_command_preserves_structured_conflict_and_transport_errors() {
    support::provider_settings_command_preserves_structured_conflict_and_transport_errors();
}

#[cfg(unix)]
#[test]
fn provider_settings_command_preserves_migration_diagnostics_from_real_host() {
    support::provider_settings_command_preserves_migration_diagnostics_from_real_host();
}

#[cfg(unix)]
#[test]
fn provider_settings_targets_skip_central_config_only_models() {
    support::provider_settings_targets_skip_central_config_only_models();
}

#[cfg(unix)]
#[test]
fn provider_settings_migration_packages_central_config_blocks_read_only() {
    support::provider_settings_migration_packages_central_config_blocks_read_only();
}
