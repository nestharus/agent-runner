use super::*;
use crate::model::{
    ClaudeRestrictions, CodexRestrictions, InvocationMode, PromptMode, ProviderConfig,
    ToolRestrictionKind, ToolRestrictions, derive_provider_name,
};
use std::io::Write;
use std::path::Path;

fn function_body<'a>(source: &'a str, needle: &str) -> &'a str {
    let start = source.find(needle).expect("function signature exists");
    let brace_start = source[start..].find('{').expect("function body starts") + start;
    let mut depth = 0usize;
    for (offset, ch) in source[brace_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[brace_start + 1..brace_start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("function body ends");
}

fn assert_contains_in_order(body: &str, expected: &[&str]) {
    let mut cursor = 0usize;
    for token in expected {
        let found = body[cursor..]
            .find(token)
            .unwrap_or_else(|| panic!("missing orchestration call {token} in {body}"));
        cursor += found + token.len();
    }
}

fn assert_forbidden_absent(body: &str, forbidden: &[&str]) {
    for token in forbidden {
        assert!(
            !body.contains(token),
            "orchestration shell must not contain inline logic token {token}: {body}"
        );
    }
}

#[test]
fn providers_config_load_orchestrates_parse_default_validate_construct() {
    let body = function_body(
        include_str!("config.rs"),
        "pub fn load(path: &Path) -> Result<Self, LoadError>",
    );

    assert_contains_in_order(
        body,
        &[
            "read_optional_providers_file",
            "parse_providers_toml",
            "apply_defaults_to_raw_providers",
            "validate_providers_config",
            "Self::from_validated_raw",
        ],
    );
    assert_forbidden_absent(
        body,
        &[
            "fs::read_to_string",
            "toml::from_str",
            "ProviderEntry {",
            "parse_prompt_mode",
            "InvocationMode::",
            "entry.validate",
        ],
    );
}

#[test]
fn parses_quota_scripts() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[claude]
quota_script = "anthropic-usage ~/.claude/.credentials.json"

[claude2]
quota_script = "anthropic-usage ~/.claude2/.credentials.json"
"#
    )
    .unwrap();
    let cfg = ProvidersConfig::load(f.path()).unwrap();
    assert_eq!(cfg.entries.len(), 2);
    assert!(
        cfg.get("claude")
            .unwrap()
            .quota_script
            .as_deref()
            .unwrap()
            .contains("anthropic-usage")
    );
}

#[test]
fn parses_auth_refresh_command() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[claude]
quota_script         = "anthropic-usage ~/.claude/.credentials.json"
auth_refresh_command = "claude auth status"

[claude2]
quota_script = "anthropic-usage ~/.claude2/.credentials.json"
"#
    )
    .unwrap();
    let cfg = ProvidersConfig::load(f.path()).unwrap();
    assert_eq!(
        cfg.get("claude").unwrap().auth_refresh_command.as_deref(),
        Some("claude auth status")
    );
    assert!(cfg.get("claude2").unwrap().auth_refresh_command.is_none());
}

#[test]
fn parses_runtime_provider_config() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[claude2]
command = "env"
args = ["-u", "CLAUDECODE", "claude2"]
interactive_args = ["-u", "CLAUDECODE", "claude2"]
prompt_mode = "stdin"

[claude2.resume]
kind = "flag"
flag = "--resume"

[claude2.session_storage]
kind = "claude_code"
projects_dir = "/tmp/claude2/projects"
"#
    )
    .unwrap();
    let cfg = ProvidersConfig::load(f.path()).unwrap();
    let model_provider = ProviderConfig {
        name: "claude2".to_string(),
        command: String::new(),
        args: vec!["--model".to_string(), "opus".to_string()],
        interactive_args: Some(vec!["--model".to_string(), "opus".to_string()]),
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    };
    let (provider, prompt_mode) = cfg.effective_provider(&model_provider).unwrap();
    assert_eq!(prompt_mode, PromptMode::Stdin);
    assert_eq!(provider.command, "env");
    assert_eq!(
        provider.args,
        ["-u", "CLAUDECODE", "claude2", "--model", "opus"]
    );
    assert_eq!(
        provider.interactive_args.as_deref(),
        Some(
            &[
                "-u".to_string(),
                "CLAUDECODE".to_string(),
                "claude2".to_string(),
                "--model".to_string(),
                "opus".to_string(),
            ][..]
        )
    );
    assert!(provider.resume.is_some());
    assert!(provider.session_storage.is_some());
}

