//! ## Declared roles
//!
//! Roles: filter, parser, predicate, formatter.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/provider_specific/session_capture/telemetry_scrub.rs
//!     role: adapter
//!     Translates:
//!       - provider-telemetry-money-redaction-contract
//!       - executor-public-stdout-contract
//! ```

use serde_json::Value;

pub(in crate::executor) fn remove_unsanctioned_money_fields(stdout: Vec<u8>) -> Vec<u8> {
    let scrubbed = {
        let Some(text) = stdout_as_utf8(&stdout) else {
            return stdout;
        };
        scrub_provider_json_lines(text)
    };
    scrubbed.map(String::into_bytes).unwrap_or(stdout)
}

fn stdout_as_utf8(stdout: &[u8]) -> Option<&str> {
    std::str::from_utf8(stdout).ok()
}

fn scrub_provider_json_lines(text: &str) -> Option<String> {
    let mut scrubbed = String::with_capacity(text.len());
    let mut changed = false;

    for line in text.split_inclusive('\n') {
        append_scrubbed_provider_json_line(&mut scrubbed, &mut changed, line);
    }

    changed.then_some(scrubbed)
}

fn append_scrubbed_provider_json_line(scrubbed: &mut String, changed: &mut bool, line: &str) {
    if let Some(line) = scrub_provider_json_line(line) {
        *changed = true;
        scrubbed.push_str(&line);
    } else {
        scrubbed.push_str(line);
    }
}

fn scrub_provider_json_line(line: &str) -> Option<String> {
    let (content, line_ending) = split_line_ending(line);
    let mut value = parse_provider_json_line(content)?;
    scrub_provider_telemetry_value(&mut value)?;
    render_provider_json_line(&value, line_ending)
}

fn parse_provider_json_line(content: &str) -> Option<Value> {
    serde_json::from_str(content).ok()
}

fn scrub_provider_telemetry_value(value: &mut Value) -> Option<()> {
    (looks_like_provider_telemetry(value) && strip_money_fields(value)).then_some(())
}

fn render_provider_json_line(value: &Value, line_ending: &str) -> Option<String> {
    let mut rendered = serde_json::to_string(value).ok()?;
    rendered.push_str(line_ending);
    Some(rendered)
}

fn split_line_ending(line: &str) -> (&str, &str) {
    let Some(without_lf) = line.strip_suffix('\n') else {
        return (line, "");
    };
    if let Some(without_crlf) = without_lf.strip_suffix('\r') {
        (without_crlf, "\r\n")
    } else {
        (without_lf, "\n")
    }
}

fn looks_like_provider_telemetry(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.get("type").and_then(Value::as_str) == Some("result")
        || object.contains_key("modelUsage")
        || object.keys().any(|key| is_money_field_name(key))
}

fn strip_money_fields(value: &mut Value) -> bool {
    money_fields_were_removed(strip_money_fields_in_place(value))
}

fn strip_money_fields_in_place(value: &mut Value) -> usize {
    match value {
        Value::Object(object) => strip_money_fields_from_object(object),
        Value::Array(items) => strip_money_fields_from_array(items),
        _ => 0,
    }
}

fn strip_money_fields_from_object(object: &mut serde_json::Map<String, Value>) -> usize {
    let keys_to_remove: Vec<String> = object
        .keys()
        .filter(|key| is_money_field_name(key))
        .cloned()
        .collect();
    let removed = keys_to_remove.len();
    remove_money_fields_from_object(object, keys_to_remove);
    removed + strip_money_fields_from_object_values(object)
}

fn remove_money_fields_from_object(
    object: &mut serde_json::Map<String, Value>,
    keys_to_remove: Vec<String>,
) {
    for key in keys_to_remove {
        object.remove(&key);
    }
}

fn strip_money_fields_from_object_values(object: &mut serde_json::Map<String, Value>) -> usize {
    object.values_mut().map(strip_money_fields_in_place).sum()
}

fn strip_money_fields_from_array(items: &mut [Value]) -> usize {
    items.iter_mut().map(strip_money_fields_in_place).sum()
}

fn money_fields_were_removed(removed_count: usize) -> bool {
    removed_count > 0
}

fn is_money_field_name(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    normalized.contains("cost") || normalized.contains("usd") || normalized.contains("price")
}
