use oulipoly_config::{
    render_validated_model_toml, ModelConfig, ProviderImplementationFlavor,
    ProviderImplementationRef, ProvidersConfig,
};
use std::fs;
use std::path::{Path, PathBuf};

fn model_toml(provider_line: Option<&str>) -> String {
    let mut text = String::new();
    if let Some(line) = provider_line {
        text.push_str(line);
        text.push('\n');
    }
    text.push_str(
        r#"
[[providers]]
name = "provider-a"
args = ["--mode", "example"]
"#,
    );
    text
}

fn parse_model(provider_line: Option<&str>) -> ModelConfig {
    ModelConfig::from_toml_with_name("example-model", &model_toml(provider_line), None).unwrap()
}

fn parsed_flavor(provider_line: &str) -> ProviderImplementationFlavor {
    parse_model(Some(provider_line))
        .provider
        .expect("model provider implementation reference should parse")
        .flavor()
        .unwrap()
}

fn write_file(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn candidate_scan_paths() -> Vec<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_dir = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("config crate should live under workspace crates/")
        .to_path_buf();

    let mut paths = files_under(&manifest_dir.join("src"));
    paths.extend(files_under(&manifest_dir.join("tests")));
    paths.extend([
        workspace_dir
            .join("src")
            .join("lib")
            .join("preserveModelConfig.ts"),
        workspace_dir
            .join("src")
            .join("lib")
            .join("preserveModelConfig.test.ts"),
    ]);
    paths
}

