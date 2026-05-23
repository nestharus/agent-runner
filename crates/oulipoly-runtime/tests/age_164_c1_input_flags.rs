//! AGE-164 cluster 1 — input-flags public-API + characterization tests.
//!
//! These tests pin the CURRENT pre-refactor behavior of input-schema validation
//! and flag formatting. They exercise `execute` (the public entrypoint) with
//! a no-op fixture provider whose argv is dumped to a sidecar file, so the
//! formatted argv is observed externally without depending on any private
//! helper. Coverage gaps 1-5 from the AGE-164 coverage inventory
//! (integer / number / array validation, repeated values, TOML default
//! formatting, required-input truth table) are exercised here.
//!
//! After the Step 6c refactor moves these helpers under `executor/cli/`, every
//! test in this file must remain PASS — the contract's public API stability
//! invariants pin the observable behavior these tests assert.
//!
//! ## Declared roles
//!
//! Roles: formatter, parser, mapper, validator, predicate, accessor.
//!
//! - formatter: fixture shell scripts and expected argv fragments.
//! - parser: argv sidecar line loading through `argv_dump_contents`.
//! - mapper: model/input fixture construction.
//! - validator: public API behavior assertions.
//! - predicate: [`token_is_file_flag`].
//! - accessor: [`indexed_token_index`].
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/tests/age_164_c1_input_flags.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly-config-input-schema-test-contract
//!       - executor-cli-public-api-test-contract
//!       - unix-fixture-argv-sidecar-contract
//! ```

#![cfg(unix)]

use oulipoly_config::{
    InputDef, InputType, ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy,
};
use oulipoly_runtime::executor::cli::execute;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

struct ArgvFixture {
    _dir: tempfile::TempDir,
    script_path: PathBuf,
    argv_dump: PathBuf,
}

fn argv_dump_fixture() -> ArgvFixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let argv_dump = dir.path().join("argv.txt");
    let script_path = dir.path().join("argv-dump.sh");
    std::fs::write(&script_path, argv_dump_script_body(&argv_dump)).expect("write script");
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();
    ArgvFixture {
        _dir: dir,
        script_path,
        argv_dump,
    }
}

fn argv_dump_script_body(argv_dump: &Path) -> String {
    format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$@\" > '{}'\n",
        argv_dump.display()
    )
}

fn argv_dump_contents(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .expect("argv dump readable")
        .lines()
        .map(str::to_string)
        .collect()
}

fn token_is_file_flag(token: &str) -> bool {
    token == "--file"
}

fn indexed_token_index(indexed: (usize, &String)) -> usize {
    indexed.0
}

fn model_with_inputs(script_path: &std::path::Path, inputs: Vec<InputDef>) -> ModelConfig {
    ModelConfig {
        name: "age-164-c1-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::new(
            script_path.to_string_lossy().into_owned(),
            Vec::new(),
        )],
        inputs,
        provider: None,
    }
}

fn input_def(name: &str, input_type: InputType, flag: Option<&str>) -> InputDef {
    InputDef {
        name: name.to_string(),
        input_type,
        required: false,
        default_input: false,
        default: None,
        description: None,
        flag: flag.map(str::to_string),
    }
}

fn required_input_def(name: &str, input_type: InputType, flag: Option<&str>) -> InputDef {
    InputDef {
        required: true,
        ..input_def(name, input_type, flag)
    }
}

fn defaulted_input_def(
    name: &str,
    input_type: InputType,
    flag: Option<&str>,
    default: toml::Value,
) -> InputDef {
    InputDef {
        default: Some(default),
        ..input_def(name, input_type, flag)
    }
}

