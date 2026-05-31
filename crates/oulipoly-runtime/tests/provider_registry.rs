//! Declared roles: accessor, predicate, validator, parser, mapper, formatter, orchestration.

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig,
    provider_implementation_ref::{ProviderImplementationRef, ProviderImplementationRefError},
};
use oulipoly_provider::generated::{
    CONTRACT_VERSION, DescribeCapabilities, DescribeResult, ErrorCategory,
};
use oulipoly_provider::resolver::{ProviderArtifactRef, RuntimeDisabledArtifact};
use oulipoly_runtime::provider_registry::{
    ArtifactKind, ProviderRegistry, ProviderRegistryError, ProviderRegistryOptions,
    RuntimeProviderArtifact,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn model(name: &str, provider: Option<ProviderImplementationRef>) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider("provider-a", Vec::new())],
        inputs: Vec::new(),
        provider,
    }
}

fn path_ref(path: impl Into<String>) -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: Some(path.into()),
        crate_name: None,
        version: None,
        binary: None,
        script: None,
    }
}

fn binary_ref(name: &str) -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: None,
        crate_name: None,
        version: None,
        binary: Some(name.to_string()),
        script: None,
    }
}

fn script_ref(path: impl Into<String>) -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: None,
        crate_name: None,
        version: None,
        binary: None,
        script: Some(path.into()),
    }
}

fn crate_ref(crate_name: &str, version: Option<&str>) -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: None,
        crate_name: Some(crate_name.to_string()),
        version: version.map(str::to_string),
        binary: None,
        script: None,
    }
}

fn invalid_multiple_flavor_ref() -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: Some("/tmp/example-provider".to_string()),
        crate_name: None,
        version: None,
        binary: Some("fake-provider".to_string()),
        script: None,
    }
}

fn invalid_version_without_crate_ref() -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: None,
        crate_name: None,
        version: Some("0.1.0".to_string()),
        binary: Some("fake-provider".to_string()),
        script: None,
    }
}

fn invalid_no_flavor_ref() -> ProviderImplementationRef {
    ProviderImplementationRef {
        path: None,
        crate_name: None,
        version: None,
        binary: None,
        script: None,
    }
}

fn registry_from_single_ref(
    provider: ProviderImplementationRef,
    options: ProviderRegistryOptions,
) -> ProviderRegistry {
    ProviderRegistry::from_model_configs(&[model("example-model", Some(provider))], options)
        .expect("registry construction should succeed before lazy provider lookup")
}

fn describe_result(provider_id: &str) -> DescribeResult {
    DescribeResult {
        provider_id: provider_id.to_string(),
        display_name: "Fake Provider".to_string(),
        contract_versions: vec![CONTRACT_VERSION.to_string()],
        preferred_contract: CONTRACT_VERSION.to_string(),
        capabilities: DescribeCapabilities {
            launch: true,
            policy: false,
            quota: false,
            session: false,
            terminal: false,
            rotation: false,
            discovery: false,
            settings: false,
            setup_brain: false,
            setup: false,
            migration: false,
        },
        settings_schema_id: None,
        concurrency: None,
    }
}

fn write_fake_provider_request_recording_script(
    dir: &Path,
    name: &str,
    counter: &Path,
    request_record: &Path,
    provider_id: &str,
) -> PathBuf {
    let script = dir.join(name);
    initialize_count_file(counter);
    let body = request_recording_script_body(counter, request_record, provider_id);
    materialize_executable_script(&script, body)
}

fn request_recording_script_body(
    counter: &Path,
    request_record: &Path,
    provider_id: &str,
) -> String {
    let counter_literal = path_literal(counter);
    let record_literal = path_literal(request_record);
    format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys

count_file = pathlib.Path({counter_literal})
current = int(count_file.read_text()) if count_file.exists() else 0
count_file.write_text(str(current + 1))
request = json.loads(sys.stdin.read() or "{{}}")
pathlib.Path({record_literal}).write_text(json.dumps(request, sort_keys=True))

response = {{
    "contract": request["contract"],
    "request_id": request["request_id"],
    "ok": True,
    "result": {{
        "provider_id": "{provider_id}",
        "display_name": "Fake Provider",
        "contract_versions": ["{contract}"],
        "preferred_contract": "{contract}",
        "capabilities": {{
            "launch": True,
            "policy": False,
            "quota": False,
            "session": False,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        }},
    }},
}}
print(json.dumps(response))
"#,
        counter_literal = counter_literal,
        record_literal = record_literal,
        contract = CONTRACT_VERSION,
        provider_id = provider_id
    )
}

fn write_fake_provider_script(
    dir: &Path,
    name: &str,
    counter: &Path,
    result: Result<&str, &str>,
) -> PathBuf {
    let script = dir.join(name);
    initialize_count_file(counter);
    let body = fake_provider_script_body(counter, result);
    materialize_executable_script(&script, body)
}

fn write_fake_provider_settings_describe_script(
    dir: &Path,
    name: &str,
    counter: &Path,
    provider_id: &str,
    settings_schema_id: &str,
) -> PathBuf {
    let script = dir.join(name);
    initialize_count_file(counter);
    let body =
        fake_provider_settings_describe_script_body(counter, provider_id, settings_schema_id);
    materialize_executable_script(&script, body)
}

fn fake_provider_settings_describe_script_body(
    counter: &Path,
    provider_id: &str,
    settings_schema_id: &str,
) -> String {
    let counter_literal = path_literal(counter);
    format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys

count_file = pathlib.Path({counter_literal})
current = int(count_file.read_text()) if count_file.exists() else 0
count_file.write_text(str(current + 1))
request = json.loads(sys.stdin.read() or "{{}}")
if len(sys.argv) < 2 or sys.argv[1] != "describe":
    response = {{
        "contract": request.get("contract", "{contract}"),
        "request_id": request.get("request_id", "request-example-001"),
        "ok": False,
        "error": {{
            "category": "unsupported",
            "code": "unsupported_subcommand",
            "retryable": False,
            "message": "unsupported",
        }},
    }}
else:
    response = {{
        "contract": request.get("contract", "{contract}"),
        "request_id": request.get("request_id", "request-example-001"),
        "ok": True,
        "result": {{
            "provider_id": "{provider_id}",
            "display_name": "Fake Provider",
            "contract_versions": ["{contract}"],
            "preferred_contract": "{contract}",
            "capabilities": {{
                "launch": True,
                "policy": False,
                "quota": False,
                "session": False,
                "terminal": False,
                "rotation": False,
                "discovery": False,
                "settings": True,
                "setup_brain": False,
                "setup": False,
                "migration": False,
            }},
            "settings_schema_id": "{settings_schema_id}",
            "concurrency": {{
                "safe_for_parallel_invocation": True,
                "state_locking": "none",
            }},
        }},
    }}
print(json.dumps(response))
"#,
        counter_literal = counter_literal,
        contract = CONTRACT_VERSION,
        provider_id = provider_id,
        settings_schema_id = settings_schema_id
    )
}