#[test]
fn providers_toml_parses_age28_policy_fields() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[claude]
command = "env -u CLAUDECODE claude"
args = ["-p"]
system_prompt_override = """
Do not use the Task tool.
Use agents -m <model> -f <prompt-file>.
"""

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
disallowed_tools = ["Task", "Task tool"]
disable_slash_commands = true
"#
    )
    .unwrap();

    let cfg = ProvidersConfig::load(f.path()).unwrap();
    let entry = cfg.get("claude").unwrap();

    assert_eq!(
        entry.system_prompt_override.as_deref(),
        Some("Do not use the Task tool.\nUse agents -m <model> -f <prompt-file>.\n")
    );
    assert_eq!(
        entry.tool_restrictions,
        Some(ToolRestrictions {
            kind: ToolRestrictionKind::Claude,
            claude: ClaudeRestrictions {
                disallowed_tools: vec!["Task".to_string(), "Task tool".to_string()],
                allowed_tools: Vec::new(),
                disable_slash_commands: true,
            },
            codex: CodexRestrictions::default(),
        })
    );
}

#[test]
fn providers_toml_age28_fields_default_to_none() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[claude]
command = "claude"
args = ["-p"]
"#
    )
    .unwrap();

    let cfg = ProvidersConfig::load(f.path()).unwrap();
    let entry = cfg.get("claude").unwrap();

    assert!(entry.system_prompt_override.is_none());
    assert!(entry.tool_restrictions.is_none());
}

#[test]
fn missing_file_is_empty_config() {
    let cfg = ProvidersConfig::load(Path::new("/nonexistent/path/providers.toml")).unwrap();
    assert!(cfg.entries.is_empty());
}

#[test]
fn effective_provider_missing_provider_returns_named_error() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[other-provider]
command = "other"
"#
    )
    .unwrap();
    let cfg = ProvidersConfig::load(f.path()).unwrap();
    let model_provider = ProviderConfig::model_provider("missing-provider", vec![]);

    let err = cfg.effective_provider(&model_provider).unwrap_err();

    assert!(
        err.contains("provider missing-provider is missing from providers.toml"),
        "{err}"
    );
}

#[test]
fn effective_provider_carries_runtime_fields() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[provider]
command = "/bin/echo"
args = ["--provider"]
interactive_args = ["interactive"]
prompt_mode = "arg"

[provider.resume]
kind = "flag"
flag = "--resume"

[provider.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"

[provider.resume_acceptance]
accepted_output_patterns = ["accepted"]
rejected_output_patterns = ["rejected"]

[provider.session_storage]
kind = "script"
cwd_script = "codex-cwd /tmp/codex-sessions"
"#
    )
    .unwrap();
    let cfg = ProvidersConfig::load(f.path()).unwrap();
    let model_provider = ProviderConfig {
        name: "provider".to_string(),
        command: String::new(),
        args: vec!["--model".to_string()],
        interactive_args: Some(vec!["interactive-model".to_string()]),
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    };

    let (provider, prompt_mode) = cfg.effective_provider(&model_provider).unwrap();

    assert_eq!(prompt_mode, PromptMode::Arg);
    assert_eq!(provider.name, "provider");
    assert_eq!(provider.command, "/bin/echo");
    assert_eq!(provider.args, ["--provider", "--model"]);
    assert_eq!(
        provider.interactive_args.as_deref(),
        Some(&["interactive".to_string(), "interactive-model".to_string()][..])
    );
    assert_eq!(
        provider.resume.as_ref().unwrap().flag.as_deref(),
        Some("--resume")
    );
    assert_eq!(
        provider.session_capture.as_ref().unwrap().flag.as_deref(),
        Some("--session-id")
    );
    assert_eq!(
        provider
            .resume_acceptance
            .as_ref()
            .unwrap()
            .accepted_output_patterns
            .as_deref(),
        Some(&["accepted".to_string()][..])
    );
    assert!(provider.session_storage.is_some());
}

#[test]
fn parses_opencode_session_capture_json_args_for_all_accounts() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[opencode]
command = "opencode1"
prompt_mode = "arg"

