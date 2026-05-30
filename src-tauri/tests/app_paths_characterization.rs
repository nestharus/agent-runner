use agent_runner_lib::load_providers_for_models_dir;

#[test]
fn load_providers_for_models_dir_loads_parent_providers_and_defaults_errors() {
    let dir = tempfile::tempdir().unwrap();
    let models_dir = dir.path().join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    std::fs::write(
        dir.path().join("providers.toml"),
        r#"
[codex]
command = "codex"
args = ["exec"]
"#,
    )
    .unwrap();

    let providers = load_providers_for_models_dir(&models_dir);
    assert!(providers.entries.contains_key("codex"));

    std::fs::write(dir.path().join("providers.toml"), "not = [valid").unwrap();
    let providers = load_providers_for_models_dir(&models_dir);
    assert!(providers.entries.is_empty());

    std::fs::remove_file(dir.path().join("providers.toml")).unwrap();
    let providers = load_providers_for_models_dir(&models_dir);
    assert!(providers.entries.is_empty());
}
