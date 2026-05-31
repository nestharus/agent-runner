#[path = "age240_relocated_support.rs"]
mod support;

#[test]
fn open_state_db_opens_models_parent_state_db_and_returns_state_db() {
    support::open_state_db_opens_models_parent_state_db_and_returns_state_db();
}

#[cfg(unix)]
#[test]
fn test_model_migrated_provider_uses_providers_toml_effective_provider() {
    support::test_model_migrated_provider_uses_providers_toml_effective_provider();
}
