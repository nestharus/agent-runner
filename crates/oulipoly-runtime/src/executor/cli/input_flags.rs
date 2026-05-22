//! ## Declared roles
//!
//! Roles: orchestration, validator, parser, formatter, predicate, accessor,
//! filter.
//!
//! - orchestration: [`resolve_input_flags`] composes validation over (a)
//!   typed parsers, (b) bound validators, (c) array-count validators, and
//!   then dispatches to the formatter helpers.
//! - validator: [`validate_extra_inputs`], [`validate_required_inputs`],
//!   [`validate_integer_input_value`], [`validate_number_input_value`],
//!   [`validate_array_count`].
//! - parser: [`parse_integer_input`], [`parse_number_input`].
//! - formatter: [`format_input_flags`], [`append_extra_input_flags`],
//!   [`append_default_input_flags`], [`append_repeated_flag_values`],
//!   [`fallback_input_flag`], [`toml_value_to_string`], and the input error
//!   message helpers.
//! - predicate: [`input_requires_user_value`],
//!   [`default_input_should_be_appended`].
//! - accessor: [`input_def_by_name`], [`default_input_flag_and_value`].
//! - filter: [`default_input_defs`].
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/input_flags.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly-config-model-input-schema
//!       - toml-default-value-contract
//!       - executor-extra-inputs-contract
//! ```

use oulipoly_config::{InputDef, InputType, ModelConfig};
use std::collections::HashMap;
use std::fmt::Display;