fn fake_provider_script_body(counter: &Path, result: Result<&str, &str>) -> String {
    match result {
        Ok(provider_id) => fake_provider_success_script_body(counter, provider_id),
        Err(code) => fake_provider_error_script_body(counter, code),
    }
}

fn fake_provider_success_script_body(counter: &Path, provider_id: &str) -> String {
    let counter_literal = path_literal(counter);
    format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys

count_file = pathlib.Path({counter_literal})
current = int(count_file.read_text()) if count_file.exists() else 0
count_file.write_text(str(current + 1))
request = json.loads(sys.stdin.read() or "{{}}")
if len(sys.argv) < 2 or sys.argv[1] != "describe":
    response = {{
        "contract": request.get("contract", "{contract}"),
        "request_id": request.get("request_id", "request-example-001"),
        "ok": False,
        "error": {{
            "category": "unsupported",
            "code": "unsupported_subcommand",
            "retryable": False,
            "message": "unsupported",
        }},
    }}
else:
    response = {{
        "contract": request.get("contract", "{contract}"),
        "request_id": request.get("request_id", "request-example-001"),
        "ok": True,
        "result": {{
            "provider_id": "{provider_id}",
            "display_name": "Fake Provider",
            "contract_versions": ["{contract}"],
            "preferred_contract": "{contract}",
            "capabilities": {{
                "launch": True,
                "policy": False,
                "quota": False,
                "session": False,
                "terminal": False,
                "rotation": False,
                "discovery": False,
                "settings": False,
                "setup_brain": False,
                "setup": False,
                "migration": False,
            }},
            "concurrency": {{
                "safe_for_parallel_invocation": True,
                "state_locking": "none",
            }},
        }},
    }}
print(json.dumps(response))
"#,
        counter_literal = counter_literal,
        contract = CONTRACT_VERSION,
        provider_id = provider_id
    )
}

fn fake_provider_error_script_body(counter: &Path, code: &str) -> String {
    let counter_literal = path_literal(counter);
    format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys

count_file = pathlib.Path({counter_literal})
current = int(count_file.read_text()) if count_file.exists() else 0
count_file.write_text(str(current + 1))
request = json.loads(sys.stdin.read() or "{{}}")
response = {{
    "contract": request.get("contract", "{contract}"),
    "request_id": request.get("request_id", "request-example-001"),
    "ok": False,
    "error": {{
        "category": "failed",
        "code": "{code}",
        "retryable": True,
        "message": "fake-provider failed",
    }},
}}
print(json.dumps(response))
"#,
        counter_literal = counter_literal,
        contract = CONTRACT_VERSION,
        code = code
    )
}

fn write_fake_provider_raw_output_script(
    dir: &Path,
    name: &str,
    counter: &Path,
    stdout_body: &str,
) -> PathBuf {
    let script = dir.join(name);
    initialize_count_file(counter);
    let body = raw_output_script_body(counter, stdout_body);
    materialize_executable_script(&script, body)
}

fn raw_output_script_body(counter: &Path, stdout_body: &str) -> String {
    let counter_literal = path_literal(counter);
    let stdout_literal = serde_json::to_string(stdout_body).expect("stdout body serializes");
    format!(
        r#"#!/usr/bin/env python3
import pathlib

count_file = pathlib.Path({counter_literal})
current = int(count_file.read_text()) if count_file.exists() else 0
count_file.write_text(str(current + 1))
print({stdout_literal})
"#,
        counter_literal = counter_literal,
        stdout_literal = stdout_literal
    )
}

fn write_fake_provider_schema_invalid_script(dir: &Path, name: &str, counter: &Path) -> PathBuf {
    let script = dir.join(name);
    initialize_count_file(counter);
    let body = schema_invalid_script_body(counter);
    materialize_executable_script(&script, body)
}

fn schema_invalid_script_body(counter: &Path) -> String {
    let counter_literal = path_literal(counter);
    format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys

count_file = pathlib.Path({counter_literal})
current = int(count_file.read_text()) if count_file.exists() else 0
count_file.write_text(str(current + 1))
request = json.loads(sys.stdin.read() or "{{}}")
response = {{
    "contract": request.get("contract", "{contract}"),
    "request_id": request.get("request_id", "request-example-001"),
    "ok": True,
    "result": {{
        "display_name": "Fake Provider"
    }},
}}
print(json.dumps(response))
"#,
        counter_literal = counter_literal,
        contract = CONTRACT_VERSION
    )
}

fn initialize_count_file(counter: &Path) {
    fs::write(counter, "0").expect("fake provider counter should be initialized");
}

fn path_literal(path: &Path) -> String {
    serde_json::to_string(&path.display().to_string()).expect("path should serialize")
}

fn materialize_executable_script(script: &Path, body: String) -> PathBuf {
    fs::write(script, body).expect("fake provider script should be written");
    make_executable(script);
    script.to_path_buf()
}

fn write_non_executable_file(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, "#!/usr/bin/env python3\nprint('not executable')\n")
        .expect("non-executable fake provider should be written");
    make_non_executable(&path);
    path
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

#[cfg(unix)]
fn make_non_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o644);
    fs::set_permissions(path, permissions).unwrap();
}

#[cfg(not(unix))]
fn make_non_executable(_path: &Path) {}

fn read_count(path: &Path) -> usize {
    parse_count(&count_text(path))
}

fn count_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn parse_count(value: &Option<String>) -> usize {
    value
        .as_deref()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn protected_tree_snapshot(roots: &[&Path]) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    sorted_snapshot_entries(snapshot_entries_for_roots(&existing_snapshot_roots(roots)))
}

fn existing_snapshot_roots<'a>(roots: &[&'a Path]) -> Vec<&'a Path> {
    roots.iter().copied().filter(|root| root.exists()).collect()
}

fn snapshot_entries_for_roots(roots: &[&Path]) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let mut entries = Vec::new();
    for root in roots {
        collect_paths(root, root, &mut entries);
    }
    entries
}