// Pinned behavior: integer input with a valid value passes through and is emitted with its flag.
#[test]
fn integer_input_valid_value_emitted_through_flag() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![input_def(
            "count",
            InputType::Integer {
                min: None,
                max: None,
            },
            Some("--count"),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert("count".to_string(), vec!["42".to_string()]);

    let result = execute(&model, 0, "prompt", None, &inputs, None).expect("execute");

    assert_eq!(result.exit_code, 0);
    let argv = argv_dump_contents(&fx.argv_dump);
    assert!(argv.contains(&"--count".to_string()), "argv={argv:?}");
    assert!(argv.contains(&"42".to_string()), "argv={argv:?}");
}

// Pinned behavior: integer with a non-integer value rejects with the canonical
// "is not a valid integer" error string.
#[test]
fn integer_input_rejects_non_integer() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![input_def(
            "count",
            InputType::Integer {
                min: None,
                max: None,
            },
            Some("--count"),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert("count".to_string(), vec!["nope".to_string()]);

    let err = execute(&model, 0, "prompt", None, &inputs, None).unwrap_err();
    assert!(
        err.contains("Input 'count'") && err.contains("not a valid integer"),
        "{err}"
    );
}

#[test]
fn integer_input_rejects_below_min() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![input_def(
            "count",
            InputType::Integer {
                min: Some(10),
                max: None,
            },
            Some("--count"),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert("count".to_string(), vec!["3".to_string()]);

    let err = execute(&model, 0, "prompt", None, &inputs, None).unwrap_err();
    assert!(err.contains("below minimum") && err.contains("10"), "{err}");
}

#[test]
fn integer_input_rejects_above_max() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![input_def(
            "count",
            InputType::Integer {
                min: None,
                max: Some(5),
            },
            Some("--count"),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert("count".to_string(), vec!["9".to_string()]);

    let err = execute(&model, 0, "prompt", None, &inputs, None).unwrap_err();
    assert!(
        err.contains("exceeds maximum") && err.contains("5"),
        "{err}"
    );
}

#[test]
fn enum_input_valid_value_emitted_through_flag() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![defaulted_input_def(
            "size",
            InputType::Enum {
                options: vec!["2048*2048".to_string(), "1024*1024".to_string()],
            },
            Some("--size"),
            toml::Value::String("2048*2048".to_string()),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert("size".to_string(), vec!["1024*1024".to_string()]);

    let result = execute(&model, 0, "prompt", None, &inputs, None).expect("execute");

    assert_eq!(result.exit_code, 0);
    let argv = argv_dump_contents(&fx.argv_dump);
    let pos = argv
        .iter()
        .position(|t| t == "--size")
        .expect("--size present");
    assert_eq!(argv[pos + 1], "1024*1024", "argv={argv:?}");
}

#[test]
fn enum_input_default_emitted_through_flag() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![defaulted_input_def(
            "format",
            InputType::Enum {
                options: vec!["jpeg".to_string(), "png".to_string()],
            },
            Some("--format"),
            toml::Value::String("jpeg".to_string()),
        )],
    );

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");

    assert_eq!(result.exit_code, 0);
    let argv = argv_dump_contents(&fx.argv_dump);
    let pos = argv
        .iter()
        .position(|t| t == "--format")
        .expect("--format present");
    assert_eq!(argv[pos + 1], "jpeg", "argv={argv:?}");
}

#[test]
fn enum_input_rejects_invalid_option() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![input_def(
            "size",
            InputType::Enum {
                options: vec!["small".to_string(), "large".to_string()],
            },
            Some("--size"),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert("size".to_string(), vec!["huge".to_string()]);

    let err = execute(&model, 0, "prompt", None, &inputs, None).unwrap_err();
    assert!(
        err.contains("Input 'size'")
            && err.contains("'huge'")
            && err.contains("not a valid option"),
        "{err}"
    );
}

#[test]
fn number_input_valid_float_emitted_through_flag() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![input_def(
            "temp",
            InputType::Number {
                min: None,
                max: None,
            },
            Some("--temp"),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert("temp".to_string(), vec!["0.75".to_string()]);

    let result = execute(&model, 0, "prompt", None, &inputs, None).expect("execute");

    assert_eq!(result.exit_code, 0);
    let argv = argv_dump_contents(&fx.argv_dump);
    assert!(argv.contains(&"0.75".to_string()), "argv={argv:?}");
}

#[test]
fn number_input_rejects_non_number() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![input_def(
            "temp",
            InputType::Number {
                min: None,
                max: None,
            },
            Some("--temp"),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert("temp".to_string(), vec!["nope".to_string()]);

    let err = execute(&model, 0, "prompt", None, &inputs, None).unwrap_err();
    assert!(
        err.contains("Input 'temp'") && err.contains("not a valid number"),
        "{err}"
    );
}

#[test]
fn number_input_rejects_below_min() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![input_def(
            "temp",
            InputType::Number {
                min: Some(0.0),
                max: None,
            },
            Some("--temp"),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert("temp".to_string(), vec!["-1.5".to_string()]);

    let err = execute(&model, 0, "prompt", None, &inputs, None).unwrap_err();
    assert!(err.contains("below minimum"), "{err}");
}

#[test]
fn number_input_rejects_above_max() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![input_def(
            "temp",
            InputType::Number {
                min: None,
                max: Some(1.0),
            },
            Some("--temp"),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert("temp".to_string(), vec!["2.0".to_string()]);

    let err = execute(&model, 0, "prompt", None, &inputs, None).unwrap_err();
    assert!(err.contains("exceeds maximum"), "{err}");
}

#[test]
fn array_input_rejects_below_min_items() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![input_def(
            "files",
            InputType::Array {
                item_type: "string".to_string(),
                min_items: Some(2),
                max_items: None,
            },
            Some("--file"),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert("files".to_string(), vec!["only-one.txt".to_string()]);

    let err = execute(&model, 0, "prompt", None, &inputs, None).unwrap_err();
    assert!(
        err.contains("need at least 2 items") && err.contains("got 1"),
        "{err}"
    );
}

#[test]
fn array_input_rejects_above_max_items() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![input_def(
            "files",
            InputType::Array {
                item_type: "string".to_string(),
                min_items: None,
                max_items: Some(2),
            },
            Some("--file"),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert(
        "files".to_string(),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
    );

    let err = execute(&model, 0, "prompt", None, &inputs, None).unwrap_err();
    assert!(
        err.contains("maximum 2 items") && err.contains("got 3"),
        "{err}"
    );
}

// Pinned behavior: repeated values for one input emit the flag once per value
// (preserving order of values as collected into the Vec). Order of values
// within a single key is deterministic; order across keys in `extra_inputs`
// is HashMap-derived, so this test pins only the per-key ordering.
#[test]
fn repeated_values_for_one_input_emit_flag_per_value() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![input_def(
            "files",
            InputType::Array {
                item_type: "string".to_string(),
                min_items: None,
                max_items: None,
            },
            Some("--file"),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert(
        "files".to_string(),
        vec!["a.txt".to_string(), "b.txt".to_string()],
    );

    let result = execute(&model, 0, "prompt", None, &inputs, None).expect("execute");

    assert_eq!(result.exit_code, 0);
    let argv = argv_dump_contents(&fx.argv_dump);
    let file_positions: Vec<usize> = argv
        .iter()
        .enumerate()
        .filter(|(_, token)| token_is_file_flag(token))
        .map(indexed_token_index)
        .collect();
    assert_eq!(file_positions.len(), 2, "argv={argv:?}");
    let values: Vec<&String> = file_positions.iter().map(|i| &argv[i + 1]).collect();
    assert!(
        values.contains(&&"a.txt".to_string()) && values.contains(&&"b.txt".to_string()),
        "argv={argv:?}",
    );
}

// Pinned behavior: known schema input without `flag` falls back to `--{name}`.
// This is the current behavior at `cli.rs::append_extra_input_flags` and is
// part of the pre-refactor contract.
#[test]
fn known_schema_input_without_flag_falls_back_to_double_dash_name() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![input_def("greeting", InputType::String, None)],
    );
    let mut inputs = HashMap::new();
    inputs.insert("greeting".to_string(), vec!["hello".to_string()]);

    let result = execute(&model, 0, "prompt", None, &inputs, None).expect("execute");

    assert_eq!(result.exit_code, 0);
    let argv = argv_dump_contents(&fx.argv_dump);
    let pos = argv
        .iter()
        .position(|t| t == "--greeting")
        .expect("--greeting present");
    assert_eq!(argv[pos + 1], "hello", "argv={argv:?}");
}

// Pinned behavior: a default value with NO `flag` is silently dropped (current
// behavior at `append_default_input_flags`). This characterizes the explicit/default
// divergence noted by the AGE-164 hookpoint research for cluster 1.
#[test]
fn default_value_without_flag_is_not_emitted() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![defaulted_input_def(
            "greeting",
            InputType::String,
            None,
            toml::Value::String("hi".to_string()),
        )],
    );

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");

    assert_eq!(result.exit_code, 0);
    let argv = argv_dump_contents(&fx.argv_dump);
    assert!(
        !argv.iter().any(|t| t == "hi"),
        "default without flag must not be emitted; argv={argv:?}"
    );
}

#[test]
fn default_value_with_flag_emits_string_default() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![defaulted_input_def(
            "size",
            InputType::String,
            Some("--size"),
            toml::Value::String("large".to_string()),
        )],
    );

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");

    let argv = argv_dump_contents(&fx.argv_dump);
    assert_eq!(result.exit_code, 0);
    let pos = argv
        .iter()
        .position(|t| t == "--size")
        .expect("--size present");
    assert_eq!(argv[pos + 1], "large", "argv={argv:?}");
}

#[test]
fn default_value_integer_formatted_via_to_string() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![defaulted_input_def(
            "count",
            InputType::Integer {
                min: None,
                max: None,
            },
            Some("--count"),
            toml::Value::Integer(7),
        )],
    );

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");

    assert_eq!(result.exit_code, 0);
    let argv = argv_dump_contents(&fx.argv_dump);
    let pos = argv
        .iter()
        .position(|t| t == "--count")
        .expect("--count present");
    assert_eq!(argv[pos + 1], "7", "argv={argv:?}");
}

#[test]
fn default_value_float_formatted_via_to_string() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![defaulted_input_def(
            "temp",
            InputType::Number {
                min: None,
                max: None,
            },
            Some("--temp"),
            toml::Value::Float(0.5),
        )],
    );

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");

    assert_eq!(result.exit_code, 0);
    let argv = argv_dump_contents(&fx.argv_dump);
    let pos = argv
        .iter()
        .position(|t| t == "--temp")
        .expect("--temp present");
    assert_eq!(argv[pos + 1], "0.5", "argv={argv:?}");
}

#[test]
fn default_value_boolean_formatted_via_to_string() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![defaulted_input_def(
            "verbose",
            InputType::Boolean,
            Some("--verbose"),
            toml::Value::Boolean(true),
        )],
    );

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");

    assert_eq!(result.exit_code, 0);
    let argv = argv_dump_contents(&fx.argv_dump);
    let pos = argv
        .iter()
        .position(|t| t == "--verbose")
        .expect("--verbose present");
    assert_eq!(argv[pos + 1], "true", "argv={argv:?}");
}

