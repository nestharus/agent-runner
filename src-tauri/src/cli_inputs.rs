//! CLI `--input` parsing helpers.
//!
//! ## Declared roles
//!
//! `parser`, `validator`, `mapper`

use std::collections::HashMap;

/// Parse --input key=value flags into a map (repeated keys become arrays).
pub(super) fn parse_inputs(raw: &[String]) -> Result<HashMap<String, Vec<String>>, String> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for entry in raw {
        let (key, value) = parse_key_value(entry)?;
        map.entry(key.to_string())
            .or_default()
            .push(value.to_string());
    }
    Ok(map)
}

fn parse_key_value(entry: &str) -> Result<(&str, &str), String> {
    entry
        .split_once('=')
        .ok_or_else(|| format!("Invalid input format '{}': expected KEY=VALUE", entry))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_inputs_collects_repeated_keys_and_rejects_missing_separator() {
        let parsed = parse_inputs(&[
            "size=large".to_string(),
            "style=flat".to_string(),
            "size=small".to_string(),
            "empty=".to_string(),
        ])
        .unwrap();

        assert_eq!(parsed["size"], vec!["large", "small"]);
        assert_eq!(parsed["style"], vec!["flat"]);
        assert_eq!(parsed["empty"], vec![""]);

        let err = parse_inputs(&["not-key-value".to_string()]).unwrap_err();
        assert_eq!(
            err,
            "Invalid input format 'not-key-value': expected KEY=VALUE"
        );
    }
}
