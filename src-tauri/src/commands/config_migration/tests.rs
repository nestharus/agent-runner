//! ## Declared roles
//!
//! `validator`, `accessor`, `parser`, `mapper`, `orchestration`

mod tests {
    use crate::commands::config_migration::orchestration::migrate_config_files;
    use std::path::Path;

    fn toml_array_strings(table: &toml::Table, key: &str) -> Vec<String> {
        table
            .get(key)
            .and_then(toml::Value::as_array)
            .unwrap_or_else(|| panic!("missing array key {key} in {table:?}"))
            .iter()
            .map(|value| value.as_str().unwrap().to_string())
            .collect()
    }

    fn migrated_model_provider(path: &Path) -> toml::Table {
        let table = parsed_toml_file(path);
        table["providers"]
            .as_array()
            .unwrap()
            .first()
            .unwrap()
            .as_table()
            .unwrap()
            .clone()
    }

    fn migrated_runtime_provider(path: &Path, provider: &str) -> toml::Table {
        let table = parsed_toml_file(path);
        table[provider].as_table().unwrap().clone()
    }

    fn parsed_toml_file(path: &Path) -> toml::Table {
        parse_toml_text(&read_toml_file(path))
    }

    fn read_toml_file(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    fn parse_toml_text(text: &str) -> toml::Table {
        text.parse::<toml::Table>().unwrap()
    }

    #[test]
    fn migrate_config_lifts_per_provider_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        std::fs::write(
            models_dir.join("claude-opus.toml"),
            r#"
prompt_mode = "stdin"

[[providers]]
name = "claude2"
command = "env -u CLAUDECODE claude2"
args = ["-p", "--model", "opus", "--output-format", "json"]
interactive_args = ["--model", "opus"]

[providers.resume]
kind = "flag"
flag = "--resume"

[providers.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"

[providers.session_storage]
kind = "claude_code"
projects_dir = "/tmp/claude2/projects"

[providers.resume_acceptance]
accepted_output_patterns = ["\"session_id\":\"{session_id}\""]
"#,
        )
        .unwrap();

        let report = migrate_config_files(&models_dir, &providers_path).unwrap();

        assert_eq!(report.model_files_rewritten, 1);
        let model = std::fs::read_to_string(models_dir.join("claude-opus.toml")).unwrap();
        assert!(model.contains("name = \"claude2\""), "{model}");
        assert!(model.contains("\"--model\""), "{model}");
        assert!(model.contains("\"opus\""), "{model}");
        assert!(!model.contains("\"--output-format\""), "{model}");
        assert!(!model.contains("command ="), "{model}");
        assert!(!model.contains("session_storage"), "{model}");

        let providers = std::fs::read_to_string(&providers_path).unwrap();
        assert!(providers.contains("[claude2]"), "{providers}");
        assert!(providers.contains("command = \"env\""), "{providers}");
        assert!(providers.contains("\"-u\""), "{providers}");
        assert!(providers.contains("\"CLAUDECODE\""), "{providers}");
        assert!(providers.contains("\"claude2\""), "{providers}");
        assert!(providers.contains("[claude2.resume]"), "{providers}");
        assert!(
            providers.contains("[claude2.session_storage]"),
            "{providers}"
        );
        assert!(providers.contains("\"--output-format\""), "{providers}");
    }

    #[test]
    fn migrate_config_backfills_session_storage_from_turn_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let sessions_path = dir.path().join("sessions.toml");
        std::fs::write(
            models_dir.join("claude-opus.toml"),
            r#"
[[providers]]
name = "claude"
args = ["--model", "opus"]
"#,
        )
        .unwrap();
        std::fs::write(
            &providers_path,
            r#"
[claude]
command = "claude"
args = ["-p"]
"#,
        )
        .unwrap();
        std::fs::write(
            &sessions_path,
            r#"
[claude]
turn_script = "claude-code-turns ~/.claude/projects"
"#,
        )
        .unwrap();

        let report = migrate_config_files(&models_dir, &providers_path).unwrap();

        assert_eq!(report.model_files_rewritten, 0);
        assert!(
            report
                .moved_blocks
                .iter()
                .any(|block| block.contains("sessions.toml[claude].turn_script")),
            "{:?}",
            report.moved_blocks
        );
        let runtime = migrated_runtime_provider(&providers_path, "claude");
        let storage = runtime
            .get("session_storage")
            .and_then(toml::Value::as_table)
            .unwrap();
        assert_eq!(
            storage.get("kind").and_then(toml::Value::as_str),
            Some("script")
        );
        assert_eq!(
            storage.get("cwd_script").and_then(toml::Value::as_str),
            Some("claude-code-cwd ~/.claude/projects")
        );
        assert_eq!(
            storage
                .get("transcript_script")
                .and_then(toml::Value::as_str),
            Some("claude-code-locate-transcript ~/.claude/projects")
        );
        assert_eq!(
            storage.get("storage_type").and_then(toml::Value::as_str),
            Some("claude_code")
        );
    }