// Required-input truth table. The contract pinned here is from
// `cli.rs::input_requires_user_value`:
//   required = !default_input && !extra.contains(name) && default.is_none() && required
#[test]
fn required_truth_table_required_with_extra_value_passes() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![required_input_def(
            "image",
            InputType::String,
            Some("--image"),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert("image".to_string(), vec!["cat.png".to_string()]);

    let result = execute(&model, 0, "prompt", None, &inputs, None).expect("execute");
    assert_eq!(result.exit_code, 0);
}

#[test]
fn required_truth_table_required_with_default_passes() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![InputDef {
            required: true,
            ..defaulted_input_def(
                "image",
                InputType::String,
                Some("--image"),
                toml::Value::String("default.png".to_string()),
            )
        }],
    );

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);
}

#[test]
fn required_truth_table_required_with_default_input_passes() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![InputDef {
            required: true,
            default_input: true,
            ..input_def("image", InputType::String, None)
        }],
    );

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);
}

#[test]
fn required_truth_table_required_without_any_value_rejects() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![required_input_def(
            "image",
            InputType::String,
            Some("--image"),
        )],
    );

    let err = execute(&model, 0, "prompt", None, &HashMap::new(), None).unwrap_err();
    assert_eq!(err, "Required input 'image' not provided");
}