/// Map user-provided inputs to CLI flag arguments based on the model's input
/// schema. Orchestrates validation and formatting; preserves unknown-input
/// passthrough behavior.
pub(super) fn resolve_input_flags(
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<Vec<String>, String> {
    validate_extra_inputs(model, extra_inputs)?;
    validate_required_inputs(model, extra_inputs)?;
    Ok(format_input_flags(model, extra_inputs))
}

fn validate_extra_inputs(
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<(), String> {
    for (key, values) in extra_inputs {
        if let Some(input_def) = input_def_by_name(model, key) {
            validate_input_values(values, input_def)?;
        }
    }
    Ok(())
}

fn validate_required_inputs(
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Result<(), String> {
    for input_def in &model.inputs {
        if input_requires_user_value(input_def, extra_inputs) {
            return Err(required_input_message(&input_def.name));
        }
    }
    Ok(())
}

fn input_requires_user_value(
    input_def: &oulipoly_config::InputDef,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> bool {
    !input_def.default_input
        && !extra_inputs.contains_key(&input_def.name)
        && input_def.default.is_none()
        && input_def.required
}

fn required_input_message(name: &str) -> String {
    format!("Required input '{name}' not provided")
}

fn format_input_flags(
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut args = Vec::new();
    append_extra_input_flags(&mut args, model, extra_inputs);
    append_default_input_flags(&mut args, model, extra_inputs);
    args
}

fn append_extra_input_flags(
    args: &mut Vec<String>,
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) {
    for (key, values) in extra_inputs {
        let flag = input_def_by_name(model, key)
            .and_then(|def| def.flag.clone())
            .unwrap_or_else(|| fallback_input_flag(key));
        append_repeated_flag_values(args, &flag, values);
    }
}

fn append_default_input_flags(
    args: &mut Vec<String>,
    model: &ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) {
    for input_def in default_input_defs(model, extra_inputs) {
        append_default_input_flag(args, input_def);
    }
}

fn input_def_by_name<'a>(
    model: &'a ModelConfig,
    name: &str,
) -> Option<&'a oulipoly_config::InputDef> {
    model.inputs.iter().find(|input| input.name == name)
}

fn default_input_defs<'a>(
    model: &'a ModelConfig,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> Vec<&'a InputDef> {
    model
        .inputs
        .iter()
        .filter(|input_def| default_input_should_be_appended(input_def, extra_inputs))
        .collect()
}

fn default_input_should_be_appended(
    input_def: &InputDef,
    extra_inputs: &HashMap<String, Vec<String>>,
) -> bool {
    !input_def.default_input && !extra_inputs.contains_key(&input_def.name)
}

fn append_default_input_flag(args: &mut Vec<String>, input_def: &InputDef) {
    if let Some((flag, default)) = default_input_flag_and_value(input_def) {
        append_default_flag_value(args, flag, default);
    }
}

fn default_input_flag_and_value(input_def: &InputDef) -> Option<(&str, &toml::Value)> {
    input_def.flag.as_deref().zip(input_def.default.as_ref())
}

fn append_default_flag_value(args: &mut Vec<String>, flag: &str, default: &toml::Value) {
    args.push(flag.to_string());
    args.push(toml_value_to_string(default));
}

fn fallback_input_flag(name: &str) -> String {
    format!("--{name}")
}

fn append_repeated_flag_values(args: &mut Vec<String>, flag: &str, values: &[String]) {
    for val in values {
        args.push(flag.to_string());
        args.push(val.clone());
    }
}

fn validate_input_values(
    values: &[String],
    input_def: &oulipoly_config::InputDef,
) -> Result<(), String> {
    match &input_def.input_type {
        InputType::Enum { options } => validate_enum_input(values, input_def, options),
        InputType::Integer { min, max } => {
            for val in values {
                let n = parse_integer_input(val, &input_def.name)?;
                validate_integer_input_value(n, *min, *max, &input_def.name)?;
            }
            Ok(())
        }
        InputType::Number { min, max } => {
            for val in values {
                let n = parse_number_input(val, &input_def.name)?;
                validate_number_input_value(n, *min, *max, &input_def.name)?;
            }
            Ok(())
        }
        InputType::Array {
            min_items,
            max_items,
            ..
        } => validate_array_count(values, *min_items, *max_items, &input_def.name),
        _ => Ok(()),
    }
}

fn validate_enum_input(
    values: &[String],
    input_def: &oulipoly_config::InputDef,
    options: &[String],
) -> Result<(), String> {
    for val in values {
        if !options.contains(val) {
            return Err(invalid_enum_option_message(&input_def.name, val, options));
        }
    }
    Ok(())
}

fn parse_integer_input(val: &str, name: &str) -> Result<i64, String> {
    val.parse().map_err(|_| invalid_integer_message(name, val))
}

fn parse_number_input(val: &str, name: &str) -> Result<f64, String> {
    val.parse().map_err(|_| invalid_number_message(name, val))
}

fn validate_integer_input_value(
    n: i64,
    min: Option<i64>,
    max: Option<i64>,
    name: &str,
) -> Result<(), String> {
    if let Some(min_val) = min
        && n < min_val
    {
        return Err(below_minimum_message(name, n, min_val));
    }
    if let Some(max_val) = max
        && n > max_val
    {
        return Err(exceeds_maximum_message(name, n, max_val));
    }
    Ok(())
}

fn validate_number_input_value(
    n: f64,
    min: Option<f64>,
    max: Option<f64>,
    name: &str,
) -> Result<(), String> {
    if let Some(min_val) = min
        && n < min_val
    {
        return Err(below_minimum_message(name, n, min_val));
    }
    if let Some(max_val) = max
        && n > max_val
    {
        return Err(exceeds_maximum_message(name, n, max_val));
    }
    Ok(())
}

fn validate_array_count(
    values: &[String],
    min_items: Option<usize>,
    max_items: Option<usize>,
    name: &str,
) -> Result<(), String> {
    if let Some(min) = min_items
        && values.len() < min
    {
        return Err(array_min_items_message(name, min, values.len()));
    }
    if let Some(max) = max_items
        && values.len() > max
    {
        return Err(array_max_items_message(name, max, values.len()));
    }
    Ok(())
}

fn invalid_enum_option_message(name: &str, value: &str, options: &[String]) -> String {
    format!("Input '{name}': '{value}' is not a valid option. Valid: {options:?}")
}

fn invalid_integer_message(name: &str, value: &str) -> String {
    format!("Input '{name}': '{value}' is not a valid integer")
}

fn invalid_number_message(name: &str, value: &str) -> String {
    format!("Input '{name}': '{value}' is not a valid number")
}

fn below_minimum_message<T: Display>(name: &str, value: T, minimum: T) -> String {
    format!("Input '{name}': {value} is below minimum {minimum}")
}

fn exceeds_maximum_message<T: Display>(name: &str, value: T, maximum: T) -> String {
    format!("Input '{name}': {value} exceeds maximum {maximum}")
}

fn array_min_items_message(name: &str, minimum: usize, actual: usize) -> String {
    format!("Input '{name}': need at least {minimum} items, got {actual}")
}

fn array_max_items_message(name: &str, maximum: usize, actual: usize) -> String {
    format!("Input '{name}': maximum {maximum} items, got {actual}")
}

fn toml_value_to_string(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        other => other.to_string(),
    }
}