fn sorted_snapshot_entries(
    mut entries: Vec<(PathBuf, Option<Vec<u8>>)>,
) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn collect_paths(root: &Path, current: &Path, out: &mut Vec<(PathBuf, Option<Vec<u8>>)>) {
    let paths = snapshot_dir_paths(current);
    append_snapshot_entries(root, &paths, out);
    collect_child_snapshot_paths(root, &snapshot_child_dirs(&paths), out);
}

fn snapshot_dir_paths(current: &Path) -> Vec<PathBuf> {
    dir_entry_paths(read_snapshot_dir_entries(current))
}

fn read_snapshot_dir_entries(current: &Path) -> Vec<fs::DirEntry> {
    fs::read_dir(current)
        .expect("snapshot directory should be readable")
        .map(|entry| entry.expect("snapshot entry should be readable"))
        .collect()
}

fn dir_entry_paths(entries: Vec<fs::DirEntry>) -> Vec<PathBuf> {
    entries.into_iter().map(|entry| entry.path()).collect()
}

fn append_snapshot_entries(
    root: &Path,
    paths: &[PathBuf],
    out: &mut Vec<(PathBuf, Option<Vec<u8>>)>,
) {
    out.extend(paths.iter().map(|path| snapshot_entry(root, path)));
}

fn snapshot_child_dirs(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter(|path| is_snapshot_directory(path))
        .cloned()
        .collect()
}

fn collect_child_snapshot_paths(
    root: &Path,
    child_dirs: &[PathBuf],
    out: &mut Vec<(PathBuf, Option<Vec<u8>>)>,
) {
    for path in child_dirs {
        collect_paths(root, path, out);
    }
}

fn snapshot_entry(root: &Path, path: &Path) -> (PathBuf, Option<Vec<u8>>) {
    (snapshot_path(root, path), snapshot_bytes(path))
}

fn snapshot_path(root: &Path, path: &Path) -> PathBuf {
    let relative = path.strip_prefix(root).unwrap().to_path_buf();
    let snapshot_path = root.file_name().unwrap_or_default().to_owned();
    PathBuf::from(snapshot_path).join(relative)
}

fn snapshot_bytes(path: &Path) -> Option<Vec<u8>> {
    if path.is_file() {
        Some(fs::read(path).expect("snapshot file should be readable"))
    } else {
        None
    }
}

fn is_snapshot_directory(path: &Path) -> bool {
    path.is_dir()
}

fn assert_transport_kind(error: ProviderRegistryError, expected_kind: &str) {
    match error {
        ProviderRegistryError::ProviderTransport { kind, .. } => {
            assert_eq!(kind, expected_kind);
        }
        other => panic!("expected ProviderTransport({expected_kind}), got {other:?}"),
    }
}

fn assert_protocol_kind(error: ProviderRegistryError, expected_kind: &str) {
    match error {
        ProviderRegistryError::ProviderProtocol { kind, .. } => {
            assert_eq!(kind, expected_kind);
        }
        other => panic!("expected ProviderProtocol({expected_kind}), got {other:?}"),
    }
}

fn assert_runtime_disabled(error: ProviderRegistryError) {
    match error {
        ProviderRegistryError::RuntimeDisabledArtifact { kind, .. } => {
            assert_eq!(kind, "runtime_disabled");
        }
        other => panic!("expected RuntimeDisabledArtifact(runtime_disabled), got {other:?}"),
    }
}

fn assert_provider_describe_failed(error: ProviderRegistryError, expected_code: &str) {
    match error {
        ProviderRegistryError::ProviderDescribeFailed {
            category,
            code,
            source,
            ..
        } => {
            assert_eq!(category, ErrorCategory::Failed);
            assert_eq!(code, expected_code);
            assert_eq!(source.provider_category(), Some(ErrorCategory::Failed));
            assert_eq!(source.provider_error_code(), Some(expected_code));
        }
        other => panic!("expected ProviderDescribeFailed({expected_code}), got {other:?}"),
    }
}

fn assert_invalid_implementation_ref_source(
    error: ProviderRegistryError,
    expected: ProviderImplementationRefError,
) {
    match error {
        ProviderRegistryError::InvalidImplementationRef { source, .. } => {
            assert_eq!(source, expected);
        }
        other => panic!("expected InvalidImplementationRef({expected:?}), got {other:?}"),
    }
}

#[test]
fn construction_ignores_models_without_provider_refs() {
    let registry = ProviderRegistry::from_model_configs(
        &[
            model("example-model", Some(binary_ref("fake-provider"))),
            model("example-model-without-provider", None),
        ],
        ProviderRegistryOptions::default(),
    )
    .expect("valid configured refs should construct a registry");

    assert_eq!(registry.configured_artifact_keys().len(), 1);
    assert!(registry.artifact_key_for_model("example-model").is_some());
    assert!(
        registry
            .artifact_key_for_model("example-model-without-provider")
            .is_none(),
        "models without provider implementation refs must not create registry artifacts"
    );
}

#[test]
fn enabled_ref_flavors_resolve_describe_and_parse_capability_description() {
    let temp = tempfile::tempdir().unwrap();

    let path_count = temp.path().join("path-count");
    let path_fake = write_fake_provider_script(
        temp.path(),
        "path-provider",
        &path_count,
        Ok("path-provider"),
    );
    let path_registry = registry_from_single_ref(
        path_ref(path_fake.display().to_string()),
        ProviderRegistryOptions::default(),
    );
    let path_result = path_registry
        .describe_model_provider("example-model")
        .expect("path provider should resolve and describe");
    assert_eq!(path_result.provider_id, "path-provider");
    assert_eq!(path_result.display_name, "Fake Provider");
    assert!(path_result.capabilities.launch);
    assert_eq!(read_count(&path_count), 1);

    let binary_count = temp.path().join("binary-count");
    write_fake_provider_script(
        temp.path(),
        "binary-provider",
        &binary_count,
        Ok("binary-provider"),
    );
    let binary_registry = registry_from_single_ref(
        binary_ref("binary-provider"),
        ProviderRegistryOptions::default().with_path_entries([temp.path().to_path_buf()]),
    );
    let binary_result = binary_registry
        .describe_model_provider("example-model")
        .expect("binary provider should resolve through configured path entries");
    assert_eq!(binary_result.provider_id, "binary-provider");
    assert!(binary_result.capabilities.launch);
    assert_eq!(read_count(&binary_count), 1);

    let script_count = temp.path().join("script-count");
    let script_fake = write_fake_provider_script(
        temp.path(),
        "script-provider",
        &script_count,
        Ok("script-provider"),
    );
    let script_registry = registry_from_single_ref(
        script_ref(script_fake.display().to_string()),
        ProviderRegistryOptions::default(),
    );
    let script_result = script_registry
        .describe_model_provider("example-model")
        .expect("script provider should resolve and describe");
    assert_eq!(script_result.provider_id, "script-provider");
    assert!(script_result.capabilities.launch);
    assert_eq!(read_count(&script_count), 1);
}