#[test]
fn input_requires_user_value_when_required_without_default_or_extra() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![required_input_def(
            "alpha",
            InputType::String,
            Some("--alpha"),
        )],
    );

    let err = execute(&model, 0, "prompt", None, &HashMap::new(), None).unwrap_err();
    assert_eq!(err, "Required input 'alpha' not provided");
}

#[test]
fn input_requires_user_value_short_circuits_on_default_input_flag() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![InputDef {
            required: true,
            default_input: true,
            ..input_def("alpha", InputType::String, Some("--alpha"))
        }],
    );

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None);
    assert!(result.is_ok(), "result={:?}", result.err());
}

#[test]
fn input_requires_user_value_short_circuits_when_default_value_present() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![InputDef {
            required: true,
            ..defaulted_input_def(
                "alpha",
                InputType::String,
                Some("--alpha"),
                toml::Value::String("filler".to_string()),
            )
        }],
    );

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None);
    assert!(result.is_ok(), "result={:?}", result.err());
}

#[test]
fn input_requires_user_value_short_circuits_when_extra_input_supplied() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(
        &fx.script_path,
        vec![required_input_def(
            "alpha",
            InputType::String,
            Some("--alpha"),
        )],
    );
    let mut inputs = HashMap::new();
    inputs.insert("alpha".to_string(), vec!["supplied".to_string()]);

    let result = execute(&model, 0, "prompt", None, &inputs, None);
    assert!(result.is_ok(), "result={:?}", result.err());
}

