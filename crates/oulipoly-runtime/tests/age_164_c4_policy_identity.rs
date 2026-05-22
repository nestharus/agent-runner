//! AGE-164 cluster 4 — provider policy + provider identity (ACR-205 PP-002).
//!
//! Verifies the runtime-owned provider-identity intrinsic surface:
//!
//!   The bounded set of recognized providers is `claude`, `codex`, and the
//!   default `openai_compat` fallback. The recognition uses `provider.name`
//!   prefix (`starts_with("claude"|"codex")`) AND/OR the command executable
//!   token (after env-prefix/option skipping and basename normalization).
//!   These tests pin that bounded set without depending on private helper
//!   names — the verification is via observable policy-driven argv shape.
//!
//! Plus characterization tests for the public parsers `shell_split` and
//! `provider_name`. Plus the policy emission shape for Claude allowed/
//! disallowed tools and slash-command disabling.
//!
//! ## Declared roles
//!
//! Roles: formatter, mapper, parser, validator.
//!
//! - formatter: fixture argv-dump scripts and expected argv fragments.
//! - mapper: provider-command fixture construction for identity matrix cases.
//! - parser: argv sidecar line loading.
//! - validator: provider identity and provider policy assertions.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/tests/age_164_c4_policy_identity.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly-config-provider-policy-test-contract
//!       - executor-cli-provider-identity-public-api-contract
//!       - unix-fixture-argv-sidecar-contract
//! ```

#![cfg(unix)]

use oulipoly_config::{
    ClaudeRestrictions, CodexRestrictions, ModelConfig, PromptMode, ProviderConfig,
    ToolRestrictionKind, ToolRestrictions,
};
use oulipoly_runtime::executor::cli::{execute, provider_name, shell_split};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn argv_dump_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let argv_dump = dir.path().join("argv.txt");
    let script_path = dir.path().join("argv-dump.sh");
    std::fs::write(&script_path, argv_dump_script_body(&argv_dump)).unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();
    (dir, script_path, argv_dump)
}

fn argv_dump_script_body(argv_dump: &Path) -> String {
    format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$@\" > '{}'\n",
        argv_dump.display()
    )
}

fn fake_env_and_claude_fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let argv_dump = dir.path().join("argv.txt");
    let env_path = dir.path().join("env");
    let claude_path = dir.path().join("claude");
    std::fs::write(&env_path, fake_env_script_body()).unwrap();
    std::fs::write(&claude_path, argv_dump_script_body(&argv_dump)).unwrap();
    for path in [&env_path, &claude_path] {
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }
    (dir, env_path, claude_path, argv_dump)
}

fn fake_env_script_body() -> &'static str {
    r#"#!/usr/bin/env bash
set -euo pipefail
while [ "$#" -gt 0 ]; do
  case "$1" in
    -u|-e|-S)
      shift 2
      ;;
    --*)
      shift
      ;;
    *=*)
      shift
      ;;
    -*)
      shift
      ;;
    *)
      exec "$@"
      ;;
  esac
done
exit 0
"#
}

fn read_argv(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn claude_restricted_provider(command: String) -> ProviderConfig {
    let mut provider = ProviderConfig::new(command, Vec::new());
    provider.name = "abc".to_string();
    provider.tool_restrictions = Some(ToolRestrictions {
        kind: ToolRestrictionKind::Claude,
        claude: ClaudeRestrictions {
            disallowed_tools: vec!["Task".to_string()],
            allowed_tools: Vec::new(),
            disable_slash_commands: false,
        },
        codex: CodexRestrictions::default(),
    });
    provider
}

fn assert_command_is_identified_as_claude(command: String, argv_dump: &std::path::Path) {
    let model = claude_identity_model(command);

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);
    let argv = read_argv(argv_dump);
    assert!(
        argv.iter().any(|t| t == "--disallowed-tools"),
        "provider command must be identified as Claude; argv={argv:?}"
    );
}

