//! AGE-164 cluster 6 — return-channel + child-marker IPC contracts.
//!
//! The contract pins these implicit external contracts as bit-for-bit
//! preservation invariants:
//!
//! - Environment variable names: `OULIPOLY_PARENT_INVOCATION`,
//!   `OULIPOLY_RETURN_CHANNEL`.
//! - Stderr marker prefix: `OULIPOLY_INVOCATION=<json>`.
//! - Return-channel JSONL: one object per line; same field names
//!   (`kind`, `payload`, etc.); blank lines tolerated; malformed lines
//!   warned and dropped.
//! - Return-channel cleanup: file is deleted after read; dir cleanup
//!   tolerates `DirectoryNotEmpty` and `NotFound` without warning.
//!
//! These tests verify the contract bit-for-bit via fixture child scripts
//! and observed-environment sidecars. They MUST remain PASS pre- and
//! post-refactor.
//!
//! ## Declared roles
//!
//! Roles: formatter, mapper, parser, validator.
//!
//! - formatter: environment-observer scripts and marker/receipt fixtures.
//! - mapper: returned-artifact and parent-invocation fixture construction.
//! - parser: sidecar path and JSONL observation parsing.
//! - validator: IPC env, JSONL, marker, and cleanup assertions.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/tests/age_164_c6_ipc.rs
//!     role: adapter
//!     Translates:
//!       - return-channel-jsonl-fixture-contract
//!       - composite-invocation-id-fixture-contract
//!       - executor-cli-ipc-public-api-contract
//!       - unix-fixture-sidecar-contract
//! ```

#![cfg(unix)]

use oulipoly_agent_messenger::{ReturnedArtifactRef, ReturnedArtifactSource, StoreAddress};
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_runtime::executor::cli::execute;
use oulipoly_state::CompositeInvocationId;
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use uuid::Uuid;

fn script_with_observed_env(
    env_name: &str,
    observed: &std::path::Path,
) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("observer.sh");
    std::fs::write(&path, observed_env_script_body(env_name, observed)).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    (dir, path)
}

fn observed_env_script_body(env_name: &str, observed: &Path) -> String {
    format!(
        "#!/usr/bin/env bash\n\
         set -euo pipefail\n\
         printf '%s' \"${{{name}-}}\" > '{observed}'\n\
         printf 'ok'\n",
        name = env_name,
        observed = observed.display()
    )
}

fn receipt_reference(invocation_uuid: Uuid, name: &str, version: u64) -> ReturnedArtifactRef {
    ReturnedArtifactRef {
        version_id: receipt_version_id(invocation_uuid, name, version),
        name: name.to_string(),
        store_address: StoreAddress {
            workflow_run_id: receipt_workflow_run_id(invocation_uuid),
            artifact_name: name.to_string(),
            version,
        },
        sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        content_len: 3,
        format_hint: None,
        verdict_line: None,
        source: ReturnedArtifactSource::InlineBytes,
        producer_invocation_uuid: invocation_uuid,
        returned_at: chrono::Utc::now(),
    }
}

fn receipt_version_id(invocation_uuid: Uuid, name: &str, version: u64) -> String {
    format!("store://return/{invocation_uuid}/{name}/{version}")
}

fn receipt_workflow_run_id(invocation_uuid: Uuid) -> String {
    format!("return:{invocation_uuid}")
}

fn receipt_json(reference: &ReturnedArtifactRef) -> String {
    serde_json::to_string(reference).expect("receipt json")
}