[opencode.session_capture]
kind = "stdout_json_event"
json_args = ["--format", "json"]
event_type = "step_start"
event_id_path = "sessionID"

[opencode2]
command = "opencode2"
prompt_mode = "arg"

[opencode2.session_capture]
kind = "stdout_json_event"
json_args = ["--format", "json"]
event_type = "step_start"
event_id_path = "sessionID"

[opencode3]
command = "opencode3"
prompt_mode = "arg"

[opencode3.session_capture]
kind = "stdout_json_event"
json_args = ["--format", "json"]
event_type = "step_start"
event_id_path = "sessionID"

[opencode4]
command = "opencode4"
prompt_mode = "arg"

[opencode4.session_capture]
kind = "stdout_json_event"
json_args = ["--format", "json"]
event_type = "step_start"
event_id_path = "sessionID"

[opencode5]
command = "opencode5"
prompt_mode = "arg"

[opencode5.session_capture]
kind = "stdout_json_event"
json_args = ["--format", "json"]
event_type = "step_start"
event_id_path = "sessionID"

"#
    )
    .unwrap();

    let cfg = ProvidersConfig::load(f.path()).unwrap();

    for name in [
        "opencode",
        "opencode2",
        "opencode3",
        "opencode4",
        "opencode5",
    ] {
        let capture = cfg
            .entries
            .get(name)
            .and_then(|entry| entry.session_capture.as_ref())
            .unwrap_or_else(|| panic!("missing capture for {name}"));
        assert_eq!(
            capture.json_args.as_deref(),
            Some(&["--format".to_string(), "json".to_string()][..])
        );
        assert_eq!(capture.event_type.as_deref(), Some("step_start"));
        assert_eq!(capture.event_id_path.as_deref(), Some("sessionID"));
        assert!(capture.last_message_flag.is_none());
        assert!(
            cfg.entries
                .get(name)
                .and_then(|entry| entry.resume_acceptance.as_ref())
                .is_none(),
            "OpenCode resume_acceptance phrases remain disabled until live wording is verified"
        );
    }
}

#[test]
fn rejects_stdout_json_event_json_flag_without_last_message_flag() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[json-provider]
command = "json-provider"

[json-provider.session_capture]
kind = "stdout_json_event"
json_flag = "--json"
event_type = "agent.session_started"
event_id_path = "data.id"
"#
    )
    .unwrap();

    let err = ProvidersConfig::load(f.path()).unwrap_err();

    assert!(
            err.contains(
                "providers.toml provider json-provider: session_capture.kind = stdout_json_event requires `last_message_flag` when `json_flag` is set"
            ),
            "{err}"
        );
}

#[test]
fn rejects_stdout_json_event_json_args_with_last_message_flag() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[opencode]
command = "opencode"

[opencode.session_capture]
kind = "stdout_json_event"
json_args = ["--format", "json"]
last_message_flag = "--last-message"
event_type = "step_start"
event_id_path = "sessionID"
"#
    )
    .unwrap();

    let err = ProvidersConfig::load(f.path()).unwrap_err();

    assert!(
            err.contains(
                "providers.toml provider opencode: session_capture.kind = stdout_json_event does not allow `last_message_flag` with `json_args`"
            ),
            "{err}"
        );
}

#[test]
fn rejects_stdout_json_event_empty_json_args() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[opencode]
command = "opencode"

[opencode.session_capture]
kind = "stdout_json_event"
json_args = []
event_type = "step_start"
event_id_path = "sessionID"
"#
    )
    .unwrap();

    let err = ProvidersConfig::load(f.path()).unwrap_err();

    assert!(
            err.contains(
                "providers.toml provider opencode: session_capture.kind = stdout_json_event requires non-empty `json_args`"
            ),
            "{err}"
        );
}

#[test]
fn parses_script_session_storage() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[provider]
command = "/bin/echo"

[provider.session_storage]
kind = "script"
cwd_script = "fixture-cwd ~/.fixture/sessions"
"#
    )
    .unwrap();

    let migrated = migrate_legacy_session_storage_file(f.path()).unwrap();
    assert!(!migrated);
    let cfg = ProvidersConfig::load(f.path()).unwrap();

    let storage = cfg
        .get("provider")
        .unwrap()
        .session_storage
        .as_ref()
        .unwrap();
    assert_eq!(storage.cwd_script(), "fixture-cwd ~/.fixture/sessions");
    assert_eq!(storage.transcript_script(), None);
    assert_eq!(storage.script_storage_type(), None);
}