#[test]
fn unknown_input_passes_through_as_double_dash_key() {
    let fx = argv_dump_fixture();
    let model = model_with_inputs(&fx.script_path, vec![]);
    let mut inputs = HashMap::new();
    inputs.insert("custom".to_string(), vec!["value".to_string()]);

    let result = execute(&model, 0, "prompt", None, &inputs, None).expect("execute");

    assert_eq!(result.exit_code, 0);
    let argv = argv_dump_contents(&fx.argv_dump);
    let pos = argv
        .iter()
        .position(|t| t == "--custom")
        .expect("--custom present");
    assert_eq!(argv[pos + 1], "value", "argv={argv:?}");
}

// Provider-index out-of-range error is pinned at execute() (public).
#[test]
fn execute_provider_index_out_of_range_returns_canonical_error() {
    let fx = argv_dump_fixture();
    let model = ModelConfig {
        name: "single-provider-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::new(
            fx.script_path.to_string_lossy().into_owned(),
            Vec::new(),
        )],
        inputs: Vec::new(),
        provider: None,
    };

    let err = execute(&model, 7, "prompt", None, &HashMap::new(), None).unwrap_err();
    assert_eq!(
        err,
        "Provider index 7 out of range for model single-provider-model"
    );
}

// Tie-in: resume payload also uses input flag resolution? execute_resume does
// NOT go through input_args (cli.rs:1934 passes &[]). This test characterizes
// that: a model with a required input still permits execute_resume because
// the resume entrypoint does not consult model inputs.
#[test]
fn execute_resume_does_not_require_model_inputs() {
    let fx = argv_dump_fixture();
    let provider = ProviderConfig::new(fx.script_path.to_string_lossy().into_owned(), Vec::new());
    let strategy = ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    };

    let result = oulipoly_runtime::executor::cli::execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        oulipoly_runtime::executor::cli::ResumePayload {
            session_id: "5169694d-de0f-40d1-890c-6e28e55bab27",
            strategy: &strategy,
        },
    )
    .expect("resume execute");

    assert_eq!(result.exit_code, 0);
}
