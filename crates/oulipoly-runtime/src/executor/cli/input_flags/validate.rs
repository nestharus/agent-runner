//! ## Declared roles
//!
//! Roles: validator.
//!
//! - validator: validates extra inputs, required inputs, scalar bounds, enum
//!   options, and array counts.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/input_flags/validate.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly-config-model-input-schema
//!       - executor-extra-inputs-contract
//!       - input-validation-error-contract
//! ```

use super::messages::{
    array_max_items_message, array_min_items_message, below_minimum_message,
    exceeds_maximum_message, invalid_enum_option_message, required_input_message,
};
use super::parse::{parse_integer_input, parse_number_input};
use super::predicates::input_requires_user_value;
use super::schema_access::input_def_by_name;
use oulipoly_config::{InputDef, InputType, ModelConfig};
use std::collections::HashMap;

pub(super) fn validate_extra_inputs(
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

pub(super) fn validate_required_inputs(
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

fn validate_input_values(values: &[String], input_def: &InputDef) -> Result<(), String> {
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
    input_def: &InputDef,
    options: &[String],
) -> Result<(), String> {
    for val in values {
        if !options.contains(val) {
            return Err(invalid_enum_option_message(&input_def.name, val, options));
        }
    }
    Ok(())
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