#[test]
fn migrates_legacy_claude_code_storage_to_script_storage() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[provider]
command = "/bin/echo"

[provider.session_storage]
kind = "claude_code"
projects_dir = "/tmp/provider/projects"
"#
    )
    .unwrap();

    let migrated = migrate_legacy_session_storage_file(f.path()).unwrap();
    let cfg = ProvidersConfig::load(f.path()).unwrap();

    let storage = cfg
        .get("provider")
        .unwrap()
        .session_storage
        .as_ref()
        .unwrap();
    assert_eq!(
        storage.cwd_script(),
        "claude-code-cwd /tmp/provider/projects"
    );
    assert_eq!(
        storage.transcript_script(),
        Some("claude-code-locate-transcript /tmp/provider/projects")
    );
    assert_eq!(
        storage.script_storage_type(),
        Some(crate::ScriptSessionStorageType::ClaudeCode)
    );
    assert!(migrated);
    let migrated_content = std::fs::read_to_string(f.path()).unwrap();
    assert!(migrated_content.contains("kind = \"script\""));
    assert!(migrated_content.contains("cwd_script = \"claude-code-cwd /tmp/provider/projects\""));
    assert!(
        migrated_content.contains(
            "transcript_script = \"claude-code-locate-transcript /tmp/provider/projects\""
        )
    );
    assert!(migrated_content.contains("storage_type = \"claude_code\""));
    assert!(!migrated_content.contains("projects_dir"));
}

#[test]
fn effective_provider_carries_age28_policy_fields() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[claude]
command = "env -u CLAUDECODE claude"
args = ["-p", "--provider-root"]
interactive_args = ["--provider-interactive"]
system_prompt_override = "root override"

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
disallowed_tools = ["Task"]
"#
    )
    .unwrap();
    let cfg = ProvidersConfig::load(f.path()).unwrap();
    let model_provider =
        ProviderConfig::model_provider("claude", vec!["--model".to_string(), "opus".to_string()]);

    let (provider, prompt_mode) = cfg.effective_provider(&model_provider).unwrap();

    assert_eq!(prompt_mode, PromptMode::Stdin);
    assert_eq!(provider.args, ["-p", "--provider-root", "--model", "opus"]);
    assert_eq!(
        provider.system_prompt_override.as_deref(),
        Some("root override")
    );
    assert_eq!(
        provider.tool_restrictions,
        Some(ToolRestrictions {
            kind: ToolRestrictionKind::Claude,
            claude: ClaudeRestrictions {
                disallowed_tools: vec!["Task".to_string()],
                allowed_tools: Vec::new(),
                disable_slash_commands: false,
            },
            codex: CodexRestrictions::default(),
        })
    );
}

#[test]
fn provider_validate_rejects_kind_mismatch() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[claude]
command = "env -u CLAUDECODE claude"
args = ["-p"]

[claude.tool_restrictions]
kind = "codex"
"#
    )
    .unwrap();

    let err = ProvidersConfig::load(f.path()).unwrap_err();

    assert!(err.contains("providers.toml provider claude"), "{err}");
    assert!(err.contains("kind"), "{err}");
    assert!(err.contains("claude"), "{err}");
    assert!(err.contains("codex"), "{err}");
}

#[test]
fn provider_validate_rejects_interactive_only_kind_mismatch() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[primary]
command = "env"
interactive_args = ["-u", "FOO", "codex", "exec"]

[primary.tool_restrictions]
kind = "claude"
"#
    )
    .unwrap();

    let err = ProvidersConfig::load(f.path()).unwrap_err();

    assert!(err.contains("providers.toml provider primary"), "{err}");
    assert!(err.contains("kind"), "{err}");
    assert!(err.contains("claude"), "{err}");
    assert!(err.contains("codex"), "{err}");
}