#[test]
fn describe_propagates_settings_capability_and_schema_id() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("describe-count");
    let fake = write_fake_provider_settings_describe_script(
        temp.path(),
        "fake-provider",
        &count,
        "fake-provider",
        "example.settings/v1",
    );

    let registry = registry_from_single_ref(
        path_ref(fake.display().to_string()),
        ProviderRegistryOptions::default(),
    );
    let result = registry
        .describe_model_provider("example-model")
        .expect("settings-capable provider should describe successfully");

    assert_eq!(result.provider_id, "fake-provider");
    assert!(result.capabilities.settings);
    assert_eq!(
        result.settings_schema_id.as_deref(),
        Some("example.settings/v1")
    );
    assert_eq!(read_count(&count), 1);
}

#[test]
fn conversion_accepts_all_configured_flavors_and_marks_crate_runtime_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("example-provider");
    let script = temp.path().join("example-provider.sh");

    let path_artifact = ProviderRegistry::convert_ref(&path_ref(path.display().to_string()))
        .expect("path refs should convert");
    assert_eq!(path_artifact.kind(), ArtifactKind::Path);
    assert!(matches!(
        path_artifact,
        RuntimeProviderArtifact::Enabled(ProviderArtifactRef::Path { .. })
    ));

    let binary_artifact =
        ProviderRegistry::convert_ref(&binary_ref("fake-provider")).expect("binary refs convert");
    assert_eq!(binary_artifact.kind(), ArtifactKind::Binary);
    assert!(matches!(
        binary_artifact,
        RuntimeProviderArtifact::Enabled(ProviderArtifactRef::Binary { .. })
    ));

    let script_artifact = ProviderRegistry::convert_ref(&script_ref(script.display().to_string()))
        .expect("script refs should convert");
    assert_eq!(script_artifact.kind(), ArtifactKind::Script);
    assert!(matches!(
        script_artifact,
        RuntimeProviderArtifact::Enabled(ProviderArtifactRef::Script { .. })
    ));

    let crate_artifact =
        ProviderRegistry::convert_ref(&crate_ref("example-provider", Some("0.1.0")))
            .expect("crate refs should convert to a runtime-disabled artifact");
    assert_eq!(crate_artifact.kind(), ArtifactKind::Crate);
    assert!(matches!(
        crate_artifact,
        RuntimeProviderArtifact::RuntimeDisabled(RuntimeDisabledArtifact::Crate { .. })
    ));
}

#[test]
fn conversion_rejects_invalid_refs_without_panic() {
    let multiple = ProviderRegistry::convert_ref(&invalid_multiple_flavor_ref())
        .expect_err("multiple provider implementation flavors should be rejected");
    assert!(matches!(
        multiple,
        ProviderRegistryError::InvalidImplementationRef {
            source: ProviderImplementationRefError::MultipleFlavors(_),
            ..
        }
    ));

    let version_without_crate = ProviderRegistry::convert_ref(&invalid_version_without_crate_ref())
        .expect_err("version without crate should be rejected");
    assert_invalid_implementation_ref_source(
        version_without_crate,
        ProviderImplementationRefError::VersionWithoutCrate,
    );

    let no_flavor = ProviderRegistry::convert_ref(&invalid_no_flavor_ref())
        .expect_err("empty implementation refs should be rejected");
    assert_invalid_implementation_ref_source(no_flavor, ProviderImplementationRefError::NoFlavor);
}

#[test]
fn enabled_flavor_resolution_errors_map_to_exact_transport_kinds() {
    let temp = tempfile::tempdir().unwrap();
    let missing_path = temp.path().join("missing-path-provider");
    let missing_script = temp.path().join("missing-script-provider");

    let path_error = registry_from_single_ref(
        path_ref(missing_path.display().to_string()),
        ProviderRegistryOptions::default(),
    )
    .describe_model_provider("example-model")
    .expect_err("missing path artifact should fail at lookup");
    assert_transport_kind(path_error, "missing_artifact");

    let binary_missing_error = registry_from_single_ref(
        binary_ref("missing-binary-provider"),
        ProviderRegistryOptions::default().with_path_entries([temp.path().to_path_buf()]),
    )
    .describe_model_provider("example-model")
    .expect_err("missing binary artifact should fail at lookup");
    assert_transport_kind(binary_missing_error, "missing_artifact");

    let binary_invalid_error = registry_from_single_ref(
        binary_ref("../fake-provider"),
        ProviderRegistryOptions::default().with_path_entries([temp.path().to_path_buf()]),
    )
    .describe_model_provider("example-model")
    .expect_err("invalid binary artifact names should fail at lookup");
    assert_transport_kind(binary_invalid_error, "invalid_binary_name");

    let script_error = registry_from_single_ref(
        script_ref(missing_script.display().to_string()),
        ProviderRegistryOptions::default(),
    )
    .describe_model_provider("example-model")
    .expect_err("missing script artifact should fail at lookup");
    assert_transport_kind(script_error, "missing_artifact");
}

#[cfg(unix)]
#[test]
fn non_executable_enabled_artifacts_map_to_exact_transport_kind() {
    let temp = tempfile::tempdir().unwrap();
    let path_fake = write_non_executable_file(temp.path(), "path-provider");
    let script_fake = write_non_executable_file(temp.path(), "script-provider");
    let _binary_fake = write_non_executable_file(temp.path(), "binary-provider");

    let path_error = registry_from_single_ref(
        path_ref(path_fake.display().to_string()),
        ProviderRegistryOptions::default(),
    )
    .describe_model_provider("example-model")
    .expect_err("non-executable path artifact should fail at lookup");
    assert_transport_kind(path_error, "not_executable");

    let binary_error = registry_from_single_ref(
        binary_ref("binary-provider"),
        ProviderRegistryOptions::default().with_path_entries([temp.path().to_path_buf()]),
    )
    .describe_model_provider("example-model")
    .expect_err("non-executable binary artifact should fail at lookup");
    assert_transport_kind(binary_error, "not_executable");

    let script_error = registry_from_single_ref(
        script_ref(script_fake.display().to_string()),
        ProviderRegistryOptions::default(),
    )
    .describe_model_provider("example-model")
    .expect_err("non-executable script artifact should fail at lookup");
    assert_transport_kind(script_error, "not_executable");
}