fn parent_env(invocation_uuid: Uuid) -> String {
    format!(r#"{{"source":"age-164-c6","id":"{invocation_uuid}"}}"#)
}

// ---------------------------------------------------------------------------
// Env var name: OULIPOLY_PARENT_INVOCATION — exact name, exact payload.
// ---------------------------------------------------------------------------

#[test]
fn parent_invocation_env_var_name_is_exact() {
    let invocation = Uuid::new_v4();
    let parent = parent_env(invocation);
    let dir = tempfile::tempdir().unwrap();
    let observed = dir.path().join("parent.txt");
    let (_script_dir, script_path) =
        script_with_observed_env("OULIPOLY_PARENT_INVOCATION", &observed);
    let provider = ProviderConfig::new(script_path.to_string_lossy().into_owned(), Vec::new());
    let model = ModelConfig {
        name: "ipc-parent-env-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };

    let result =
        execute(&model, 0, "prompt", None, &HashMap::new(), Some(&parent)).expect("execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(std::fs::read_to_string(&observed).unwrap(), parent);
}

// ---------------------------------------------------------------------------
// Env var name: OULIPOLY_RETURN_CHANNEL — bound by parent env, unbound when
// no parent env (and pre-existing parent-RC env vars are removed from the
// child env).
// ---------------------------------------------------------------------------

#[test]
fn return_channel_env_var_name_is_bound_when_parent_env_set() {
    let invocation = Uuid::new_v4();
    let parent = parent_env(invocation);
    let dir = tempfile::tempdir().unwrap();
    let observed = dir.path().join("rc.txt");
    let (_script_dir, script_path) = script_with_observed_env("OULIPOLY_RETURN_CHANNEL", &observed);
    let provider = ProviderConfig::new(script_path.to_string_lossy().into_owned(), Vec::new());
    let model = ModelConfig {
        name: "ipc-rc-bind-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };

    let result =
        execute(&model, 0, "prompt", None, &HashMap::new(), Some(&parent)).expect("execute");

    assert_eq!(result.exit_code, 0);
    let observed_value = std::fs::read_to_string(&observed).unwrap();
    assert!(
        !observed_value.is_empty(),
        "OULIPOLY_RETURN_CHANNEL must be set in the child env when parent_invocation_env is provided"
    );
    // The channel path lives under TMPDIR / "oulipoly-return-channels" / invocation_id /...
    assert!(
        observed_value.contains("oulipoly-return-channels"),
        "channel path must be in the runtime's return-channel directory; got {observed_value}"
    );
    assert!(
        observed_value.contains(&invocation.to_string()),
        "channel path must include the invocation id; got {observed_value}"
    );
}

// ---------------------------------------------------------------------------
// Return-channel JSONL: one object per line; valid receipt accepted.
// ---------------------------------------------------------------------------

#[test]
fn return_channel_jsonl_one_object_per_line_is_accepted() {
    let invocation = Uuid::new_v4();
    let parent = parent_env(invocation);
    let receipt_a = receipt_json(&receipt_reference(invocation, "a.bin", 1));
    let receipt_b = receipt_json(&receipt_reference(invocation, "b.bin", 2));

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("two-receipts.sh");
    std::fs::write(
        &path,
        format!(
            "#!/usr/bin/env bash\n\
             set -euo pipefail\n\
             printf '%s\\n%s\\n' '{a}' '{b}' >> \"$OULIPOLY_RETURN_CHANNEL\"\n",
            a = receipt_a,
            b = receipt_b
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();

    let provider = ProviderConfig::new(path.to_string_lossy().into_owned(), Vec::new());
    let model = ModelConfig {
        name: "ipc-jsonl-two-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };
    let result =
        execute(&model, 0, "prompt", None, &HashMap::new(), Some(&parent)).expect("execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.returned_artifacts.len(), 2);
    assert_eq!(result.returned_artifacts[0].name, "a.bin");
    assert_eq!(result.returned_artifacts[1].name, "b.bin");
}

// ---------------------------------------------------------------------------
// Return-channel JSONL: malformed lines are dropped (and warned), but the
// other receipts in the same channel are still accepted.
// ---------------------------------------------------------------------------

#[test]
fn return_channel_jsonl_malformed_line_is_dropped_but_others_kept() {
    let invocation = Uuid::new_v4();
    let parent = parent_env(invocation);
    let receipt = receipt_json(&receipt_reference(invocation, "valid.bin", 1));

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mixed.sh");
    std::fs::write(
        &path,
        format!(
            "#!/usr/bin/env bash\n\
             set -euo pipefail\n\
             printf 'this is not json\\n' >> \"$OULIPOLY_RETURN_CHANNEL\"\n\
             printf '%s\\n' '{}' >> \"$OULIPOLY_RETURN_CHANNEL\"\n\
             printf '{{not-json}}\\n' >> \"$OULIPOLY_RETURN_CHANNEL\"\n",
            receipt
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();

    let provider = ProviderConfig::new(path.to_string_lossy().into_owned(), Vec::new());
    let model = ModelConfig {
        name: "ipc-jsonl-malformed-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };
    let result =
        execute(&model, 0, "prompt", None, &HashMap::new(), Some(&parent)).expect("execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.returned_artifacts.len(), 1);
    assert_eq!(result.returned_artifacts[0].name, "valid.bin");
}

#[test]
fn return_channel_blank_line_is_skipped_without_artifact() {
    let invocation = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
    let parent = parent_env(invocation);
    let receipt = receipt_json(&receipt_reference(invocation, "blob", 1));

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("blank-lines.sh");
    std::fs::write(
        &path,
        format!(
            "#!/usr/bin/env bash\n\
             set -euo pipefail\n\
             printf '\\n' >> \"$OULIPOLY_RETURN_CHANNEL\"\n\
             printf '%s\\n' '{receipt}' >> \"$OULIPOLY_RETURN_CHANNEL\"\n\
             printf '\\n' >> \"$OULIPOLY_RETURN_CHANNEL\"\n",
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();

    let provider = ProviderConfig::new(path.to_string_lossy().into_owned(), Vec::new());
    let model = ModelConfig {
        name: "blank-line-test".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };
    let result =
        execute(&model, 0, "prompt", None, &HashMap::new(), Some(&parent)).expect("execute");

    assert_eq!(result.returned_artifacts.len(), 1);
    assert_eq!(result.returned_artifacts[0].name, "blob");
}

// ---------------------------------------------------------------------------
// Return-channel sidecar is deleted after read.
// ---------------------------------------------------------------------------

#[test]
fn return_channel_sidecar_file_is_deleted_after_read() {
    let invocation = Uuid::new_v4();
    let parent = parent_env(invocation);
    let dir = tempfile::tempdir().unwrap();
    let observed_channel = dir.path().join("observed.txt");
    let path = dir.path().join("observer.sh");
    std::fs::write(
        &path,
        format!(
            "#!/usr/bin/env bash\n\
             set -euo pipefail\n\
             printf '%s' \"$OULIPOLY_RETURN_CHANNEL\" > '{}'\n",
            observed_channel.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();

    let provider = ProviderConfig::new(path.to_string_lossy().into_owned(), Vec::new());
    let model = ModelConfig {
        name: "ipc-sidecar-cleanup-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };
    let result =
        execute(&model, 0, "prompt", None, &HashMap::new(), Some(&parent)).expect("execute");

    assert_eq!(result.exit_code, 0);
    let channel_path =
        std::path::PathBuf::from(std::fs::read_to_string(&observed_channel).unwrap());
    assert!(
        !channel_path.exists(),
        "return channel sidecar must be deleted after read; still exists at {channel_path:?}"
    );
}

#[test]
fn return_channel_cleanup_tolerates_non_empty_dir() {
    let invocation = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
    let parent = parent_env(invocation);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dirty.sh");
    // Touch a sibling file in the channel dir before exit. The runtime
    // owns `${TMPDIR}/oulipoly-return-channels/${invocation_id}/`; on
    // cleanup the dir-not-empty case should be tolerated by the predicate.
    std::fs::write(
        &path,
        "#!/usr/bin/env bash\nset -euo pipefail\n\
         channel_dir=\"$(dirname \"$OULIPOLY_RETURN_CHANNEL\")\"\n\
         : > \"$channel_dir/sidecar.txt\"\nexit 0\n",
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();

    let provider = ProviderConfig::new(path.to_string_lossy().into_owned(), Vec::new());
    let model = ModelConfig {
        name: "dirty-channel-test".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };
    let result =
        execute(&model, 0, "prompt", None, &HashMap::new(), Some(&parent)).expect("execute");

    assert_eq!(result.exit_code, 0);
    assert!(result.returned_artifacts.is_empty());
}

// ---------------------------------------------------------------------------
// Parent invocation env: invalid JSON returns canonical error.
// ---------------------------------------------------------------------------

#[test]
fn invalid_parent_invocation_env_returns_canonical_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("never.sh");
    std::fs::write(&path, "#!/usr/bin/env bash\nexit 0\n").unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();

    let provider = ProviderConfig::new(path.to_string_lossy().into_owned(), Vec::new());
    let model = ModelConfig {
        name: "ipc-bad-parent-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };
    let err = execute(
        &model,
        0,
        "prompt",
        None,
        &HashMap::new(),
        Some("not valid json"),
    )
    .unwrap_err();
    assert!(
        err.contains("Failed to parse parent invocation for return channel"),
        "{err}"
    );
}

// ---------------------------------------------------------------------------
// Stderr marker prefix: OULIPOLY_INVOCATION=<json>. The runtime parser
// recognizes lines with this exact prefix and parses the JSON payload via
// CompositeInvocationId::parse_env_value.
// ---------------------------------------------------------------------------

#[test]
fn child_marker_stderr_prefix_is_exact_and_parser_recognizes_canonical_payload() {
    let marker = CompositeInvocationId {
        source: "age-164-c6-child".to_string(),
        id: "33333333-3333-3333-3333-333333333333".to_string(),
    };
    let marker_line = marker.stderr_line();
    // The line MUST begin with the literal prefix.
    assert!(
        marker_line.starts_with("OULIPOLY_INVOCATION="),
        "stderr marker line must use the canonical prefix; got {marker_line}"
    );

    // Run a fixture provider whose stderr emits the marker. The runtime
    // captures it and surfaces it in result.captured_child_invocations.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("marker.sh");
    std::fs::write(
        &path,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' '{}' >&2\n",
            marker_line
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();

    let provider = ProviderConfig::new(path.to_string_lossy().into_owned(), Vec::new());
    let model = ModelConfig {
        name: "ipc-marker-prefix-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };
    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.captured_child_invocations.len(), 1);
    assert_eq!(result.captured_child_invocations[0].composite_id, marker);
    assert_eq!(
        result.captured_child_invocations[0].raw_marker_line,
        marker_line
    );
}

#[test]
fn child_marker_parser_drops_malformed_lines_and_dedupes_by_composite_id() {
    let marker = CompositeInvocationId {
        source: "age-164-c6-child".to_string(),
        id: "44444444-4444-4444-4444-444444444444".to_string(),
    };
    let marker_line = marker.stderr_line();
    // 1) Plain noise (no prefix). 2) Prefix with non-UUID. 3) Valid marker.
    // 4) Same valid marker again — dedupe should keep one.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("marker-dedupe.sh");
    std::fs::write(
        &path,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\n\
             printf 'noise\\n' >&2\n\
             printf 'OULIPOLY_INVOCATION={{\"source\":\"age-164-c6-child\",\"id\":\"not-a-uuid\"}}\\n' >&2\n\
             printf '%s\\n' '{m}' >&2\n\
             printf '%s\\n' '{m}' >&2\n",
            m = marker_line
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();

    let provider = ProviderConfig::new(path.to_string_lossy().into_owned(), Vec::new());
    let model = ModelConfig {
        name: "ipc-marker-dedupe-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };
    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.captured_child_invocations.len(), 1);
    assert_eq!(result.captured_child_invocations[0].composite_id, marker);
    assert_eq!(
        result.captured_child_invocations[0].raw_marker_line,
        marker_line
    );
}

// ---------------------------------------------------------------------------
// Without parent env, OULIPOLY_RETURN_CHANNEL is removed from the child
// env (not inherited), and no channel directory is created or read.
// ---------------------------------------------------------------------------

#[test]
fn no_parent_env_removes_return_channel_from_child_env() {
    let dir = tempfile::tempdir().unwrap();
    let observed = dir.path().join("rc.txt");
    let (_script_dir, script_path) = script_with_observed_env("OULIPOLY_RETURN_CHANNEL", &observed);
    let provider = ProviderConfig::new(script_path.to_string_lossy().into_owned(), Vec::new());
    let model = ModelConfig {
        name: "ipc-no-parent-env-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };
    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(
        std::fs::read_to_string(&observed).unwrap(),
        "",
        "child must see OULIPOLY_RETURN_CHANNEL unset when no parent_invocation_env is provided"
    );
    assert!(result.returned_artifacts.is_empty());
}
