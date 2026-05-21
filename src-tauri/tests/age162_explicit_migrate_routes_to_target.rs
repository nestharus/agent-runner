#![cfg(unix)]

//! AGE-162 Symptom 5 — `agents --resume <session-id> --migrate <target>` must
//! dispatch the next CLI subprocess as `<target>`'s binary, not the session
//! owner's binary.
//!
//! Root attestation (`supplementary-evidence-resume-pinning.md` §Symptom 5):
//! tried `--migrate <some-other-provider>` to manually rotate off the loaded
//! session's owner, and the dispatch STILL went to the owner. This is
//! distinct from Symptoms 1+4 because the AGE-148 `DecisionFailed` guard at
//! `services/migration.rs:60-69` is gated on `request.manual_target.is_none()`
//! — it should NOT fire when `--migrate` is supplied — yet the `--migrate`
//! path still doesn't move the session.
//!
//! Hypothesis space named in the supplementary evidence (Phase 2 to localize):
//!   (a) `--migrate <provider>` parsing fails to populate
//!       `MigrationServiceRequest.manual_target`.
//!   (b) AGE-148's resume-time provider recording pins the chain to the
//!       recorded provider regardless of `--migrate`.
//!   (c) Migration completes (DB segment updated) but the dispatch process
//!       subsequently picks the original provider via some other code path.
//!   (d) `run_service_migration` returns `Migrated { segment }` but the
//!       caller never uses the new segment for the actual CLI invocation.
//!
//! Contract under test (root-attested): after the operator runs
//! `agents --resume <sid> --migrate <target>`, the dispatched OS subprocess
//! command-line MUST be the target provider's `command`, not the original.
//! This test materialises the binaries as marker-writing shell scripts so
//! the assertion can read which binary actually ran from disk.

use rusqlite::{Connection, params};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const CHAIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const MODEL_NAME: &str = "age162-migrate-repro";
const SESSION_OWNER: &str = "claude-owner";
const MIGRATE_TARGET: &str = "claude-target";

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();
        Self {
            dir,
            config_home,
            data_home,
            app_config_dir,
            models_dir,
        }
    }

    fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn conn(&self) -> Connection {
        let _ = oulipoly_state::StateDb::open(&self.db_path()).unwrap();
        Connection::open(self.db_path()).unwrap()
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn provider_projects_dir(&self, provider: &str) -> PathBuf {
        self.dir.path().join(format!("{provider}-projects"))
    }

    /// Writes a model TOML referencing two resume-capable providers in the
    /// order owner-first, target-second so `decide_migration`'s manual-target
    /// branch has both eligible candidates.
    fn write_resume_pool(&self, owner_body: &str, target_body: &str) {
        fs::write(
            self.models_dir.join(format!("{MODEL_NAME}.toml")),
            format!(
                r#"[[providers]]
name = "{SESSION_OWNER}"
args = ["exec-{SESSION_OWNER}"]

[[providers]]
name = "{MIGRATE_TARGET}"
args = ["exec-{MIGRATE_TARGET}"]
"#
            ),
        )
        .unwrap();

        // Minimal diagnostics-model wiring so the runtime's diagnostics
        // service doesn't trip during fixture setup.
        fs::write(
            self.models_dir.join("diagnostic.toml"),
            r#"[[providers]]
name = "diagnostic-provider"
"#,
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("config.toml"),
            r#"diagnostics_model = "diagnostic"
"#,
        )
        .unwrap();

        let owner_cmd = self.write_script(&format!("{SESSION_OWNER}-resume.sh"), owner_body);
        let target_cmd = self.write_script(&format!("{MIGRATE_TARGET}-resume.sh"), target_body);
        let owner_projects = self.provider_projects_dir(SESSION_OWNER);
        let target_projects = self.provider_projects_dir(MIGRATE_TARGET);
        let diagnostic_command =
            self.write_script("diagnostic-provider.sh", "cat >/dev/null\nexit 9");

        let providers_toml = format!(
            r#"[{SESSION_OWNER}]
command = {}
args = []
interactive_args = ["launch-{SESSION_OWNER}"]
prompt_mode = "arg"

[{SESSION_OWNER}.resume]
kind = "flag"
flag = "--resume"

[{SESSION_OWNER}.session_storage]
kind = "claude_code"
projects_dir = {}

[{MIGRATE_TARGET}]
command = {}
args = []
interactive_args = ["launch-{MIGRATE_TARGET}"]
prompt_mode = "arg"

[{MIGRATE_TARGET}.resume]
kind = "flag"
flag = "--resume"

[{MIGRATE_TARGET}.session_storage]
kind = "claude_code"
projects_dir = {}

[diagnostic-provider]
command = {}
args = []
prompt_mode = "stdin"
"#,
            toml_string(&owner_cmd.display().to_string()),
            toml_string(&owner_projects.display().to_string()),
            toml_string(&target_cmd.display().to_string()),
            toml_string(&target_projects.display().to_string()),
            toml_string(&diagnostic_command.display().to_string()),
        );
        fs::write(self.app_config_dir.join("providers.toml"), providers_toml).unwrap();
    }

    /// Place a Claude-Code-style transcript file for the session owner.
    /// The migration step's `find_claude_source_from_storage` walks the
    /// owner's `projects_dir`; without this file `migrate_chain_segment`
    /// would fail with `SourceMissingStorage`.
    fn stage_active_claude_jsonl(&self) {
        let source_dir = self
            .provider_projects_dir(SESSION_OWNER)
            .join("source-project");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(
            source_dir.join(format!("{SESSION_ID}.jsonl")),
            format!(
                r#"{{"sessionId":"{SESSION_ID}","turnId":"turn-1","timestamp":"2026-04-17T08:00:00Z","type":"assistant"}}"#
            ),
        )
        .unwrap();
    }

    fn seed_active_chain(&self) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', ?2)",
            params![CHAIN_ID, MODEL_NAME],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
            params![CHAIN_ID, SESSION_OWNER, SESSION_ID],
        )
        .unwrap();
    }

    fn run_resume_with_migrate(&self, target_provider: &str) -> Output {
        // AGE-163 WU-A.6: the per-dispatch rotation flag was renamed from
        // `--migrate` to `--rotate-provider` in lockstep with this driver
        // update; the assertion bodies below are preserved verbatim.
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("-m")
            .arg(MODEL_NAME)
            .arg("--resume")
            .arg(SESSION_ID)
            .arg("--rotate-provider")
            .arg(target_provider)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("manual rotate via --rotate-provider flag");
        cmd.current_dir(self.dir.path());
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.output().unwrap()
    }
}

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