#[test]
fn construction_with_executable_provider_does_not_describe_until_lookup() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("describe-count");
    let fake =
        write_fake_provider_script(temp.path(), "fake-provider", &count, Ok("fake-provider"));

    let registry = ProviderRegistry::from_model_configs(
        &[model(
            "example-model",
            Some(path_ref(fake.display().to_string())),
        )],
        ProviderRegistryOptions::default(),
    )
    .expect("registry construction should succeed without describing providers");

    assert_eq!(
        read_count(&count),
        0,
        "constructing a registry with an executable provider must not invoke describe"
    );

    registry
        .describe_model_provider("example-model")
        .expect("first lookup should describe the configured provider");
    assert_eq!(read_count(&count), 1);

    registry
        .describe_model_provider("example-model")
        .expect("second lookup should use the successful describe cache");
    assert_eq!(
        read_count(&count),
        1,
        "successful describe must be cached after the first lookup"
    );
}

#[test]
fn duplicate_equivalent_refs_deduplicate_to_one_artifact_and_one_describe_lookup() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("describe-count");
    let fake =
        write_fake_provider_script(temp.path(), "fake-provider", &count, Ok("fake-provider"));

    let registry = ProviderRegistry::from_model_configs(
        &[
            model("example-model", Some(path_ref(fake.display().to_string()))),
            model(
                "example-model-copy",
                Some(path_ref(fake.display().to_string())),
            ),
        ],
        ProviderRegistryOptions::default(),
    )
    .expect("duplicate equivalent refs should not fail construction");

    assert_eq!(
        registry.configured_artifact_keys().len(),
        1,
        "equivalent refs should share one deterministic artifact key"
    );
    assert_eq!(
        registry
            .artifact_key_for_model("example-model")
            .expect("first model should be configured"),
        registry
            .artifact_key_for_model("example-model-copy")
            .expect("second model should be configured")
    );

    registry
        .describe_model_provider("example-model")
        .expect("first describe should succeed");
    registry
        .describe_model_provider("example-model-copy")
        .expect("duplicate describe should use the same cached artifact");

    assert_eq!(read_count(&count), 1);
}

#[test]
fn duplicate_binary_refs_deduplicate_to_one_artifact_and_one_describe_lookup() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("describe-count");
    write_fake_provider_script(temp.path(), "fake-provider", &count, Ok("fake-provider"));

    let registry = ProviderRegistry::from_model_configs(
        &[
            model("example-model", Some(binary_ref("fake-provider"))),
            model("example-model-copy", Some(binary_ref("fake-provider"))),
        ],
        ProviderRegistryOptions::default().with_path_entries([temp.path().to_path_buf()]),
    )
    .expect("duplicate equivalent binary refs should not fail construction");

    assert_eq!(registry.configured_artifact_keys().len(), 1);
    assert_eq!(
        registry.artifact_key_for_model("example-model"),
        registry.artifact_key_for_model("example-model-copy")
    );
    registry
        .describe_model_provider("example-model")
        .expect("first binary describe should succeed");
    registry
        .describe_model_provider("example-model-copy")
        .expect("duplicate binary describe should use the same cached artifact");
    assert_eq!(read_count(&count), 1);
}

#[test]
fn duplicate_script_refs_deduplicate_to_one_artifact_and_one_describe_lookup() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("describe-count");
    let fake =
        write_fake_provider_script(temp.path(), "fake-provider", &count, Ok("fake-provider"));

    let registry = ProviderRegistry::from_model_configs(
        &[
            model(
                "example-model",
                Some(script_ref(fake.display().to_string())),
            ),
            model(
                "example-model-copy",
                Some(script_ref(fake.display().to_string())),
            ),
        ],
        ProviderRegistryOptions::default(),
    )
    .expect("duplicate equivalent script refs should not fail construction");

    assert_eq!(registry.configured_artifact_keys().len(), 1);
    assert_eq!(
        registry.artifact_key_for_model("example-model"),
        registry.artifact_key_for_model("example-model-copy")
    );
    registry
        .describe_model_provider("example-model")
        .expect("first script describe should succeed");
    registry
        .describe_model_provider("example-model-copy")
        .expect("duplicate script describe should use the same cached artifact");
    assert_eq!(read_count(&count), 1);
}

#[test]
fn duplicate_crate_refs_deduplicate_to_one_disabled_artifact_key_and_runtime_disabled_error() {
    let registry = ProviderRegistry::from_model_configs(
        &[
            model(
                "example-model",
                Some(crate_ref("example-provider", Some("0.1.0"))),
            ),
            model(
                "example-model-copy",
                Some(crate_ref("example-provider", Some("0.1.0"))),
            ),
        ],
        ProviderRegistryOptions::default(),
    )
    .expect("duplicate runtime-disabled crate refs should not fail construction");

    assert_eq!(registry.configured_artifact_keys().len(), 1);
    assert_eq!(
        registry.artifact_key_for_model("example-model"),
        registry.artifact_key_for_model("example-model-copy")
    );
    let error = registry
        .describe_model_provider("example-model-copy")
        .expect_err("duplicate crate refs stay runtime-disabled at lookup");
    assert_runtime_disabled(error);
}

#[test]
fn distinct_artifacts_key_distinctly() {
    let temp = tempfile::tempdir().unwrap();
    let first_count = temp.path().join("first-count");
    let second_count = temp.path().join("second-count");
    let first = write_fake_provider_script(
        temp.path(),
        "fake-provider-a",
        &first_count,
        Ok("provider-a"),
    );
    let second = write_fake_provider_script(
        temp.path(),
        "fake-provider-b",
        &second_count,
        Ok("provider-b"),
    );

    let registry = ProviderRegistry::from_model_configs(
        &[
            model(
                "example-model-a",
                Some(path_ref(first.display().to_string())),
            ),
            model(
                "example-model-b",
                Some(path_ref(second.display().to_string())),
            ),
        ],
        ProviderRegistryOptions::default(),
    )
    .expect("distinct provider refs should not fail construction");

    assert_eq!(registry.configured_artifact_keys().len(), 2);
    assert_ne!(
        registry.artifact_key_for_model("example-model-a"),
        registry.artifact_key_for_model("example-model-b")
    );
}

