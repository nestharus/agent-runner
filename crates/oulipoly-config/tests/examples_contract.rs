use oulipoly_config::{load_models, ProvidersConfig};
use std::fs;

#[test]
fn codex_resume_example_loads_with_canonical_codex_root_providers() {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("oulipoly-config crate should live under workspace crates/");
    let example_path = workspace_root
        .join("examples")
        .join("models")
        .join("codex-resume.toml");
    let example = fs::read_to_string(&example_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", example_path.display()));

    let temp = tempfile::tempdir().unwrap();
    let models_dir = temp.path().join("models");
    fs::create_dir_all(&models_dir).unwrap();
    fs::write(models_dir.join("codex-resume.toml"), example).unwrap();
    fs::write(
        temp.path().join("providers.toml"),
        r#"
[codex]
command = "codex"
args = ["exec", "-c", "sandbox=workspace-write"]
interactive_args = ["exec", "--dangerously-bypass-approvals-and-sandbox"]
"#,
    )
    .unwrap();

    let providers = ProvidersConfig::load(&temp.path().join("providers.toml")).unwrap();
    let models = load_models(&models_dir, Some(&providers)).unwrap();

    let model = models.get("codex-resume").expect("codex-resume model loads");
    assert_eq!(model.name, "codex-resume");
    assert_eq!(model.providers.len(), 1);
    assert_eq!(model.providers[0].name, "codex");
}
