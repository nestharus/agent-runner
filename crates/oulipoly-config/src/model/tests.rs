//! ## Declared roles
//!
//! `validator`

use crate::providers::{ProviderEntry, ProvidersConfig};

use super::*;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

mod codex_overlap_tests;
mod derive_tests;
mod provider_validation_tests;
mod toml_io_tests;

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

fn write_providers_toml(root: &Path, body: &str) {
    fs::write(root.join("providers.toml"), body).unwrap();
}

fn write_model_toml(models_dir: &Path, name: &str, body: &str) {
    fs::create_dir_all(models_dir).unwrap();
    fs::write(models_dir.join(format!("{name}.toml")), body).unwrap();
}

fn load_temp_models(
    providers_toml: &str,
    model_name: &str,
    model_toml: &str,
) -> Result<HashMap<String, ModelConfig>, String> {
    let temp = TempDir::new().unwrap();
    let root = temp.path();
    let models_dir = root.join("models");
    write_providers_toml(root, providers_toml);
    write_model_toml(&models_dir, model_name, model_toml);

    let providers = ProvidersConfig::load(&root.join("providers.toml")).unwrap();
    // AGE-40 Step 6c adds the `load_models(&Path, Option<&ProvidersConfig>)` signature.
    Ok(load_models(&models_dir, Some(&providers))?)
}

fn test_model(provider_name: &str, args: &[&str]) -> ModelConfig {
    ModelConfig {
        name: "gpt-high".into(),
        prompt_mode: PromptMode::Stdin,
        providers: vec![ProviderConfig::model_provider(
            provider_name,
            args.iter().map(|arg| (*arg).to_string()).collect(),
        )],
        inputs: vec![],
        provider: None,
    }
}

fn test_providers(provider_name: &str, args: &[&str]) -> ProvidersConfig {
    let mut entries = HashMap::new();
    entries.insert(
        provider_name.to_string(),
        ProviderEntry {
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            ..ProviderEntry::default()
        },
    );
    ProvidersConfig { entries }
}

fn codex_providers(args: &[&str], interactive_args: Option<&[&str]>) -> ProvidersConfig {
    let mut entries = HashMap::new();
    entries.insert(
        "codex".to_string(),
        ProviderEntry {
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            interactive_args: interactive_args
                .map(|args| args.iter().map(|arg| (*arg).to_string()).collect()),
            ..ProviderEntry::default()
        },
    );
    ProvidersConfig { entries }
}

fn codex_providers_with_typed_policy(config_pairs: &[&str]) -> ProvidersConfig {
    let mut entries = HashMap::new();
    entries.insert(
        "codex".to_string(),
        ProviderEntry {
            command: Some("codex".to_string()),
            args: vec!["exec".to_string()],
            tool_restrictions: Some(ToolRestrictions {
                kind: ToolRestrictionKind::Codex,
                claude: ClaudeRestrictions::default(),
                codex: CodexRestrictions {
                    config_pairs: config_pairs
                        .iter()
                        .map(|pair| (*pair).to_string())
                        .collect(),
                    disabled_features: Vec::new(),
                },
            }),
            ..ProviderEntry::default()
        },
    );
    ProvidersConfig { entries }
}
