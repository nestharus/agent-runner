use oulipoly_config::{InvocationMode, ProvidersConfig};

#[test]
fn examples_load_default_policy_providers_fixture_remains_valid() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("oulipoly-config crate should live under workspace crates/");
    let fixture = workspace_root
        .join("tests")
        .join("fixtures")
        .join("age28-default-policy.providers.toml");

    let providers = ProvidersConfig::load(&fixture).unwrap();

    for entry in providers.entries.values() {
        assert_eq!(entry.invocation_mode, InvocationMode::Headless);
    }
}
