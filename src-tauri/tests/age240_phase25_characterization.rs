//! ## Declared roles
//! accessor, parser, validator
//!
//! Phase 2.5 characterization guards for AGE-240. These tests capture current
//! behavior before L6 moves ownership out of `lib.rs`.

#![allow(dead_code)]

use agent_runner_lib::test_model_command::{diagnostics_fallback, validator};
use oulipoly_runtime::diagnostics::{Diagnosis, ErrorCategory};
use oulipoly_runtime::services::DiagnosticsServiceOutput;

fn reload_source() -> &'static str {
    include_str!("../src/commands/models/reload.rs")
}

fn app_paths_source() -> &'static str {
    include_str!("../src/app_paths.rs")
}

fn source_block_after(source: &str, needle: &str) -> String {
    let start = source
        .find(needle)
        .unwrap_or_else(|| panic!("missing {needle}"));
    let open = source[start..]
        .find('{')
        .map(|idx| start + idx)
        .unwrap_or_else(|| panic!("missing opening brace for {needle}"));
    let mut depth = 0usize;
    for (offset, ch) in source[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return source[open..=open + offset].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("missing closing brace for {needle}");
}

fn compact(source: &str) -> String {
    source.split_whitespace().collect::<String>()
}

fn assert_order(haystack: &str, first: &str, second: &str, context: &str) {
    let first_idx = haystack
        .find(first)
        .unwrap_or_else(|| panic!("{context}: missing first marker `{first}`"));
    let second_idx = haystack
        .find(second)
        .unwrap_or_else(|| panic!("{context}: missing second marker `{second}`"));
    assert!(
        first_idx < second_idx,
        "{context}: `{first}` must appear before `{second}`"
    );
}

#[test]
fn age240_reload_models_preserves_current_provider_lookup_and_cache_refresh_sequence() {
    let reload = source_block_after(reload_source(), "fn reload_models_inner");
    let reload_compact = compact(&reload);
    let providers_path = source_block_after(
        app_paths_source(),
        "fn providers_config_path_for_models_dir",
    );
    let models_root = source_block_after(app_paths_source(), "fn models_config_root");
    let load_providers =
        source_block_after(app_paths_source(), "fn load_providers_for_models_dir_with");

    assert!(
        reload.contains(
            "load_providers_for_models_dir_with(&state.models_dir, &*state.providers_config)"
        ),
        "reload_models must load providers for the current models_dir through the configured repository"
    );
    assert!(
        providers_path.contains("models_config_root(models_dir).join(\"providers.toml\")"),
        "provider config lookup must remain models_dir parent plus providers.toml"
    );
    assert!(
        models_root.contains(".parent()"),
        "provider config lookup must derive the config root from models_dir.parent()"
    );
    assert!(
        load_providers.contains(".load_providers(&providers_path).unwrap_or_default()"),
        "provider config load failures must currently fall back to default provider config"
    );
    assert!(
        reload.contains(
            "config::load_models(&state.models_dir, Some(&providers)).unwrap_or_default()"
        ),
        "model reload failures must currently fall back to an empty/default model map"
    );
    assert!(
        reload_compact.contains("letmutmodels=state.models.lock().map_err(|e|e.to_string())?;"),
        "poisoned model-cache lock errors must currently surface through Display::to_string()"
    );
    assert!(
        reload_compact.contains("*models=fresh;"),
        "reload_models must replace the entire cached model map with the freshly loaded map"
    );
    assert_order(
        &reload,
        "let providers =",
        "let fresh =",
        "provider config must be loaded before model configs are reloaded",
    );
    assert_order(
        &reload,
        "let fresh =",
        "let mut models =",
        "fresh model configs must be available before locking the model cache",
    );
    assert_order(
        &reload,
        "*models = fresh;",
        "drop(models);",
        "model cache replacement must happen before the cache lock is dropped",
    );
    assert_order(
        &reload,
        "drop(models);",
        "provider_settings::refresh_provider_settings_host(state)?;",
        "provider settings host refresh must run after the model cache lock is released",
    );
}

#[test]
fn age240_diagnostic_input_preserves_combined_stderr_stdout_order_and_empty_input() {
    assert_eq!(
        diagnostics_fallback::diagnostic_input("  stderr quota  ", b"  stdout quota  "),
        "stderr quota\nstdout quota",
        "diagnostic fallback must join trimmed stderr before trimmed stdout"
    );
    assert_eq!(
        diagnostics_fallback::diagnostic_input("  \n\t", b"  \n\t"),
        "",
        "empty diagnostic stderr and stdout must currently produce an empty classifier input"
    );
    assert_eq!(
        diagnostics_fallback::diagnostic_input("", b" stdout only "),
        "stdout only",
        "stdout-only fallback input must remain observable when stderr is empty"
    );
    assert_eq!(
        diagnostics_fallback::diagnostic_input(" stderr only ", b""),
        "stderr only",
        "stderr-only fallback input must remain observable when stdout is empty"
    );
}

#[test]
fn age240_unexpected_diagnostics_output_preserves_current_error_string() {
    let output = DiagnosticsServiceOutput::Diagnosis {
        diagnosis: Diagnosis {
            category: ErrorCategory::Unknown,
            summary: "not an exhaustion classification".to_string(),
        },
    };

    assert_eq!(
        validator::validate_diagnostics_output_variant(output),
        Err("Diagnostics service returned unexpected output".to_string()),
        "unexpected diagnostics output must keep the current user-visible error string"
    );
}