#[test]
fn provider_validate_rejects_duplicate_claude_tool_flag() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[claude]
command = "claude"
args = ["-p", "--allowedTools=Bash"]

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
allowed_tools = ["Bash"]
"#
    )
    .unwrap();

    let err = ProvidersConfig::load(f.path()).unwrap_err();

    assert!(err.contains("providers.toml provider claude"), "{err}");
    assert!(
        err.contains("tool_restrictions.claude.allowed_tools"),
        "{err}"
    );
    assert!(err.contains("Bash"), "{err}");
    assert!(err.contains("--allowedTools"), "{err}");
}

#[test]
fn provider_validate_rejects_duplicate_claude_tool_flag_in_command() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[claude]
command = "claude --allowed-tools Bash"
args = ["-p"]

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
allowed_tools = ["Bash"]
"#
    )
    .unwrap();

    let err = ProvidersConfig::load(f.path()).unwrap_err();

    assert!(err.contains("providers.toml provider claude"), "{err}");
    assert!(
        err.contains("tool_restrictions.claude.allowed_tools"),
        "{err}"
    );
    assert!(err.contains("command"), "{err}");
    assert!(err.contains("Bash"), "{err}");
    assert!(err.contains("--allowed-tools"), "{err}");
}

#[test]
fn provider_validate_rejects_mutually_exclusive_claude_tool_lists() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[claude]
command = "claude"

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
allowed_tools = ["Bash"]
disallowed_tools = ["Task"]
"#
    )
    .unwrap();

    let err = ProvidersConfig::load(f.path()).unwrap_err();

    assert!(err.contains("providers.toml provider claude"), "{err}");
    assert!(err.contains("mutually exclusive"), "{err}");
    assert!(
        err.contains("tool_restrictions.claude.allowed_tools"),
        "{err}"
    );
    assert!(
        err.contains("tool_restrictions.claude.disallowed_tools"),
        "{err}"
    );
}

#[test]
fn provider_validate_rejects_inactive_restriction_branch() {
    let mut claude_file = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        claude_file,
        r#"
[claude]
command = "claude"

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.codex]
disabled_features = ["web_search"]
"#
    )
    .unwrap();

    let err = ProvidersConfig::load(claude_file.path()).unwrap_err();

    assert!(err.contains("providers.toml provider claude"), "{err}");
    assert!(
        err.contains("tool_restrictions.codex must be empty"),
        "{err}"
    );

    let mut codex_file = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        codex_file,
        r#"
[codex]
command = "codex"

[codex.tool_restrictions]
kind = "codex"

[codex.tool_restrictions.claude]
disallowed_tools = ["Task"]
"#
    )
    .unwrap();

    let err = ProvidersConfig::load(codex_file.path()).unwrap_err();

    assert!(err.contains("providers.toml provider codex"), "{err}");
    assert!(
        err.contains("tool_restrictions.claude must be empty"),
        "{err}"
    );
}

#[test]
fn provider_validate_rejects_non_allowlisted_codex_config_key() {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        f,
        r#"
[codex]
command = "codex"
args = ["exec"]
prompt_mode = "arg"

[codex.tool_restrictions]
kind = "codex"

[codex.tool_restrictions.codex]
config_pairs = ["model_reasoning_effort=high"]
"#
    )
    .unwrap();

    let err = ProvidersConfig::load(f.path()).unwrap_err();

    assert!(err.contains("providers.toml provider codex"), "{err}");
    assert!(
        err.contains("tool_restrictions.codex.config_pairs"),
        "{err}"
    );
    assert!(err.contains("model_reasoning_effort"), "{err}");
    assert!(err.contains("no allowlisted Codex config pair"), "{err}");
}

#[test]
fn provider_validate_rejects_duplicate_codex_policy_flags() {
    let mut config_file = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        config_file,
        r#"
[codex]
command = "codex -c sandbox=workspace-write"

[codex.tool_restrictions]
kind = "codex"

[codex.tool_restrictions.codex]
config_pairs = ["sandbox=read-only"]
"#
    )
    .unwrap();

    let err = ProvidersConfig::load(config_file.path()).unwrap_err();

    assert!(err.contains("providers.toml provider codex"), "{err}");
    assert!(
        err.contains("tool_restrictions.codex.config_pairs"),
        "{err}"
    );
    assert!(err.contains("sandbox"), "{err}");
    assert!(err.contains("command"), "{err}");
    assert!(err.contains("-c"), "{err}");

    let mut feature_file = tempfile::NamedTempFile::new().unwrap();
    writeln!(
        feature_file,
        r#"
[codex]
command = "codex"
args = ["--disable", "web_search"]

[codex.tool_restrictions]
kind = "codex"

[codex.tool_restrictions.codex]
disabled_features = ["web_search"]
"#
    )
    .unwrap();

    let err = ProvidersConfig::load(feature_file.path()).unwrap_err();

    assert!(err.contains("providers.toml provider codex"), "{err}");
    assert!(
        err.contains("tool_restrictions.codex.disabled_features"),
        "{err}"
    );
    assert!(err.contains("web_search"), "{err}");
    assert!(err.contains("args"), "{err}");
    assert!(err.contains("--disable"), "{err}");
}