fn claude_identity_model(command: String) -> ModelConfig {
    ModelConfig {
        name: "from-command-env-matrix-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![claude_restricted_provider(command)],
        inputs: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// shell_split contract — pins behavior used by both `provider_name` and the
// `provider_executable_name` runtime identity helper.
// ---------------------------------------------------------------------------

#[test]
fn shell_split_simple_two_tokens() {
    assert_eq!(shell_split("echo hello"), vec!["echo", "hello"]);
}

#[test]
fn shell_split_single_token() {
    assert_eq!(shell_split("codex"), vec!["codex"]);
}

#[test]
fn shell_split_preserves_quoted_whitespace() {
    assert_eq!(
        shell_split(r#"env -u FOO "my cmd""#),
        vec!["env", "-u", "FOO", "my cmd"]
    );
}

#[test]
fn shell_split_empty_string_returns_empty_vec() {
    assert_eq!(shell_split(""), Vec::<String>::new());
}

#[test]
fn shell_split_whitespace_only_returns_empty_vec() {
    assert_eq!(shell_split("   \t  "), Vec::<String>::new());
}

#[test]
fn shell_split_unmatched_quote_does_not_panic_and_does_not_error() {
    // Characterization: current `shell_split` does not error on unmatched
    // quotes; it treats the trailing content as a quoted token.
    let tokens = shell_split(r#"env "trailing"#);
    assert_eq!(tokens.len(), 2);
    assert_eq!(tokens[0], "env");
    // The trailing token has the leading quote stripped and contains
    // everything after, including any whitespace that would normally split.
    assert!(tokens[1].contains("trailing"));
}

#[test]
fn shell_split_no_escape_processing_for_backslash() {
    // Characterization: backslash is NOT an escape character in this parser.
    let tokens = shell_split(r#"echo a\ b"#);
    // Without escape handling, the backslash is included as a literal char
    // and the unescaped space splits the token.
    assert_eq!(tokens.len(), 3);
    assert_eq!(tokens[0], "echo");
}

#[test]
fn shell_split_single_quotes_are_treated_as_literal() {
    // Characterization: single quotes do NOT toggle quote state; they are
    // treated as ordinary characters that become part of the token.
    let tokens = shell_split(r#"echo 'foo bar'"#);
    assert_eq!(tokens.len(), 3);
    assert_eq!(tokens[0], "echo");
    assert!(tokens[1].starts_with('\''));
}

// ---------------------------------------------------------------------------
// provider_name contract — uses shell_split + last-token rule.
// ---------------------------------------------------------------------------

#[test]
fn provider_name_simple_returns_single_token() {
    assert_eq!(provider_name("claude"), "claude");
}

#[test]
fn provider_name_with_env_prefix_returns_last_token() {
    assert_eq!(provider_name("env -u CLAUDECODE claude"), "claude");
}

#[test]
fn provider_name_with_quoted_token_strips_quotes() {
    assert_eq!(provider_name(r#"env -u FOO "claude""#), "claude");
}

#[test]
fn provider_name_with_quoted_multi_word_preserves_internal_space() {
    assert_eq!(provider_name(r#"env -u FOO "my provider""#), "my provider");
}

#[test]
fn provider_name_empty_falls_back_to_input_string() {
    // From cli.rs:2485-2489 — `shell_split` yields no tokens; the fallback
    // is `command.to_string()`.
    assert_eq!(provider_name(""), "");
}

// ---------------------------------------------------------------------------
// ACR-205 intrinsic-surface verification (PP-002): bounded provider set.
//
// The runtime identity domain has exactly three values: `Claude`, `Codex`,
// and an `OpenAiCompat` fallback. We pin this through the OBSERVABLE effects:
// applying Claude tool_restrictions to a provider whose name and command both
// suggest "claude" produces `--disallowed-tools ...`; doing the same with a
// name "codex" produces a Codex policy block (`<<<NESTHARUS-POLICY>>>` etc.
// is only added when system_prompt_override exists; for restriction-only
// policy emission, Codex emits `-c` config_pairs and `--disable` features).
//
// Any future widening of the runtime identity domain MUST update both these
// tests and the ACR-205 intrinsic-surface declaration.
// ---------------------------------------------------------------------------

#[test]
fn intrinsic_surface_claude_identity_drives_claude_policy_emission() {
    let (_dir, script_path, argv_dump) = argv_dump_fixture();
    let mut provider = ProviderConfig::new(script_path.to_string_lossy().into_owned(), Vec::new());
    provider.name = "claude-fixture".to_string();
    provider.tool_restrictions = Some(ToolRestrictions {
        kind: ToolRestrictionKind::Claude,
        claude: ClaudeRestrictions {
            disallowed_tools: vec!["Task".to_string(), "Bash".to_string()],
            allowed_tools: vec!["Read".to_string()],
            disable_slash_commands: true,
        },
        codex: CodexRestrictions::default(),
    });
    let model = ModelConfig {
        name: "claude-id-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
    };
    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);
    let argv = read_argv(&argv_dump);

    let dis_pos = argv
        .iter()
        .position(|t| t == "--disallowed-tools")
        .expect("Claude policy must emit --disallowed-tools");
    assert_eq!(argv[dis_pos + 1], "Task,Bash");

    let allow_pos = argv
        .iter()
        .position(|t| t == "--allowed-tools")
        .expect("Claude policy must emit --allowed-tools when populated");
    assert_eq!(argv[allow_pos + 1], "Read");

    assert!(
        argv.iter().any(|t| t == "--disable-slash-commands"),
        "Claude policy must emit --disable-slash-commands when set; argv={argv:?}"
    );
}

#[test]
fn intrinsic_surface_codex_identity_drives_codex_policy_emission() {
    let (_dir, script_path, argv_dump) = argv_dump_fixture();
    let mut provider = ProviderConfig::new(script_path.to_string_lossy().into_owned(), Vec::new());
    provider.name = "codex-fixture".to_string();
    provider.tool_restrictions = Some(ToolRestrictions {
        kind: ToolRestrictionKind::Codex,
        claude: ClaudeRestrictions::default(),
        codex: CodexRestrictions {
            config_pairs: vec!["sandbox=workspace-write".to_string()],
            disabled_features: vec!["web_search".to_string()],
        },
    });
    let model = ModelConfig {
        name: "codex-id-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
    };
    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);
    let argv = read_argv(&argv_dump);

    let c_pos = argv
        .iter()
        .position(|t| t == "-c")
        .expect("Codex policy must emit -c config pair");
    assert_eq!(argv[c_pos + 1], "sandbox=workspace-write");

    let disable_pos = argv
        .iter()
        .position(|t| t == "--disable")
        .expect("Codex policy must emit --disable feature");
    assert_eq!(argv[disable_pos + 1], "web_search");
}

#[test]
fn intrinsic_surface_openai_compat_identity_emits_no_policy_when_no_restrictions() {
    // The default fallback: provider with neither system_prompt_override
    // nor tool_restrictions skips policy emission entirely.
    let (_dir, script_path, argv_dump) = argv_dump_fixture();
    let provider = ProviderConfig::new(script_path.to_string_lossy().into_owned(), Vec::new());
    let model = ModelConfig {
        name: "openai-compat-id-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
    };
    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);
    let argv = read_argv(&argv_dump);
    assert!(
        !argv.iter().any(|t| t == "--disallowed-tools"),
        "openai-compat fallback must not emit Claude policy; argv={argv:?}"
    );
    assert!(
        !argv.iter().any(|t| t == "-c"),
        "openai-compat fallback must not emit Codex config pairs; argv={argv:?}"
    );
}

// ---------------------------------------------------------------------------
// Identity-from-command tests: provider.name does NOT start with claude/codex
// but provider.command does — runtime identity helper must still classify
// correctly via `provider_executable_name` / `command_provider_basename`.
// ---------------------------------------------------------------------------

#[test]
fn intrinsic_surface_identity_from_command_path_basename_claude() {
    let dir = tempfile::tempdir().unwrap();
    let claude_bin = dir.path().join("claude");
    let argv_dump = dir.path().join("argv.txt");
    std::fs::write(
        &claude_bin,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$@\" > '{}'\n",
            argv_dump.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&claude_bin).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&claude_bin, perms).unwrap();

    // Provider name is "abc" (not claude-prefixed) but command basename is "claude".
    let mut provider = ProviderConfig::new(claude_bin.to_string_lossy().into_owned(), Vec::new());
    provider.name = "abc".to_string();
    provider.tool_restrictions = Some(ToolRestrictions {
        kind: ToolRestrictionKind::Claude,
        claude: ClaudeRestrictions {
            disallowed_tools: vec!["Task".to_string()],
            allowed_tools: Vec::new(),
            disable_slash_commands: false,
        },
        codex: CodexRestrictions::default(),
    });
    let model = ModelConfig {
        name: "from-command-claude-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
    };

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);
    let argv = read_argv(&argv_dump);
    assert!(
        argv.iter().any(|t| t == "--disallowed-tools"),
        "provider with claude-suffixed command path must be identified as Claude; argv={argv:?}"
    );
}

#[test]
fn intrinsic_surface_identity_via_env_prefix_skips_to_claude() {
    let dir = tempfile::tempdir().unwrap();
    let claude_bin = dir.path().join("claude");
    let argv_dump = dir.path().join("argv.txt");
    std::fs::write(
        &claude_bin,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$@\" > '{}'\n",
            argv_dump.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&claude_bin).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&claude_bin, perms).unwrap();

    // command = "env -u CLAUDECODE <claude_bin>" — the runtime identity
    // helper must skip `env`, the `-u` and its argument, and recognize
    // claude_bin as Claude.
    let command = format!("env -u CLAUDECODE {}", claude_bin.display());
    let mut provider = ProviderConfig::new(command, Vec::new());
    provider.name = "abc".to_string();
    provider.tool_restrictions = Some(ToolRestrictions {
        kind: ToolRestrictionKind::Claude,
        claude: ClaudeRestrictions {
            disallowed_tools: vec!["Task".to_string()],
            allowed_tools: Vec::new(),
            disable_slash_commands: false,
        },
        codex: CodexRestrictions::default(),
    });
    let model = ModelConfig {
        name: "env-prefix-claude-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
    };

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);
    let argv = read_argv(&argv_dump);
    assert!(
        argv.iter().any(|t| t == "--disallowed-tools"),
        "env -u prefix must not block claude identity; argv={argv:?}"
    );
}

#[test]
fn intrinsic_surface_identity_via_env_assignment_skips_to_claude() {
    let (_dir, env_path, claude_path, argv_dump) = fake_env_and_claude_fixture();
    assert_command_is_identified_as_claude(
        format!("{} FOO=bar {}", env_path.display(), claude_path.display()),
        &argv_dump,
    );
}

#[test]
fn intrinsic_surface_identity_via_env_e_option_skips_to_claude() {
    let (_dir, env_path, claude_path, argv_dump) = fake_env_and_claude_fixture();
    assert_command_is_identified_as_claude(
        format!("{} -e FOO {}", env_path.display(), claude_path.display()),
        &argv_dump,
    );
}

#[test]
fn intrinsic_surface_identity_via_env_s_option_skips_to_claude() {
    let (_dir, env_path, claude_path, argv_dump) = fake_env_and_claude_fixture();
    assert_command_is_identified_as_claude(
        format!(
            "{} -S ignored {}",
            env_path.display(),
            claude_path.display()
        ),
        &argv_dump,
    );
}

#[test]
fn intrinsic_surface_identity_via_env_long_option_skips_to_claude() {
    let (_dir, env_path, claude_path, argv_dump) = fake_env_and_claude_fixture();
    assert_command_is_identified_as_claude(
        format!(
            "{} --ignore-environment {}",
            env_path.display(),
            claude_path.display()
        ),
        &argv_dump,
    );
}

#[test]
fn intrinsic_surface_identity_via_env_mixed_options_skips_to_claude() {
    let (_dir, env_path, claude_path, argv_dump) = fake_env_and_claude_fixture();
    assert_command_is_identified_as_claude(
        format!(
            "{} -i -u FOO FOO=bar {}",
            env_path.display(),
            claude_path.display()
        ),
        &argv_dump,
    );
}

#[test]
fn intrinsic_surface_identity_via_env_without_executable_fails_policy_inference() {
    let (_dir, env_path, _claude_path, _argv_dump) = fake_env_and_claude_fixture();
    let mut provider = ProviderConfig::new(format!("{} -u FOO", env_path.display()), Vec::new());
    provider.name = "abc".to_string();
    provider.system_prompt_override = Some("some override".to_string());
    let model = ModelConfig {
        name: "env-no-command-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
    };

    let err = execute(&model, 0, "prompt", None, &HashMap::new(), None).unwrap_err();
    assert!(
        err.contains("defines policy but its ecosystem could not be inferred"),
        "{err}"
    );
}

// ---------------------------------------------------------------------------
// Policy validation failure paths.
// ---------------------------------------------------------------------------

#[test]
fn policy_with_unknown_provider_kind_fails_closed() {
    // Provider name "abc" + bare command "abc" + system_prompt_override
    // means no kind can be inferred. Expected error string matches the
    // current contract.
    let mut provider = ProviderConfig::new("abc", Vec::new());
    provider.name = "abc".to_string();
    provider.system_prompt_override = Some("some override".to_string());
    let model = ModelConfig {
        name: "unknown-kind-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
    };

    let err = execute(&model, 0, "prompt", None, &HashMap::new(), None).unwrap_err();
    assert!(
        err.contains("defines policy but its ecosystem could not be inferred"),
        "{err}"
    );
}

#[test]
fn policy_with_claude_kind_payload_codex_settings_fails_closed() {
    let (_dir, script_path, _argv_dump) = argv_dump_fixture();
    let mut provider = ProviderConfig::new(script_path.to_string_lossy().into_owned(), Vec::new());
    provider.name = "claude-fixture".to_string();
    provider.tool_restrictions = Some(ToolRestrictions {
        kind: ToolRestrictionKind::Claude,
        claude: ClaudeRestrictions::default(),
        codex: CodexRestrictions {
            config_pairs: vec!["sandbox=workspace-write".to_string()],
            disabled_features: Vec::new(),
        },
    });
    let model = ModelConfig {
        name: "claude-with-codex-payload-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
    };

    let err = execute(&model, 0, "prompt", None, &HashMap::new(), None).unwrap_err();
    assert!(
        err.contains("declares Claude restrictions but populated Codex settings"),
        "{err}"
    );
}

#[test]
fn policy_with_codex_kind_payload_claude_settings_fails_closed() {
    let (_dir, script_path, _argv_dump) = argv_dump_fixture();
    let mut provider = ProviderConfig::new(script_path.to_string_lossy().into_owned(), Vec::new());
    provider.name = "codex-fixture".to_string();
    provider.tool_restrictions = Some(ToolRestrictions {
        kind: ToolRestrictionKind::Codex,
        claude: ClaudeRestrictions {
            disallowed_tools: vec!["Task".to_string()],
            allowed_tools: Vec::new(),
            disable_slash_commands: false,
        },
        codex: CodexRestrictions::default(),
    });
    let model = ModelConfig {
        name: "codex-with-claude-payload-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
    };

    let err = execute(&model, 0, "prompt", None, &HashMap::new(), None).unwrap_err();
    assert!(
        err.contains("declares Codex restrictions but populated Claude settings"),
        "{err}"
    );
}

#[test]
fn policy_with_both_claude_and_codex_populated_fails_closed() {
    let (_dir, script_path, _argv_dump) = argv_dump_fixture();
    let mut provider = ProviderConfig::new(script_path.to_string_lossy().into_owned(), Vec::new());
    provider.name = "ambiguous-fixture".to_string();
    provider.tool_restrictions = Some(ToolRestrictions {
        kind: ToolRestrictionKind::Claude,
        claude: ClaudeRestrictions {
            disallowed_tools: vec!["Task".to_string()],
            allowed_tools: Vec::new(),
            disable_slash_commands: false,
        },
        codex: CodexRestrictions {
            config_pairs: vec!["sandbox=workspace-write".to_string()],
            disabled_features: Vec::new(),
        },
    });
    let model = ModelConfig {
        name: "ambiguous-policy-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
    };

    let err = execute(&model, 0, "prompt", None, &HashMap::new(), None).unwrap_err();
    assert!(
        err.contains("has both Claude and Codex tool restrictions populated"),
        "{err}"
    );
}

// ---------------------------------------------------------------------------
// Claude policy: system_prompt_override emits --append-system-prompt before
// tool restriction flags.
// ---------------------------------------------------------------------------

#[test]
fn claude_policy_renders_append_system_prompt_in_order() {
    let (_dir, script_path, argv_dump) = argv_dump_fixture();
    let mut provider = ProviderConfig::new(script_path.to_string_lossy().into_owned(), Vec::new());
    provider.name = "claude-fixture".to_string();
    provider.system_prompt_override = Some("Be precise.".to_string());
    provider.tool_restrictions = Some(ToolRestrictions {
        kind: ToolRestrictionKind::Claude,
        claude: ClaudeRestrictions {
            disallowed_tools: vec!["Task".to_string()],
            allowed_tools: Vec::new(),
            disable_slash_commands: false,
        },
        codex: CodexRestrictions::default(),
    });
    let model = ModelConfig {
        name: "claude-append-system-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
    };

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);
    let argv = read_argv(&argv_dump);
    let append_pos = argv
        .iter()
        .position(|t| t == "--append-system-prompt")
        .expect("--append-system-prompt present");
    let dis_pos = argv
        .iter()
        .position(|t| t == "--disallowed-tools")
        .expect("--disallowed-tools present");
    assert!(
        append_pos < dis_pos,
        "Claude policy must render --append-system-prompt before tool flags; argv={argv:?}"
    );
    assert_eq!(argv[append_pos + 1], "Be precise.");
}

// ---------------------------------------------------------------------------
// Codex policy without system_prompt_override does NOT emit the policy
// block but DOES emit config pairs and disabled features.
// ---------------------------------------------------------------------------

#[test]
fn codex_policy_without_override_emits_no_policy_block_in_argv_prompt() {
    let (_dir, script_path, argv_dump) = argv_dump_fixture();
    let mut provider = ProviderConfig::new(script_path.to_string_lossy().into_owned(), Vec::new());
    provider.name = "codex-fixture".to_string();
    provider.tool_restrictions = Some(ToolRestrictions {
        kind: ToolRestrictionKind::Codex,
        claude: ClaudeRestrictions::default(),
        codex: CodexRestrictions {
            config_pairs: vec!["sandbox=workspace-write".to_string()],
            disabled_features: vec!["web_search".to_string()],
        },
    });
    let model = ModelConfig {
        name: "codex-no-override-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
    };

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);
    let argv = read_argv(&argv_dump);
    // No NESTHARUS-POLICY block in the argv prompt.
    assert!(
        !argv.iter().any(|t| t.contains("<<<NESTHARUS-POLICY>>>")),
        "Codex policy must not render the policy block without system_prompt_override; argv={argv:?}"
    );
}

// ---------------------------------------------------------------------------
// Codex policy WITH system_prompt_override and arg-mode prompt: the policy
// block is prepended to the prompt argument.
// ---------------------------------------------------------------------------

#[test]
fn codex_policy_with_override_prepends_policy_block_to_arg_prompt() {
    let (_dir, script_path, argv_dump) = argv_dump_fixture();
    let mut provider = ProviderConfig::new(script_path.to_string_lossy().into_owned(), Vec::new());
    provider.name = "codex-fixture".to_string();
    provider.system_prompt_override = Some("Be precise.".to_string());
    provider.tool_restrictions = Some(ToolRestrictions {
        kind: ToolRestrictionKind::Codex,
        claude: ClaudeRestrictions::default(),
        codex: CodexRestrictions {
            config_pairs: Vec::new(),
            disabled_features: Vec::new(),
        },
    });
    let model = ModelConfig {
        name: "codex-with-override-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
    };

    let result = execute(&model, 0, "user prompt", None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);
    let argv = read_argv(&argv_dump);
    let combined = argv.join("\n");
    assert!(
        combined.contains("<<<NESTHARUS-POLICY>>>"),
        "Codex policy must prepend NESTHARUS-POLICY block to the prompt; argv={argv:?}"
    );
    assert!(
        combined.contains("Be precise."),
        "Codex policy must include override text; argv={argv:?}"
    );
    assert!(
        combined.contains("user prompt"),
        "Codex policy must preserve the user prompt; argv={argv:?}"
    );
}
