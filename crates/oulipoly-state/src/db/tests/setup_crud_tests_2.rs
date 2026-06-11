//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn upsert_and_list_model_parameters() {
    let db = test_db();

    let temp_param = ModelParameter {
        name: "temperature".to_string(),
        display_name: "Temperature".to_string(),
        param_type: ParamType::Number {
            min: Some(0.0),
            max: Some(2.0),
        },
        description: "Controls randomness".to_string(),
        cli_mapping: CliMapping {
            flag: "--temperature".to_string(),
            value_template: "{value}".to_string(),
        },
    };

    let model_param = ModelParameter {
        name: "model".to_string(),
        display_name: "Model".to_string(),
        param_type: ParamType::Enum {
            options: vec!["opus-4".to_string(), "sonnet-4".to_string()],
        },
        description: "Model variant to use".to_string(),
        cli_mapping: CliMapping {
            flag: "-m".to_string(),
            value_template: "{value}".to_string(),
        },
    };

    db.upsert_model_parameter("claude-opus-4", "claude", &temp_param)
        .unwrap();
    db.upsert_model_parameter("claude-opus-4", "claude", &model_param)
        .unwrap();

    let params = db.list_model_parameters("claude-opus-4", "claude").unwrap();
    assert_eq!(params.len(), 2);
    // Ordered by name
    assert_eq!(params[0].name, "model");
    assert_eq!(params[1].name, "temperature");

    // Verify ParamType round-trip
    match &params[0].param_type {
        ParamType::Enum { options } => {
            assert_eq!(options.len(), 2);
            assert_eq!(options[0], "opus-4");
        }
        other => panic!("Expected Enum, got {:?}", other),
    }

    match &params[1].param_type {
        ParamType::Number { min, max } => {
            assert_eq!(*min, Some(0.0));
            assert_eq!(*max, Some(2.0));
        }
        other => panic!("Expected Number, got {:?}", other),
    }

    // Verify CliMapping round-trip
    assert_eq!(params[1].cli_mapping.flag, "--temperature");
    assert_eq!(params[1].cli_mapping.value_template, "{value}");
}

#[test]
fn upsert_model_parameter_updates_existing() {
    let db = test_db();

    let param = ModelParameter {
        name: "verbose".to_string(),
        display_name: "Verbose".to_string(),
        param_type: ParamType::Boolean,
        description: "Enable verbose output".to_string(),
        cli_mapping: CliMapping {
            flag: "--verbose".to_string(),
            value_template: "".to_string(),
        },
    };
    db.upsert_model_parameter("gpt-5.3", "codex", &param)
        .unwrap();

    // Update description
    let mut updated = param.clone();
    updated.description = "Toggle verbose mode".to_string();
    db.upsert_model_parameter("gpt-5.3", "codex", &updated)
        .unwrap();

    let params = db.list_model_parameters("gpt-5.3", "codex").unwrap();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].description, "Toggle verbose mode");
}

#[test]
fn list_model_parameters_empty() {
    let db = test_db();
    let params = db
        .list_model_parameters("nonexistent", "nonexistent")
        .unwrap();
    assert!(params.is_empty());
}

#[test]
fn param_type_string_variant() {
    let db = test_db();
    let param = ModelParameter {
        name: "system_prompt".to_string(),
        display_name: "System Prompt".to_string(),
        param_type: ParamType::String,
        description: "The system prompt".to_string(),
        cli_mapping: CliMapping {
            flag: "--system".to_string(),
            value_template: "{value}".to_string(),
        },
    };
    db.upsert_model_parameter("m", "p", &param).unwrap();
    let params = db.list_model_parameters("m", "p").unwrap();
    assert_eq!(params[0].param_type, ParamType::String);
}
