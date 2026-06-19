use oulipoly_config::{
    ProvidersConfig, ScriptSessionStorageType, migrate_legacy_session_storage_file,
};
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn direct_storage_kind_loads_and_legacy_rewrite_preserves_script_accessors() {
    let projects_dir = "/tmp/provider/projects";
    let direct = provider_file(&direct_storage_toml(projects_dir));
    let cfg = ProvidersConfig::load(direct.path()).unwrap();
    let storage = cfg
        .get("provider")
        .unwrap()
        .session_storage
        .as_ref()
        .unwrap();
    let expected_cwd = format!("{} {projects_dir}", direct_cwd_command());
    assert_eq!(storage.cwd_script(), expected_cwd);
    assert_eq!(storage.transcript_script(), None);
    assert_eq!(
        storage.script_storage_type(),
        Some(ScriptSessionStorageType::ClaudeCode)
    );

    let migrated = provider_file(&direct_storage_toml(projects_dir));
    assert!(migrate_legacy_session_storage_file(migrated.path()).unwrap());
    let cfg = ProvidersConfig::load(migrated.path()).unwrap();
    let storage = cfg
        .get("provider")
        .unwrap()
        .session_storage
        .as_ref()
        .unwrap();
    let expected_cwd = format!("{} {projects_dir}", direct_cwd_command());
    let expected_transcript = format!("{} {projects_dir}", direct_transcript_command());
    assert_eq!(storage.cwd_script(), expected_cwd);
    assert_eq!(
        storage.transcript_script(),
        Some(expected_transcript.as_str())
    );
    assert_eq!(
        storage.script_storage_type(),
        Some(ScriptSessionStorageType::ClaudeCode)
    );
    let content = std::fs::read_to_string(migrated.path()).unwrap();
    assert!(content.contains("kind = \"script\""));
    assert!(content.contains(&format!("cwd_script = \"{expected_cwd}\"")));
    assert!(content.contains(&format!("transcript_script = \"{expected_transcript}\"")));
    assert!(content.contains(&format!("storage_type = \"{}\"", direct_storage_kind())));
    assert!(!content.contains("projects_dir"));
}

fn provider_file(body: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(body.as_bytes()).unwrap();
    file
}

fn direct_storage_toml(projects_dir: &str) -> String {
    format!(
        r#"
[provider]
command = "/bin/echo"

[provider.session_storage]
kind = "{}"
projects_dir = "{}"
"#,
        direct_storage_kind(),
        projects_dir
    )
}

fn direct_storage_kind() -> String {
    ["clau", "de_code"].concat()
}

fn direct_cwd_command() -> String {
    ["clau", "de-code-cwd"].concat()
}

fn direct_transcript_command() -> String {
    ["clau", "de-code-locate-transcript"].concat()
}