fn load_inline_providers(text: &str) -> Result<ProvidersConfig, LoadError> {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    write!(file, "{text}").unwrap();
    ProvidersConfig::load(file.path())
}

#[test]
fn providers_toml_defaults_invocation_mode_to_headless() {
    let cfg = load_inline_providers(
        r#"
[claude]
command = "claude"
"#,
    )
    .unwrap();

    let entry = cfg.get("claude").unwrap();
    assert_eq!(entry.invocation_mode, InvocationMode::Headless);
    let (runtime, _) = cfg.runtime_provider("claude").unwrap();
    assert_eq!(runtime.invocation_mode, InvocationMode::Headless);
}

#[test]
fn providers_toml_parses_explicit_proxy_invocation_mode() {
    let cfg = load_inline_providers(
        r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
    )
    .unwrap();

    assert_eq!(
        cfg.get("claude").unwrap().invocation_mode,
        InvocationMode::Proxy
    );
}

#[test]
fn providers_toml_rejects_unknown_invocation_mode_with_actionable_error() {
    let err = load_inline_providers(
        r#"
[claude]
command = "claude"
invocation_mode = "bogus"
"#,
    )
    .unwrap_err();

    match err {
        LoadError::InvocationMode { provider, value } => {
            assert_eq!(provider, "claude");
            assert_eq!(value, "bogus");
        }
        other => panic!("expected LoadError::InvocationMode, got {other:?}"),
    }
}

#[test]
fn proxy_claude_rejects_tools_mcp_filter_in_args() {
    let err = load_inline_providers(
        r#"
[claude]
command = "claude"
invocation_mode = "proxy"
args = ["--tools", "mcp__age104p2__Task"]
"#,
    )
    .unwrap_err();

    assert!(err.contains("proxy-mode Claude"), "{err}");
    assert!(err.contains("--tools mcp__"), "{err}");
}

#[test]
fn proxy_claude_rejects_tools_mcp_filter_in_interactive_args() {
    let err = load_inline_providers(
        r#"
[claude]
command = "claude"
invocation_mode = "proxy"
interactive_args = ["--tools", "mcp__age104p2__Task"]
"#,
    )
    .unwrap_err();

    assert!(err.contains("proxy-mode Claude"), "{err}");
    assert!(err.contains("--tools mcp__"), "{err}");
}

#[test]
fn proxy_claude_rejects_tools_mcp_filter_in_command() {
    let err = load_inline_providers(
        r#"
[claude]
command = "env -u CLAUDECODE claude --tools mcp__age104p2__Task"
invocation_mode = "proxy"
"#,
    )
    .unwrap_err();

    assert!(err.contains("proxy-mode Claude"), "{err}");
    assert!(err.contains("--tools mcp__"), "{err}");
}

#[test]
fn proxy_claude_accepts_allowed_tools_mcp_filter() {
    let cfg = load_inline_providers(
        r#"
[claude]
command = "claude"
invocation_mode = "proxy"
args = ["--allowedTools", "mcp__age104p2__Task"]
"#,
    )
    .unwrap();

    assert_eq!(
        cfg.get("claude").unwrap().invocation_mode,
        InvocationMode::Proxy
    );
}

#[test]
fn proxy_claude_accepts_allowed_tools_kebab_mcp_filter() {
    let cfg = load_inline_providers(
        r#"
[claude]
command = "claude"
invocation_mode = "proxy"
args = ["--allowed-tools", "mcp__age104p2__Task"]
"#,
    )
    .unwrap();

    assert_eq!(
        cfg.get("claude").unwrap().invocation_mode,
        InvocationMode::Proxy
    );
}