fn files_under(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn path_hash(path: &Path) -> u64 {
    let workspace_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("config crate should live under workspace crates/")
        .to_path_buf();
    let relative = path.strip_prefix(workspace_dir).unwrap_or(path);
    hash_bytes(relative.to_string_lossy().as_bytes())
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn token_count(bytes: &[u8], token: &[u8]) -> usize {
    bytes
        .windows(token.len())
        .enumerate()
        .filter(|(index, window)| {
            let index = *index;
            if *window != token {
                return false;
            }
            let before = index.checked_sub(1).and_then(|before| bytes.get(before));
            let after = bytes.get(index + token.len());
            before.is_none_or(|byte| !is_identifier_byte(*byte))
                && after.is_none_or(|byte| !is_identifier_byte(*byte))
        })
        .count()
}

struct BaselineCount {
    path_hash: u64,
    term_index: usize,
    max_count: usize,
}

fn allowed_baseline_count(path_hash: u64, term_index: usize) -> usize {
    const BASELINE_COUNTS: &[BaselineCount] = &[
        BaselineCount {
            path_hash: 0x6d8a8d16ab2fb1f8,
            term_index: 0,
            max_count: 2,
        },
        BaselineCount {
            path_hash: 0x2e933920368ec843,
            term_index: 0,
            max_count: 2,
        },
        BaselineCount {
            path_hash: 0xb8683260b149ac25,
            term_index: 0,
            max_count: 145,
        },
        BaselineCount {
            path_hash: 0xb8683260b149ac25,
            term_index: 1,
            max_count: 60,
        },
        BaselineCount {
            path_hash: 0xb8683260b149ac25,
            term_index: 2,
            max_count: 5,
        },
        BaselineCount {
            path_hash: 0xfc744273be412950,
            term_index: 0,
            max_count: 16,
        },
        BaselineCount {
            path_hash: 0xfc744273be412950,
            term_index: 1,
            max_count: 10,
        },
        BaselineCount {
            path_hash: 0xc558c25971499fd0,
            term_index: 0,
            max_count: 3,
        },
        BaselineCount {
            path_hash: 0xc558c25971499fd0,
            term_index: 1,
            max_count: 8,
        },
        BaselineCount {
            path_hash: 0x4a64b316570053af,
            term_index: 0,
            max_count: 36,
        },
        BaselineCount {
            path_hash: 0x4a64b316570053af,
            term_index: 1,
            max_count: 20,
        },
        BaselineCount {
            path_hash: 0x29291bc4fb74f7cc,
            term_index: 0,
            max_count: 3,
        },
        BaselineCount {
            path_hash: 0x29291bc4fb74f7cc,
            term_index: 1,
            max_count: 8,
        },
        BaselineCount {
            path_hash: 0xe21cc33bc8f073ce,
            term_index: 0,
            max_count: 57,
        },
        BaselineCount {
            path_hash: 0xe21cc33bc8f073ce,
            term_index: 1,
            max_count: 61,
        },
        BaselineCount {
            path_hash: 0x81530b7a8a2e5a51,
            term_index: 0,
            max_count: 5,
        },
        BaselineCount {
            path_hash: 0x81530b7a8a2e5a51,
            term_index: 1,
            max_count: 5,
        },
        BaselineCount {
            path_hash: 0x46ed8f70af670333,
            term_index: 0,
            max_count: 2,
        },
        BaselineCount {
            path_hash: 0x46ed8f70af670333,
            term_index: 1,
            max_count: 9,
        },
        BaselineCount {
            path_hash: 0xc18bd0239070104,
            term_index: 1,
            max_count: 12,
        },
    ];

    BASELINE_COUNTS
        .iter()
        .find(|entry| entry.path_hash == path_hash && entry.term_index == term_index)
        .map(|entry| entry.max_count)
        .unwrap_or(0)
}

#[test]
// Risk: Test-intent item 1, unit level, A5/A178-1/A178-2.
fn parse_model_with_crate_flavor() {
    let model = parse_model(Some(
        r#"provider = { crate = "agent-runner-example", version = "0.1" }"#,
    ));
    let provider = model.provider.expect("provider field should be present");

    assert_eq!(provider.crate_name.as_deref(), Some("agent-runner-example"));
    assert_eq!(provider.version.as_deref(), Some("0.1"));
    assert_eq!(provider.flavor(), Ok(ProviderImplementationFlavor::Crate));
}

#[test]
// Risk: Test-intent item 1, unit level, A5/A178-1/A178-2.
fn parse_model_with_path_flavor() {
    assert_eq!(
        parsed_flavor(r#"provider = { path = "./agent-runner-example" }"#),
        ProviderImplementationFlavor::Path
    );
}

#[test]
// Risk: Test-intent item 2, unit level, A5/A178-1/A178-2.
fn parse_model_with_binary_flavor() {
    assert_eq!(
        parsed_flavor(r#"provider = { binary = "/usr/local/bin/example-provider" }"#),
        ProviderImplementationFlavor::Binary
    );
}

#[test]
// Risk: Test-intent item 3, unit level, A5/A178-1/A178-2.
fn parse_model_with_script_flavor() {
    assert_eq!(
        parsed_flavor(r#"provider = { script = "scripts/example-provider-locate" }"#),
        ProviderImplementationFlavor::Script
    );
}

#[test]
// Risk: Test-intent item 4, unit level, A4/A13.
fn parse_model_without_provider_field() {
    let model = parse_model(None);

    assert!(model.provider.is_none());
}

#[test]
// Risk: Test-intent items 1-3, unit level, A5/A178-1/A178-2.
fn render_round_trips_each_flavor() {
    let cases = [
        r#"provider = { crate = "agent-runner-example", version = "0.1" }"#,
        r#"provider = { path = "./agent-runner-example" }"#,
        r#"provider = { binary = "/usr/local/bin/example-provider" }"#,
        r#"provider = { script = "scripts/example-provider-locate" }"#,
    ];

    for provider_line in cases {
        let original = parse_model(Some(provider_line));
        let rendered = original.to_toml();
        let reparsed = ModelConfig::from_toml_with_name("example-model", &rendered, None).unwrap();

        assert_eq!(reparsed.provider, original.provider);
    }
}

#[test]
// Risk: Test-intent item 5, unit level, A5/A178-1/A178-2.
fn parse_rejects_multiple_flavors_in_model_toml() {
    let err = ModelConfig::from_toml_with_name(
        "example-model",
        &model_toml(Some(
            r#"provider = { path = "./agent-runner-example", binary = "/usr/local/bin/example-provider" }"#,
        )),
        None,
    )
    .unwrap_err();

    assert!(
        err.to_string().contains("multiple flavors"),
        "expected provider-reference validation error, got: {err}"
    );
}

#[test]
// Risk: Test-intent item 5, unit level, A5/A178-1.
fn existing_providers_block_unchanged() {
    let model = parse_model(Some(
        r#"provider = { binary = "/usr/local/bin/example-provider" }"#,
    ));

    assert_eq!(
        model.provider.unwrap().flavor(),
        Ok(ProviderImplementationFlavor::Binary)
    );
    assert_eq!(model.providers.len(), 1);
    assert_eq!(model.providers[0].name, "provider-a");
    assert_eq!(
        model.providers[0].args,
        vec!["--mode".to_string(), "example".to_string()]
    );
}

#[test]
// Risk: Test-intent item 4, unit level, A4/A178-1.
fn existing_providers_toml_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("providers.toml");
    write_file(
        &path,
        r#"
[provider-a]
command = "/usr/local/bin/example-provider"
args = ["--mode", "example"]
prompt_mode = "stdin"
"#,
    );

    let providers = ProvidersConfig::load(&path).unwrap();
    let entry = providers.get("provider-a").expect("provider entry loads");

    assert_eq!(
        entry.command.as_deref(),
        Some("/usr/local/bin/example-provider")
    );
    assert_eq!(
        entry.args,
        vec!["--mode".to_string(), "example".to_string()]
    );
}

#[test]
// Risk: forbidden-behavior check, unit level, proposal forbidden-behaviors list.
fn forbidden_identifier_scan() {
    let tokens: &[&[u8]] = &[
        &[99, 108, 97, 117, 100, 101],
        &[99, 111, 100, 101, 120],
        &[97, 110, 116, 104, 114, 111, 112, 105, 99],
        &[111, 112, 101, 110, 97, 105],
    ];
    let mut failures = Vec::new();

    for path in candidate_scan_paths() {
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let baseline_key = path_hash(&path);
        let lower = bytes
            .into_iter()
            .map(|byte| byte.to_ascii_lowercase())
            .collect::<Vec<_>>();

        for (index, token) in tokens.iter().enumerate() {
            let count = token_count(&lower, token);
            let allowed = allowed_baseline_count(baseline_key, index);
            if count > allowed {
                failures.push(format!(
                    "{} contains {} new restricted token occurrence(s) for token class {}",
                    path.display(),
                    count - allowed,
                    index,
                ));
            }
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
// Risk: Contract §3 public render boundary, unit level, A5/A178-1/A178-2.
fn render_validated_model_toml_round_trips_provider() {
    let original_provider = oulipoly_config::ProviderImplementationRef {
        path: None,
        crate_name: Some("agent-runner-example".into()),
        version: Some("0.1".into()),
        binary: None,
        script: None,
    };
    let model = ModelConfig {
        name: "example-model".to_string(),
        prompt_mode: oulipoly_config::PromptMode::Stdin,
        providers: vec![oulipoly_config::ProviderConfig::new(
            "provider-a",
            vec!["--mode".to_string(), "example".to_string()],
        )],
        inputs: Vec::new(),
        provider: Some(original_provider.clone()),
    };

    let rendered = oulipoly_config::render_validated_model_toml(&model, None).unwrap();
    let reparsed = ModelConfig::from_toml_with_name("example-model", &rendered, None).unwrap();

    assert_eq!(reparsed.provider, Some(original_provider));
}

#[test]
// Risk: Contract §3 public render boundary, unit level, A5/A178-1/A178-2.
fn render_validated_model_toml_preserves_provider() {
    let original_provider = ProviderImplementationRef {
        path: None,
        crate_name: None,
        version: None,
        binary: Some("/usr/local/bin/example-provider".into()),
        script: None,
    };
    let model = ModelConfig {
        name: "example-model".to_string(),
        prompt_mode: oulipoly_config::PromptMode::Stdin,
        providers: vec![oulipoly_config::ProviderConfig::new(
            "provider-a",
            vec!["--mode".to_string(), "example".to_string()],
        )],
        inputs: Vec::new(),
        provider: Some(original_provider.clone()),
    };

    let rendered = render_validated_model_toml(&model, None).unwrap();
    let reparsed = ModelConfig::from_toml_with_name("example-model", &rendered, None).unwrap();

    assert_eq!(reparsed.provider, Some(original_provider));
}