    #[test]
    fn migrate_config_keeps_model_only_interactive_args_out_of_provider_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let model_path = models_dir.join("claude-haiku.toml");
        std::fs::write(
            &providers_path,
            r#"
[claude]
command = "claude"
args = ["-p", "--output-format", "json"]
interactive_args = ["--dangerously-skip-permissions"]
"#,
        )
        .unwrap();
        std::fs::write(
            &model_path,
            r#"
[[providers]]
name = "claude"
args = ["--model", "haiku"]
interactive_args = ["--model", "haiku"]
"#,
        )
        .unwrap();

        let report = migrate_config_files(&models_dir, &providers_path).unwrap();

        assert_eq!(report.model_files_rewritten, 0);
        let runtime = migrated_runtime_provider(&providers_path, "claude");
        assert_eq!(
            toml_array_strings(&runtime, "interactive_args"),
            ["--dangerously-skip-permissions"]
        );
        let model = migrated_model_provider(&model_path);
        assert_eq!(
            toml_array_strings(&model, "interactive_args"),
            ["--model", "haiku"]
        );
    }

    #[test]
    fn migrate_config_lifts_runtime_args_strips_model_flags() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let model_path = models_dir.join("claude-opus.toml");
        std::fs::write(
            &model_path,
            r#"
[[providers]]
name = "claude"
command = "env"
args = ["-u", "CLAUDECODE", "claude", "-p", "--model", "opus", "--dangerously-skip-permissions"]
"#,
        )
        .unwrap();

        migrate_config_files(&models_dir, &providers_path).unwrap();

        let runtime = migrated_runtime_provider(&providers_path, "claude");
        assert_eq!(
            toml_array_strings(&runtime, "args"),
            [
                "-u",
                "CLAUDECODE",
                "claude",
                "-p",
                "--dangerously-skip-permissions"
            ]
        );
        let model = migrated_model_provider(&model_path);
        assert_eq!(toml_array_strings(&model, "args"), ["--model", "opus"]);
    }

    #[test]
    fn migrate_config_strips_dash_m_pairs() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let model_path = models_dir.join("gpt-high.toml");
        std::fs::write(
            &model_path,
            r#"
[[providers]]
name = "codex"
command = "codex"
args = ["exec", "--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-5.5"]
"#,
        )
        .unwrap();

        migrate_config_files(&models_dir, &providers_path).unwrap();

        let runtime = migrated_runtime_provider(&providers_path, "codex");
        assert_eq!(
            toml_array_strings(&runtime, "args"),
            ["exec", "--dangerously-bypass-approvals-and-sandbox"]
        );
        let model = migrated_model_provider(&model_path);
        assert_eq!(toml_array_strings(&model, "args"), ["-m", "gpt-5.5"]);
    }

    #[test]
    fn migrate_config_strips_model_prefixed_c_keys() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let model_path = models_dir.join("gpt-high.toml");
        std::fs::write(
            &model_path,
            r#"
[[providers]]
name = "codex"
command = "codex"
args = ["exec", "-c", "model_reasoning_effort=high", "-c", "sandbox=workspace-write"]
"#,
        )
        .unwrap();

        migrate_config_files(&models_dir, &providers_path).unwrap();

        let runtime = migrated_runtime_provider(&providers_path, "codex");
        assert_eq!(
            toml_array_strings(&runtime, "args"),
            ["exec", "-c", "sandbox=workspace-write"]
        );
        let model = migrated_model_provider(&model_path);
        assert_eq!(
            toml_array_strings(&model, "args"),
            ["-c", "model_reasoning_effort=high"]
        );
    }

    #[test]
    fn migrate_config_strips_interactive_args_same_filter() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let model_path = models_dir.join("gpt-high.toml");
        std::fs::write(
            &model_path,
            r#"
[[providers]]
name = "codex"
command = "codex"
args = ["exec", "-m", "gpt-5.5"]
interactive_args = ["exec", "-c", "model_reasoning_effort=high", "-c", "sandbox=workspace-write"]
"#,
        )
        .unwrap();

        migrate_config_files(&models_dir, &providers_path).unwrap();

        let runtime = migrated_runtime_provider(&providers_path, "codex");
        assert_eq!(toml_array_strings(&runtime, "args"), ["exec"]);
        assert_eq!(
            toml_array_strings(&runtime, "interactive_args"),
            ["exec", "-c", "sandbox=workspace-write"]
        );
        let model = migrated_model_provider(&model_path);
        assert_eq!(toml_array_strings(&model, "args"), ["-m", "gpt-5.5"]);
        assert_eq!(
            toml_array_strings(&model, "interactive_args"),
            ["-c", "model_reasoning_effort=high"]
        );
    }

    #[test]
    fn migrate_config_aborts_on_conflicting_runtime_args() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        for (name, env_name) in [("a", "CLAUDECODE"), ("b", "CLAUDE_CONFIG_DIR")] {
            std::fs::write(
                models_dir.join(format!("{name}.toml")),
                format!(
                    r#"
[[providers]]
name = "claude"
command = "env"
args = ["-u", "{env_name}", "claude", "-p", "--model", "opus"]
"#
                ),
            )
            .unwrap();
        }

        let err = migrate_config_files(&models_dir, &providers_path).unwrap_err();

        assert!(
            err.contains("conflicting args for provider claude"),
            "{err}"
        );
    }

    #[test]
    fn migrate_config_idempotent_after_proper_lift() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let model_path = models_dir.join("claude-opus.toml");
        std::fs::write(
            &model_path,
            r#"
[[providers]]
name = "claude"
command = "env"
args = ["-u", "CLAUDECODE", "claude", "-p", "--model", "opus"]
"#,
        )
        .unwrap();

        migrate_config_files(&models_dir, &providers_path).unwrap();
        let model_after_first = std::fs::read_to_string(&model_path).unwrap();
        let providers_after_first = std::fs::read_to_string(&providers_path).unwrap();
        let second = migrate_config_files(&models_dir, &providers_path).unwrap();

        assert_eq!(second.model_files_rewritten, 0);
        assert_eq!(
            model_after_first,
            std::fs::read_to_string(&model_path).unwrap()
        );
        assert_eq!(
            providers_after_first,
            std::fs::read_to_string(&providers_path).unwrap()
        );
    }

    #[test]
    fn migrate_config_repairs_prior_empty_runtime_args() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        let model_path = models_dir.join("claude-opus.toml");
        std::fs::write(
            &providers_path,
            r#"
[claude]
command = "env"
args = []
interactive_args = []

[claude.resume]
kind = "flag"
flag = "--resume"
"#,
        )
        .unwrap();
        std::fs::write(
            &model_path,
            r#"
[[providers]]
name = "claude"
args = ["-u", "CLAUDECODE", "claude", "-p", "--model", "opus"]
interactive_args = ["-u", "CLAUDECODE", "claude", "--model", "opus"]
"#,
        )
        .unwrap();

        migrate_config_files(&models_dir, &providers_path).unwrap();

        let runtime = migrated_runtime_provider(&providers_path, "claude");
        assert_eq!(
            toml_array_strings(&runtime, "args"),
            ["-u", "CLAUDECODE", "claude", "-p"]
        );
        assert_eq!(
            toml_array_strings(&runtime, "interactive_args"),
            ["-u", "CLAUDECODE", "claude"]
        );
        let model = migrated_model_provider(&model_path);
        assert_eq!(toml_array_strings(&model, "args"), ["--model", "opus"]);
        assert_eq!(
            toml_array_strings(&model, "interactive_args"),
            ["--model", "opus"]
        );
    }

    #[test]
    fn migrate_config_aborts_on_conflicting_command() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        for (name, command) in [("a", "claude"), ("b", "claude-other")] {
            std::fs::write(
                models_dir.join(format!("{name}.toml")),
                format!(
                    r#"
[[providers]]
name = "p"
command = "{command}"
"#
                ),
            )
            .unwrap();
        }

        let err = migrate_config_files(&models_dir, &providers_path).unwrap_err();

        assert!(err.contains("conflicting command for provider p"), "{err}");
    }

    #[test]
    fn migrate_config_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let providers_path = dir.path().join("providers.toml");
        std::fs::write(
            models_dir.join("claude-opus.toml"),
            r#"
[[providers]]
name = "claude"
command = "claude"
args = ["-p", "--model", "opus"]

[providers.resume]
kind = "flag"
flag = "--resume"
"#,
        )
        .unwrap();

        let first = migrate_config_files(&models_dir, &providers_path).unwrap();
        let model_after_first =
            std::fs::read_to_string(models_dir.join("claude-opus.toml")).unwrap();
        let providers_after_first = std::fs::read_to_string(&providers_path).unwrap();
        let second = migrate_config_files(&models_dir, &providers_path).unwrap();

        assert_eq!(first.model_files_rewritten, 1);
        assert_eq!(second.model_files_rewritten, 0);
        assert_eq!(
            model_after_first,
            std::fs::read_to_string(models_dir.join("claude-opus.toml")).unwrap()
        );
        assert_eq!(
            providers_after_first,
            std::fs::read_to_string(&providers_path).unwrap()
        );
    }
}