#[test]
fn proxy_claude_accepts_no_tool_filter() {
    let cfg = load_inline_providers(
        r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
    )
    .unwrap();

    assert_eq!(
        cfg.get("claude").unwrap().invocation_mode,
        InvocationMode::Proxy
    );
}

#[test]
fn effective_provider_carries_invocation_mode() {
    let cfg = load_inline_providers(
        r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
    )
    .unwrap();
    let model_provider = ProviderConfig::model_provider("claude", vec!["--model".into()]);

    let (provider, _) = cfg.effective_provider(&model_provider).unwrap();

    assert_eq!(provider.invocation_mode, InvocationMode::Proxy);
}

#[test]
fn runtime_provider_carries_invocation_mode() {
    let cfg = load_inline_providers(
        r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
    )
    .unwrap();

    let (provider, _) = cfg.runtime_provider("claude").unwrap();

    assert_eq!(provider.invocation_mode, InvocationMode::Proxy);
}

#[test]
fn providers_toml_prefixed_command_proxy_mode_preserves_provider_family_and_name() {
    let cfg = load_inline_providers(
        r#"
[claude]
command = "env -u CLAUDECODE claude"
invocation_mode = "proxy"
args = ["--allowedTools", "mcp__age104p2__Task"]
"#,
    )
    .unwrap();

    let (provider, _) = cfg.runtime_provider("claude").unwrap();

    assert_eq!(provider.name, "claude");
    assert_eq!(
        derive_provider_name(&provider.command, &provider.args),
        "claude"
    );
    assert_eq!(provider.invocation_mode, InvocationMode::Proxy);
}

#[test]
fn parse_providers_toml_returns_raw_struct_for_valid_text() {
    let raw = parse_providers_toml(
        r#"
[claude]
command = "claude"
"#,
    )
    .unwrap();

    assert!(raw.contains_key("claude"));
}

#[test]
fn parse_providers_toml_returns_err_for_malformed_text() {
    let err = parse_providers_toml("not = [").unwrap_err();

    match err {
        LoadError::Toml(message) => assert!(!message.is_empty()),
        other => panic!("expected LoadError::Toml, got {other:?}"),
    }
}

#[test]
fn apply_defaults_to_raw_providers_sets_headless_for_absent_mode() {
    let mut raw = RawProvidersToml::new();
    raw.insert(
        "claude".to_string(),
        RawEntry {
            quota_script: None,
            auth_refresh_command: None,
            command: Some("claude".to_string()),
            args: vec![],
            interactive_args: None,
            prompt_mode: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: None,
        },
    );

    let defaulted = apply_defaults_to_raw_providers(raw);

    assert_eq!(
        defaulted.get("claude").unwrap().invocation_mode.as_deref(),
        Some("headless")
    );
}

#[test]
fn apply_defaults_to_raw_providers_preserves_explicit_mode() {
    let raw = parse_providers_toml(
        r#"
[claude]
command = "claude"
invocation_mode = "proxy"
"#,
    )
    .unwrap();

    let defaulted = apply_defaults_to_raw_providers(raw);

    assert_eq!(
        defaulted.get("claude").unwrap().invocation_mode.as_deref(),
        Some("proxy")
    );
}

#[test]
fn validate_providers_config_passes_for_valid_input() {
    let raw = apply_defaults_to_raw_providers(
        parse_providers_toml(
            r#"
[claude]
command = "claude"
invocation_mode = "proxy"
args = ["--allowedTools", "mcp__age104p2__Task"]
"#,
        )
        .unwrap(),
    );

    assert_eq!(validate_providers_config(&raw), Ok(()));
}

#[test]
fn validate_providers_config_rejects_bad_invocation_mode() {
    let raw = parse_providers_toml(
        r#"
[claude]
command = "claude"
invocation_mode = "bogus"
"#,
    )
    .unwrap();

    let err = validate_providers_config(&raw).unwrap_err();

    match err {
        LoadError::InvocationMode { provider, value } => {
            assert_eq!(provider, "claude");
            assert_eq!(value, "bogus");
        }
        other => panic!("expected LoadError::InvocationMode, got {other:?}"),
    }
}