#[test]
fn describe_success_is_cached_per_registry_instance_only() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("describe-count");
    let fake =
        write_fake_provider_script(temp.path(), "fake-provider", &count, Ok("fake-provider"));
    let models = vec![model(
        "example-model",
        Some(path_ref(fake.display().to_string())),
    )];

    let registry =
        ProviderRegistry::from_model_configs(&models, ProviderRegistryOptions::default())
            .expect("registry construction should succeed before describe");
    let first = registry
        .describe_model_provider("example-model")
        .expect("first describe should succeed");
    let second = registry
        .describe_model_provider("example-model")
        .expect("same registry should use cached successful describe");
    assert_eq!(first, second);
    assert_eq!(read_count(&count), 1);

    let fresh = ProviderRegistry::from_model_configs(&models, ProviderRegistryOptions::default())
        .expect("fresh registry construction should succeed");
    fresh
        .describe_model_provider("example-model")
        .expect("fresh registry should perform its own describe");
    assert_eq!(
        read_count(&count),
        2,
        "successful describe cache must be scoped to one registry instance"
    );
}

#[test]
fn describe_request_envelope_matches_provider_contract() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("describe-count");
    let request_record = temp.path().join("describe-request.json");
    let config_root = temp.path().join("config");
    let data_root = temp.path().join("data");
    let cache_root = temp.path().join("cache");
    fs::create_dir_all(&config_root).unwrap();
    fs::create_dir_all(&data_root).unwrap();
    fs::create_dir_all(&cache_root).unwrap();
    let fake = write_fake_provider_request_recording_script(
        temp.path(),
        "fake-provider",
        &count,
        &request_record,
        "fake-provider",
    );

    let registry = ProviderRegistry::from_model_configs(
        &[model(
            "example-model",
            Some(path_ref(fake.display().to_string())),
        )],
        ProviderRegistryOptions::default()
            .with_config_root(config_root.clone())
            .with_data_root(data_root.clone())
            .with_cache_root(cache_root.clone()),
    )
    .expect("registry construction should not invoke describe");

    assert_eq!(read_count(&count), 0);
    registry
        .describe_model_provider("example-model")
        .expect("request-recording provider should describe successfully");

    let request = recorded_describe_request(&request_record);
    let (config_root_display, data_root_display) =
        expected_host_root_strings(&config_root, &data_root);
    assert_eq!(request["contract"].as_str(), Some(CONTRACT_VERSION));
    assert!(
        request["request_id"]
            .as_str()
            .is_some_and(|request_id| !request_id.is_empty()),
        "describe request must include a non-empty request_id"
    );
    assert_eq!(
        request["provider_instance_id"], "provider-registry",
        "describe uses a neutral provider instance id, not a model or account name"
    );
    assert_eq!(
        request["params"],
        serde_json::json!({}),
        "describe params must be EmptyParams"
    );
    assert_eq!(
        request["host"]["app"].as_str(),
        Some("oulipoly-agent-runner")
    );
    assert_eq!(
        request["host"]["config_root"].as_str(),
        Some(config_root_display.as_str())
    );
    assert_eq!(
        request["host"]["data_root"].as_str(),
        Some(data_root_display.as_str())
    );
    assert!(
        request["host"].as_object().is_some(),
        "describe request must include a host context block"
    );
}

fn recorded_describe_request(request_record: &Path) -> Value {
    parse_recorded_describe_request(&recorded_describe_request_bytes(request_record))
}

fn recorded_describe_request_bytes(request_record: &Path) -> Vec<u8> {
    fs::read(request_record).expect("fake provider should record describe request")
}

fn parse_recorded_describe_request(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).expect("recorded request should be valid JSON")
}

fn expected_host_root_strings(config_root: &Path, data_root: &Path) -> (String, String) {
    (
        config_root.display().to_string(),
        data_root.display().to_string(),
    )
}

#[test]
fn describe_failure_returns_err_without_failing_prior_construction_or_poisoning_retry() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("describe-count");
    let fake = write_fake_provider_script(
        temp.path(),
        "fake-provider",
        &count,
        Err("example_describe_failed"),
    );

    let registry = ProviderRegistry::from_model_configs(
        &[model(
            "example-model",
            Some(path_ref(fake.display().to_string())),
        )],
        ProviderRegistryOptions::default(),
    )
    .expect("describe failures must not fail registry construction");

    let first = registry
        .describe_model_provider("example-model")
        .expect_err("provider describe failure should be returned to the lookup caller");
    assert_provider_describe_failed(first, "example_describe_failed");

    let second = registry
        .describe_model_provider("example-model")
        .expect_err("failed describes should be retryable rather than cached");
    assert_provider_describe_failed(second, "example_describe_failed");
    assert_eq!(read_count(&count), 2);
}

#[test]
fn runtime_disabled_crate_lookup_returns_runtime_disabled_error() {
    let registry = ProviderRegistry::from_model_configs(
        &[model(
            "example-model",
            Some(crate_ref("example-provider", Some("0.1.0"))),
        )],
        ProviderRegistryOptions::default(),
    )
    .expect("runtime-disabled crate refs should not fail construction");

    let error = registry
        .describe_model_provider("example-model")
        .expect_err("crate provider refs are parse-only and runtime-disabled in AGE-214 S4");
    assert_runtime_disabled(error);
}

#[test]
fn missing_artifact_lookup_returns_transport_error() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing-fake-provider");

    let registry = ProviderRegistry::from_model_configs(
        &[model(
            "example-model",
            Some(path_ref(missing.display().to_string())),
        )],
        ProviderRegistryOptions::default(),
    )
    .expect("missing artifacts must not fail registry construction");

    let error = registry
        .describe_model_provider("example-model")
        .expect_err("missing configured artifacts should fail at lookup");
    assert_transport_kind(error, "missing_artifact");
}