fn marker_was_written(marker: &Path) -> bool {
    fs::read_to_string(marker)
        .map(|content| !content.trim().is_empty())
        .unwrap_or(false)
}

fn provider_body(marker: &Path, shell: &str) -> String {
    format!(
        "printf '%s\\n' ran >> {}\n{shell}",
        toml_string(&marker.display().to_string())
    )
}

/// (a) Integration test: `--migrate <target>` must dispatch the target's
/// binary on the next subprocess, not the session owner's binary.
///
/// Pre-state:
///   - Active chain segment owned by `claude-owner` (the loaded session).
///   - Source JSONL staged so migration succeeds (rules out the AGE-148
///     `SourceMissingStorage` short-circuit; that's covered by Test 1).
///
/// Operator command:
///   `agents -m <model> --resume <sid> --migrate claude-target "<prompt>"`
///
/// Expected: after the dispatch, only `claude-target`'s marker file
/// contains `ran`. `claude-owner`'s marker is empty/absent.
///
/// Failure mode under the live AGE-159 / root attestation: `claude-owner`'s
/// marker is written and `claude-target`'s is not — i.e. the `--migrate`
/// flag was silently ignored and the dispatch went to the session owner.
#[test]
fn age162_resume_with_explicit_migrate_dispatches_target_binary_not_session_owner() {
    let fixture = Fixture::new();
    let owner_marker = fixture.dir.path().join("age162-migrate-owner-marker.txt");
    let target_marker = fixture.dir.path().join("age162-migrate-target-marker.txt");
    let _ = fs::remove_file(&owner_marker);
    let _ = fs::remove_file(&target_marker);

    fixture.write_resume_pool(
        &provider_body(
            &owner_marker,
            "printf '%s\\n' 'session owner dispatched (BUG: --migrate ignored)' >&2\nexit 1",
        ),
        &provider_body(
            &target_marker,
            "printf '%s\\n' 'migrate target dispatched'\nexit 0",
        ),
    );
    fixture.stage_active_claude_jsonl();
    fixture.seed_active_chain();

    let output = fixture.run_resume_with_migrate(MIGRATE_TARGET);

    let owner_ran = marker_was_written(&owner_marker);
    let target_ran = marker_was_written(&target_marker);

    assert!(
        target_ran,
        "AGE-162 Symptom 5: --migrate {MIGRATE_TARGET} did not dispatch the \
         target provider's binary. Target marker {target_marker:?} is empty/\
         absent after running `agents --resume <sid> --migrate {MIGRATE_TARGET}`. \
         Output: status={:?} stdout={:?} stderr={:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        !owner_ran,
        "AGE-162 Symptom 5: session-owner provider {SESSION_OWNER} was \
         dispatched despite the operator supplying `--migrate {MIGRATE_TARGET}`. \
         Root attested this empirically: 'tried --migrate <some-other-provider> \
         to manually rotate off claude5 (the loaded session's owner), and the \
         dispatch STILL went to claude5.' Owner marker {owner_marker:?} \
         contains the dispatch trace. Output: status={:?} stdout={:?} \
         stderr={:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// (b) Confirmation that the chain segment also moved to the migrate target.
/// Pinned alongside (a) so the bug surface is unambiguous: even if the
/// dispatch happens to land on the target by accident, the DB state must
/// reflect the manual rotation requested by the operator. If only (a) is
/// satisfied and (b) is not, the runtime is dispatching to the right binary
/// but lying about it in state.
#[test]
fn age162_resume_with_explicit_migrate_records_target_as_active_chain_segment_provider() {
    let fixture = Fixture::new();
    let owner_marker = fixture.dir.path().join("age162-migrate-owner-marker-b.txt");
    let target_marker = fixture
        .dir
        .path()
        .join("age162-migrate-target-marker-b.txt");
    let _ = fs::remove_file(&owner_marker);
    let _ = fs::remove_file(&target_marker);

    fixture.write_resume_pool(
        &provider_body(&owner_marker, "printf '%s\\n' 'owner stdout'\nexit 0"),
        &provider_body(&target_marker, "printf '%s\\n' 'target stdout'\nexit 0"),
    );
    fixture.stage_active_claude_jsonl();
    fixture.seed_active_chain();

    let _output = fixture.run_resume_with_migrate(MIGRATE_TARGET);

    let active_provider: String = fixture
        .conn()
        .query_row(
            "SELECT provider_name
             FROM session_chain_segments
             WHERE chain_id = ?1 AND ended_at IS NULL",
            params![CHAIN_ID],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(
        active_provider, MIGRATE_TARGET,
        "AGE-162 Symptom 5 DB-state pin: after `agents --resume <sid> \
         --migrate {MIGRATE_TARGET}`, the active chain segment in \
         session_chain_segments must reflect the manual rotation. \
         current_active_provider={active_provider:?}"
    );
}
