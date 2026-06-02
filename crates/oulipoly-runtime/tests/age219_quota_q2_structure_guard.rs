use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn quota_q2_facade_declares_modules_and_reexports_public_surface() {
    let source = read("src/quota/mod.rs");

    for module in ["source", "adapter_derived_source", "refresh"] {
        assert!(
            source.contains(&format!("mod {module};")),
            "quota facade must declare Q2 module {module}"
        );
    }

    assert!(
        source.contains("pub use refresh::") || source.contains("pub use self::refresh::"),
        "quota facade must re-export refresh public surface from quota::refresh"
    );
    assert!(
        source.contains("pub use source::") || source.contains("pub use self::source::"),
        "quota facade must re-export source public surface from quota::source"
    );

    for public_item in [
        "RuntimeQuotaService",
        "refresh_provider",
        "refresh_provider_for_routing",
        "has_refresh_source",
    ] {
        assert!(
            source.contains(public_item),
            "quota facade must preserve public import path for {public_item}"
        );
    }
}

#[test]
fn quota_q2_source_module_owns_refresh_source_resolution_surface() {
    let source = read("src/quota/source.rs");

    for required in [
        "struct RefreshSource",
        "fn refresh_source",
        "fn has_refresh_source",
    ] {
        assert!(
            source.contains(required),
            "quota::source must own source-resolution symbol {required}"
        );
    }
}

#[test]
fn quota_q2_adapter_derived_source_module_owns_labeled_credential_island() {
    let source = read("src/quota/adapter_derived_source.rs");

    assert!(
        source.contains("S10/S11") && source.contains("provider-extraction"),
        "adapter-derived quota module must be labeled as the S10/S11 provider-extraction move candidate"
    );

    for required in [
        "fn derived_quota_script_from_provider_entry",
        "fn derived_quota_script_from_adapter_command",
        "fn shell_word_arg",
        "anthropic-usage",
        "chatgpt-usage",
        ".credentials.json",
        "auth.json",
    ] {
        assert!(
            source.contains(required),
            "adapter-derived quota module must preserve credential-island symbol or literal {required}"
        );
    }
}

#[test]
fn quota_q2_refresh_module_owns_refresh_orchestration_surface() {
    let source = read("src/quota/refresh.rs");

    for required in [
        "struct RuntimeQuotaService",
        "fn refresh_provider",
        "fn refresh_provider_for_routing",
        "fn refresh_provider_from_script",
        "fn should_attempt_auth_refresh",
    ] {
        assert!(
            source.contains(required),
            "quota::refresh must own refresh-orchestration symbol {required}"
        );
    }
}

fn read(relative: &str) -> String {
    let path = manifest_path(relative);
    fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "failed to read required AGE-219 Q2 source file {}: {error}",
            path.display()
        )
    })
}

fn manifest_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}