#[test]
fn malformed_describe_output_returns_protocol_error() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("describe-count");
    let fake = write_fake_provider_raw_output_script(
        temp.path(),
        "fake-provider",
        &count,
        "this is not json",
    );

    let registry = ProviderRegistry::from_model_configs(
        &[model(
            "example-model",
            Some(path_ref(fake.display().to_string())),
        )],
        ProviderRegistryOptions::default(),
    )
    .expect("protocol failures must not fail registry construction");

    let error = registry
        .describe_model_provider("example-model")
        .expect_err("malformed provider output should be a protocol/schema registry error");
    assert_protocol_kind(error, "invalid_json");
    assert_eq!(
        read_count(&count),
        1,
        "protocol failures occur during lookup, not construction"
    );
}

#[test]
fn schema_invalid_describe_output_returns_protocol_error() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("describe-count");
    let fake = write_fake_provider_schema_invalid_script(temp.path(), "fake-provider", &count);

    let registry = ProviderRegistry::from_model_configs(
        &[model(
            "example-model",
            Some(path_ref(fake.display().to_string())),
        )],
        ProviderRegistryOptions::default(),
    )
    .expect("schema failures must not fail registry construction");

    let error = registry
        .describe_model_provider("example-model")
        .expect_err("schema-invalid provider output should be a protocol/schema registry error");
    assert_protocol_kind(error, "schema_invalid_response");
    assert_eq!(read_count(&count), 1);
}

#[test]
fn lookup_does_not_discover_an_unconfigured_provider() {
    let temp = tempfile::tempdir().unwrap();
    let count = temp.path().join("describe-count");
    let _unconfigured =
        write_fake_provider_script(temp.path(), "fake-provider", &count, Ok("fake-provider"));

    let registry = ProviderRegistry::from_model_configs(
        &[model("example-model", Some(binary_ref("provider-a")))],
        ProviderRegistryOptions::default().with_path_entries([temp.path().to_path_buf()]),
    )
    .expect("registry construction should not scan for ambient providers");

    assert!(
        registry.artifact_key_for_model("fake-provider").is_none(),
        "lookup by an unconfigured provider must not discover ambient executables"
    );
    assert!(
        registry.describe_model_provider("fake-provider").is_err(),
        "describe lookup must require a configured model/provider ref"
    );
    assert_eq!(
        read_count(&count),
        0,
        "unconfigured fake-provider executable must not be spawned"
    );
}

#[test]
fn registry_construction_and_cache_operations_do_not_mutate_state_config_or_cache_files() {
    let temp = tempfile::tempdir().unwrap();
    let config_root = temp.path().join("config");
    let data_root = temp.path().join("data");
    let cache_root = temp.path().join("cache");
    fs::create_dir_all(config_root.join("models")).unwrap();
    fs::create_dir_all(&data_root).unwrap();
    fs::create_dir_all(&cache_root).unwrap();
    fs::write(config_root.join("providers.toml"), "").unwrap();
    fs::write(config_root.join("sessions.toml"), "").unwrap();
    fs::write(config_root.join("models").join("example-model.toml"), "").unwrap();
    fs::write(data_root.join("state.db"), "").unwrap();

    let count = temp.path().join("describe-count");
    let fake =
        write_fake_provider_script(temp.path(), "fake-provider", &count, Ok("fake-provider"));
    let before = protected_tree_snapshot(&[&config_root, &data_root, &cache_root]);

    let registry = ProviderRegistry::from_model_configs(
        &[model(
            "example-model",
            Some(path_ref(fake.display().to_string())),
        )],
        ProviderRegistryOptions::default()
            .with_config_root(config_root.clone())
            .with_data_root(data_root.clone())
            .with_cache_root(cache_root.clone()),
    )
    .expect("registry construction should be file-inert");
    registry
        .describe_model_provider("example-model")
        .expect("describe should succeed");
    registry
        .describe_model_provider("example-model")
        .expect("cached describe should succeed");

    let after = protected_tree_snapshot(&[&config_root, &data_root, &cache_root]);
    assert_eq!(
        before, after,
        "registry/cache operations must not create or mutate state/config/cache files"
    );
}

#[test]
fn production_service_sources_do_not_route_current_paths_through_provider_registry_seams() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for relative in provider_registry_forbidden_sources() {
        assert_source_forbids_provider_registry(relative, &read_runtime_source(&root, relative));
    }
    assert_diagnostics_provider_registry_refs_are_terminal_classify_only(&read_runtime_source(
        &root,
        "diagnostics/mod.rs",
    ));
    assert_session_lifecycle_provider_registry_refs_are_s7a_only(
        &read_runtime_source(&root, "services/adapters.rs"),
        &read_runtime_source(&root, "services/session_lifecycle.rs"),
    );
}

fn provider_registry_forbidden_sources() -> &'static [&'static str] {
    &[
        "quota/in_flight.rs",
        "quota/mod.rs",
        "quota/marker_verification/mod.rs",
        "quota/marker_verification/config.rs",
        "quota/marker_verification/health.rs",
        "quota/marker_verification/lock.rs",
        "quota/outcome.rs",
        "quota/parse.rs",
        "quota/process.rs",
        "session_export/mod.rs",
        "session_replace/mod.rs",
        "services/migration.rs",
    ]
}

