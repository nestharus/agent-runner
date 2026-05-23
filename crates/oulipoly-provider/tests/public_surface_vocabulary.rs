//! ## Declared roles
//!
//! Roles: accessor, filter, mapper, predicate, validator.
//!
//! - accessor: [`read_dir_entries`], [`read_source_text`].
//! - filter: [`collect_rs_files`].
//! - mapper: [`normalize`], [`guarded_terms`],
//!   [`disallowed_manifest_terms`].
//! - predicate: [`is_rs_file`], [`is_directory`].
//! - validator: [`assert_no_terms`],
//!   [`provider_source_uses_neutral_vocabulary`],
//!   [`provider_manifest_has_no_internal_crate_dependencies`].
//!
use std::fs;
use std::path::{Path, PathBuf};

fn read_dir_entries(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("failed to read provider source directory {dir:?}: {error}"))
        .map(|entry| {
            entry
                .expect("provider source directory entry should be readable")
                .path()
        })
        .collect()
}

fn is_rs_file(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("rs")
}

fn is_directory(path: &Path) -> bool {
    path.is_dir()
}

fn collect_rs_files(root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();

    while let Some(dir) = pending.pop() {
        for path in read_dir_entries(&dir) {
            if is_directory(&path) {
                pending.push(path);
            } else if is_rs_file(&path) {
                files.push(path);
            }
        }
    }

    files
}

fn read_source_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("failed to read {path:?}: {error}"))
}

fn normalize(text: &str) -> String {
    text.to_lowercase()
}

fn guarded_terms() -> Vec<String> {
    vec![
        ["cla", "ude"].concat(),
        ["co", "de", "x"].concat(),
        ["an", "thro", "pic"].concat(),
        ["op", "en", "ai"].concat(),
    ]
}

fn disallowed_manifest_terms() -> Vec<String> {
    vec![
        "oulipoly-core".to_string(),
        "oulipoly-config".to_string(),
        "oulipoly-state".to_string(),
        "oulipoly-setup".to_string(),
        "oulipoly-agent-store".to_string(),
        "oulipoly-agent-scratchpad".to_string(),
        "oulipoly-agent-messenger".to_string(),
        "tauri".to_string(),
        "tokio".to_string(),
    ]
}

fn assert_no_terms(text: &str, terms: &[String], context: &str) {
    for term in terms {
        assert!(
            !text.contains(term),
            "{context} must not contain disallowed term {term}"
        );
    }
}

// Risk: concrete provider vocabulary could leak into the neutral public surface.
// Level: integration. Source: contract C5.5/C6 and proposal test-intent item 5.
#[test]
fn provider_source_uses_neutral_vocabulary() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    assert!(
        src_dir.is_dir(),
        "provider crate must have a src directory after implementation"
    );

    let files = collect_rs_files(&src_dir);
    assert!(
        !files.is_empty(),
        "provider crate must expose source files after implementation"
    );

    let terms = guarded_terms();
    for path in files {
        let text = read_source_text(&path);
        let normalized = normalize(&text);
        let context = format!("provider source {path:?}");
        assert_no_terms(&normalized, &terms, &context);
    }
}

// Risk: the leaf contract crate could accidentally depend on another internal
// crate, especially the runtime owner of later integration bridges.
// Level: integration. Source: contract C5.6/C6 and proposal test-intent item 6.
#[test]
fn provider_manifest_has_no_internal_crate_dependencies() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifest = read_source_text(&manifest_path);
    let normalized = normalize(&manifest);
    let terms = disallowed_manifest_terms();

    assert_no_terms(&normalized, &terms, "provider manifest");
}