fn read_runtime_source(root: &Path, relative: &str) -> String {
    let path = root.join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn assert_source_forbids_provider_registry(relative: &str, source: &str) {
    assert!(
        !has_provider_registry_reference(source),
        "{relative} must stay on the current production path in AGE-214 S4"
    );
}

fn has_provider_registry_reference(source: &str) -> bool {
    source.contains("provider_registry")
        || source.contains("ProviderRegistry")
        || source.contains("describe_model_provider")
}

fn assert_diagnostics_provider_registry_refs_are_terminal_classify_only(source: &str) {
    assert_diagnostics_terminal_classify_registry_hook_present(source);
    assert_diagnostics_legacy_error_branch_has_no_registry(source);
    assert_diagnostics_classify_exhaustion_branch_has_no_registry(source);
}

fn assert_diagnostics_terminal_classify_registry_hook_present(source: &str) {
    for expected in [
        "DiagnosticsServiceRequest::ClassifyTerminal",
        "classify_terminal_with_registry",
        "diagnose_terminal_classify_hook",
        "external_provider::classify_terminal",
        "ProviderRegistryHandle::current",
    ] {
        assert!(
            source.contains(expected),
            "diagnostics/mod.rs must keep the sanctioned external terminal.classify registry hook: {expected}"
        );
    }
}

fn assert_diagnostics_legacy_error_branch_has_no_registry(source: &str) {
    assert!(
        !has_provider_registry_reference(diagnostics_request_branch_until(
            source,
            "DiagnosticsServiceRequest::DiagnoseError",
            ".map_err(|message| ServiceError::Dependency { message }),",
        )),
        "built-in diagnostics DiagnoseError branch must not route through provider registry"
    );
}

fn assert_diagnostics_classify_exhaustion_branch_has_no_registry(source: &str) {
    assert!(
        !has_provider_registry_reference(diagnostics_request_branch_until(
            source,
            "DiagnosticsServiceRequest::ClassifyExhaustion",
            "DiagnosticsServiceRequest::ClassifyTerminal",
        )),
        "built-in ClassifyExhaustion branch must not route through provider registry"
    );
}

fn assert_session_lifecycle_provider_registry_refs_are_s7a_only(
    adapters_source: &str,
    lifecycle_source: &str,
) {
    assert_adapters_provider_registry_refs_are_s7a_only(adapters_source);
    assert_lifecycle_provider_registry_refs_are_s7a_only(lifecycle_source);
    assert_adapters_provider_registry_wiring_terms(adapters_source);
    assert_lifecycle_provider_registry_dispatch_terms(lifecycle_source);
    assert_deferred_provider_registry_flows_absent(adapters_source, lifecycle_source);
}

fn assert_adapters_provider_registry_refs_are_s7a_only(adapters_source: &str) {
    assert_provider_registry_refs_match_allowed_lines(
        "services/adapters.rs",
        adapters_source,
        &[
            "use crate::provider_registry::ProviderRegistryHandle;",
            "provider_registry: Option<ProviderRegistryHandle>,",
            "pub fn with_registry_handle(provider_registry: ProviderRegistryHandle) -> Self {",
            "provider_registry: Some(provider_registry),",
            "self.provider_registry.as_ref(),",
        ],
    );
}

fn assert_lifecycle_provider_registry_refs_are_s7a_only(lifecycle_source: &str) {
    assert_provider_registry_refs_match_allowed_lines(
        "services/session_lifecycle.rs",
        lifecycle_source,
        &[
            "use crate::provider_registry::ProviderRegistryHandle;",
            "provider_registry: Option<&ProviderRegistryHandle>,",
            "provider_registry,",
            "provider_registry: Option<&'a ProviderRegistryHandle>,",
            ".provider_registry",
            ".ok_or_else(external_provider_registry_unavailable)?",
            "registry: &'a crate::provider_registry::ProviderRegistry,",
            "fn external_provider_registry_unavailable() -> ServiceError {",
            "message: \"session_provider_registry_unavailable\".to_string(),",
        ],
    );
}

fn assert_adapters_provider_registry_wiring_terms(adapters_source: &str) {
    for expected in [
        "ProductionSessionLifecycleService",
        "with_registry_handle",
        "ProviderRegistryHandle",
    ] {
        assert!(
            adapters_source.contains(expected),
            "services/adapters.rs may reference provider registry only to wire AGE-243 S7a session lifecycle dispatch: {expected}"
        );
    }
}

fn assert_lifecycle_provider_registry_dispatch_terms(lifecycle_source: &str) {
    for expected in [
        "SessionServiceExternalProviderIdentity",
        "session_provider",
        "session.read_turns",
        "session.capture",
    ] {
        assert!(
            lifecycle_source.contains(expected),
            "services/session_lifecycle.rs may reference provider registry only for AGE-243 S7a read/capture dispatch: {expected}"
        );
    }
}

fn assert_deferred_provider_registry_flows_absent(adapters_source: &str, lifecycle_source: &str) {
    for forbidden in ["session_export", "session_replace", "migration", "rotation"] {
        assert_deferred_provider_registry_flow_absent("adapters", adapters_source, forbidden);
        assert_deferred_provider_registry_flow_absent("lifecycle", lifecycle_source, forbidden);
    }
}

fn assert_deferred_provider_registry_flow_absent(label: &str, source: &str, forbidden: &str) {
    assert!(
        !source.contains(&deferred_provider_registry_flow_pattern(forbidden)),
        "S7a must not add provider registry dispatch for deferred {forbidden} flows in {label}"
    );
}

fn deferred_provider_registry_flow_pattern(forbidden: &str) -> String {
    format!("{forbidden}::external_provider")
}

fn assert_provider_registry_refs_match_allowed_lines(
    relative: &str,
    source: &str,
    allowed_lines: &[&str],
) {
    let refs = provider_registry_reference_lines(source);
    let unexpected_refs = unexpected_provider_registry_reference_lines(&refs, allowed_lines);
    assert_provider_registry_refs_allowed(relative, &unexpected_refs);
}

fn unexpected_provider_registry_reference_lines(
    refs: &[String],
    allowed_lines: &[&str],
) -> Vec<String> {
    refs.iter()
        .filter(|line| !line_matches_allowed_provider_registry_ref(line, allowed_lines))
        .cloned()
        .collect()
}

fn line_matches_allowed_provider_registry_ref(line: &str, allowed_lines: &[&str]) -> bool {
    allowed_lines.iter().any(|allowed| line.contains(allowed))
}

fn assert_provider_registry_refs_allowed(relative: &str, unexpected_refs: &[String]) {
    assert!(
        unexpected_refs.is_empty(),
        "{relative} provider-registry references must stay inside sanctioned S7a session lifecycle construction/read/capture paths: {unexpected_refs:?}"
    );
}

fn provider_registry_reference_lines(source: &str) -> Vec<String> {
    trim_source_lines(filter_provider_registry_reference_lines(source_lines(
        source,
    )))
}

fn source_lines(source: &str) -> Vec<&str> {
    source.lines().collect()
}

fn filter_provider_registry_reference_lines(lines: Vec<&str>) -> Vec<&str> {
    lines
        .into_iter()
        .filter(|line| has_provider_registry_reference(line))
        .collect()
}

fn trim_source_lines(lines: Vec<&str>) -> Vec<String> {
    lines.into_iter().map(trim_source_line).collect()
}

fn trim_source_line(line: &str) -> String {
    line.trim().to_string()
}

fn diagnostics_request_branch_until<'a>(
    source: &'a str,
    marker: &str,
    terminator: &str,
) -> &'a str {
    source
        .split(marker)
        .nth(1)
        .and_then(|after| after.split(terminator).next())
        .unwrap_or("")
}

#[test]
fn describe_result_fixture_matches_provider_contract_shape() {
    let result = describe_result("fake-provider");
    assert_eq!(result.provider_id, "fake-provider");
    assert_eq!(result.preferred_contract, CONTRACT_VERSION);
}
