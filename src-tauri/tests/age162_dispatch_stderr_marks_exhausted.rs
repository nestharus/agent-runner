#![cfg(unix)]

//! AGE-162 Symptom 2 — A dispatch whose stderr matches the transient
//! rate-limit signature (HTTP 429 / "rate limit" / "too many requests")
//! must NOT cause the runtime to flip `exhausted_at` on the dispatched
//! provider's quota record.
//!
//! Live incident (2026-05-19): `agents --usage` reported
//! `claude4 window-0 (error: HTTP 429: rate-limited)` while the operator's
//! T+30s direct probe of the same credentials returned `{"used_percent": 0.0}`
//! for both windows — i.e. `claude4` was healthy, and the earlier 429 was a
//! transient probe-endpoint rate limit. Yet `claude4` was removed from
//! rotation. The AGE-159 dispatch trace records the matching signature:
//!
//! ```text
//! [diagnostics] rate_limit: Heuristic classification based on stderr content
//! ```
//!
//! The Phase 1 cycle 1 child traced the strongest candidate path to
//! `src-tauri/src/balanced_cli.rs::balanced_result_error_category` →
//! `oulipoly_runtime::diagnostics::classify_exhaustion` and the typed
//! Recognizer chain → `terminal_outcome_adapter::apply_terminal_signal_outcome`
//! → `balanced_cli::mark_provider_exhausted` (which calls
//! `<StateDb as ProviderQuotaRepository>::mark_exhausted`).
//!
//! The bug locus is the over-broad keyword set in
//! `crates/oulipoly-runtime/src/executor/providers/claude.rs::contains_quota_token`,
//! which classifies "rate limit" / "rate_limit_error" / "too many requests"
//! as `TerminalSignalKind::QuotaExhaustedInband` — a strictly stronger
//! claim than "this dispatch hit a rate limit". `apply_terminal_signal_outcome`
//! then maps that signal to `QuotaExhaustedRetry` disposition, which calls
//! `mark_provider_exhausted` and sets `exhausted_at` permanently (until
//! `clear_exhausted` is invoked, which only happens via the reset-implied
//! cooldown path in the balancer).
//!
//! Contract under test (derived from incident-brief §Symptom 2 + the live
//! operator observation that `claude4` was at 0%/0% used at the same moment):
//!
//! - A dispatch stderr that contains ONLY rate-limit signatures (no
//!   "usage limit reached" / "monthly limit" / "billing limit" wording) must
//!   NOT result in `exhausted_at` being set on the dispatched provider.
//! - A dispatch stderr that contains a legitimate quota signature
//!   ("usage limit reached") may still flip `exhausted_at` — that's the
//!   intended path for true upstream quota exhaustion. This test pins the
//!   distinction.

use oulipoly_state::StateDb;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SESSION_OWNER: &str = "claude-rate-repro";
const SIBLING: &str = "claude-rate-sibling";
const CODEX_OWNER: &str = "codex-rate-repro";
const CODEX_SIBLING: &str = "codex-rate-sibling";
const OPENAI_COMPAT_OWNER: &str = "gemini-rate-repro";
const OPENAI_COMPAT_SIBLING: &str = "gemini-rate-sibling";

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

    fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path()).unwrap()
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

    fn write_model(&self, model_name: &str, providers: &[&str]) {
        let mut body = String::new();
        for provider in providers {
            body.push_str(&format!(
                r#"[[providers]]
name = "{provider}"
args = []

"#
            ));
        }
        fs::write(self.models_dir.join(format!("{model_name}.toml")), body).unwrap();
    }

    /// Each provider's script:
    ///   - appends `ran` to its marker file (proves it was invoked)
    ///   - emits its `stderr_payload` to stderr
    ///   - exits with code 1 (so the runtime treats the dispatch as failed
    ///     and consults the recognizer / diagnostics path)
    fn write_providers(&self, providers: &[(&str, &Path, &str)]) {
        let mut body = String::new();
        for (provider, marker, stderr_payload) in providers {
            let command = self.write_script(
                &format!("{provider}-command.sh"),
                &format!(
                    "printf '%s\\n' ran >> {}\nprintf '%s\\n' {} >&2\nexit 1",
                    toml_string(&marker.display().to_string()),
                    toml_string(stderr_payload)
                ),
            );
            body.push_str(&format!(
                r#"[{provider}]
command = {}
args = []
prompt_mode = "arg"

"#,
                toml_string(&command.display().to_string())
            ));
        }
        fs::write(self.app_config_dir.join("providers.toml"), body).unwrap();
    }

    fn run_one_shot(&self, model_name: &str) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg(model_name)
            .arg("prompt");
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env("HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.output().unwrap()
    }
}

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

fn provider_exhausted_at_is_set(db: &StateDb, provider: &str) -> bool {
    db.get_quota(provider)
        .unwrap()
        .and_then(|quota| quota.exhausted_at)
        .is_some()
}

/// (a) Pin the Symptom 2 contract against the AGE-159 signature.
///
/// stderr matches the empirical AGE-159 trace signature: a transient HTTP 429
/// / rate-limit message. With the live operator snapshot showing the same
/// provider at 0%/0% used immediately before, the rate-limit was on the
/// probe endpoint, not on this account's actual quota. The runtime must
/// NOT flip `exhausted_at` for the dispatched provider.
///
/// Failure mode under current code (root cause hypothesis): the Claude
/// `Recognizer` matches "rate limit" / "too many requests" / "rate_limit_error"
/// via `executor/providers/claude.rs::contains_quota_token`, which produces
/// `TerminalSignalKind::QuotaExhaustedInband`. The typed terminal-signal
/// outcome flow in `terminal_outcome_adapter::apply_terminal_signal_outcome`
/// then calls `balanced_cli::mark_provider_exhausted`.
#[test]
fn age162_transient_rate_limit_stderr_does_not_mark_exhausted_under_claude_recognizer() {
    let fixture = Fixture::new();
    let owner_marker = fixture.dir.path().join("age162-rate-owner.txt");
    let sibling_marker = fixture.dir.path().join("age162-rate-sibling.txt");

    fixture.write_model("age162-rate-repro", &[SESSION_OWNER, SIBLING]);
    fixture.write_providers(&[
        (
            SESSION_OWNER,
            &owner_marker,
            // AGE-159 trace + canonical HTTP 429 rate-limit signature. No
            // "usage limit reached" / "billing limit" / "monthly limit"
            // wording — pure transient rate-limit. The operator's T+30s
            // direct probe showed claude4 at 0%/0% used at the same
            // moment, so this MUST be transient and NOT mark exhausted.
            "HTTP 429: too many requests. rate_limit_error encountered.",
        ),
        (
            SIBLING,
            &sibling_marker,
            // Sibling: a benign error so the test ends without an "all
            // providers exhausted" cascade.
            "connection refused",
        ),
    ]);

    let _output = fixture.run_one_shot("age162-rate-repro");

    let db = fixture.open_db();
    let owner_exhausted = provider_exhausted_at_is_set(&db, SESSION_OWNER);
    assert!(
        !owner_exhausted,
        "AGE-162 Symptom 2: provider {SESSION_OWNER} had its `exhausted_at` \
         flipped after a dispatch whose stderr was a transient HTTP 429 / \
         rate-limit signature. The operator's parallel live probe showed \
         the same account at 0%/0% used; rate-limited probe responses are \
         transient and MUST NOT flip the provider's quota-exhausted state. \
         Bug locus: executor/providers/claude.rs::contains_quota_token \
         conflates rate-limit signatures with quota-exhausted, then \
         terminal_outcome_adapter::apply_terminal_signal_outcome routes \
         that into balanced_cli::mark_provider_exhausted."
    );
}

/// (b) Branch-coverage companion: a stderr with a legitimate quota signature
/// (Anthropic's canonical "Claude usage limit reached") DOES flip
/// `exhausted_at`. This pins the distinction asked for by the supplementary
/// evidence: the fix must preserve the legitimate quota-exhausted path
/// while gating the over-broad rate-limit classification.
///
/// This test should PASS green against current code (the canonical quota
/// signature is correctly classified). It exists to document that the
/// failing test above is not asking for the heuristic to be removed
/// wholesale — only for the rate-limit branch to be gated.
#[test]
fn age162_canonical_quota_signature_still_flips_exhausted_at_under_claude_recognizer() {
    let fixture = Fixture::new();
    let owner_marker = fixture.dir.path().join("age162-quota-owner.txt");
    let sibling_marker = fixture.dir.path().join("age162-quota-sibling.txt");

    fixture.write_model("age162-quota-repro", &[SESSION_OWNER, SIBLING]);
    fixture.write_providers(&[
        (
            SESSION_OWNER,
            &owner_marker,
            // Canonical Anthropic quota signature — this is the legitimate
            // "your monthly quota is exhausted; come back next billing cycle"
            // message that the heuristic exists to catch.
            "Claude usage limit reached for your account.",
        ),
        (
            SIBLING,
            &sibling_marker,
            "connection refused",
        ),
    ]);

    let _output = fixture.run_one_shot("age162-quota-repro");

    let db = fixture.open_db();
    let owner_exhausted = provider_exhausted_at_is_set(&db, SESSION_OWNER);
    assert!(
        owner_exhausted,
        "AGE-162 Symptom 2 branch coverage: provider {SESSION_OWNER} did NOT \
         have `exhausted_at` set after a dispatch whose stderr contained \
         the canonical 'Claude usage limit reached' quota signature. The \
         legitimate quota-exhausted path must remain functional — the \
         Symptom 2 fix is to narrow the rate-limit branch, not to remove \
         the quota-exhaustion classifier entirely."
    );
}

/// (c) Same Symptom 2 contract pinned against the Codex `Recognizer` — a
/// dispatch whose stderr is a pure HTTP-429 / `rate_limit_exceeded` signature
/// (no `usage cap` / `billing limit` / `quota exceeded` wording) must NOT
/// flip `exhausted_at`. The Codex recognizer is selected when the provider
/// name (or command basename) starts with `codex`.
#[test]
fn age162_transient_rate_limit_stderr_does_not_mark_exhausted_under_codex_recognizer() {
    let fixture = Fixture::new();
    let owner_marker = fixture.dir.path().join("age162-codex-rate-owner.txt");
    let sibling_marker = fixture.dir.path().join("age162-codex-rate-sibling.txt");

    fixture.write_model("age162-codex-rate-repro", &[CODEX_OWNER, CODEX_SIBLING]);
    fixture.write_providers(&[
        (
            CODEX_OWNER,
            &owner_marker,
            "OpenAI API returned HTTP 429: rate_limit_exceeded for this request.",
        ),
        (CODEX_SIBLING, &sibling_marker, "connection refused"),
    ]);

    let _output = fixture.run_one_shot("age162-codex-rate-repro");

    let db = fixture.open_db();
    let owner_exhausted = provider_exhausted_at_is_set(&db, CODEX_OWNER);
    assert!(
        !owner_exhausted,
        "AGE-162 Symptom 2 (Codex recognizer): provider {CODEX_OWNER} had \
         its `exhausted_at` flipped after a dispatch whose stderr was a \
         transient HTTP 429 / rate_limit_exceeded signature. Bug locus: \
         executor/providers/codex.rs predicate must split persistent-quota \
         tokens from transient rate-limit tokens."
    );
}

/// (d) Codex branch-coverage companion: a stderr with a legitimate Codex
/// persistent-quota signature (`usage cap` / `quota exceeded`) DOES flip
/// `exhausted_at`. This pins that the Codex recognizer fix narrows the
/// rate-limit branch without disabling the legitimate quota path.
#[test]
fn age162_canonical_quota_signature_still_flips_exhausted_at_under_codex_recognizer() {
    let fixture = Fixture::new();
    let owner_marker = fixture.dir.path().join("age162-codex-quota-owner.txt");
    let sibling_marker = fixture.dir.path().join("age162-codex-quota-sibling.txt");

    fixture.write_model("age162-codex-quota-repro", &[CODEX_OWNER, CODEX_SIBLING]);
    fixture.write_providers(&[
        (
            CODEX_OWNER,
            &owner_marker,
            "OpenAI API: usage cap reached for this account; quota exceeded.",
        ),
        (CODEX_SIBLING, &sibling_marker, "connection refused"),
    ]);

    let _output = fixture.run_one_shot("age162-codex-quota-repro");

    let db = fixture.open_db();
    let owner_exhausted = provider_exhausted_at_is_set(&db, CODEX_OWNER);
    assert!(
        owner_exhausted,
        "AGE-162 Symptom 2 branch coverage (Codex recognizer): provider \
         {CODEX_OWNER} did NOT have `exhausted_at` set after a dispatch \
         whose stderr contained the canonical Codex persistent-quota \
         signature (`usage cap` / `quota exceeded`). The legitimate \
         quota-exhausted path must remain functional for the Codex \
         recognizer."
    );
}

/// (e) Same Symptom 2 contract pinned against the OpenAI-compatible
/// `Recognizer` — a dispatch whose stderr is a pure `rate_limit_exceeded` /
/// 429 / `too many requests` / `rate limit exceeded` signature (no
/// `quota exhausted` / `quota exceeded` wording) must NOT flip
/// `exhausted_at`. The OpenAI-compatible recognizer is the default when the
/// provider name does not start with `claude` or `codex`.
#[test]
fn age162_transient_rate_limit_stderr_does_not_mark_exhausted_under_openai_compat_recognizer() {
    let fixture = Fixture::new();
    let owner_marker = fixture.dir.path().join("age162-gemini-rate-owner.txt");
    let sibling_marker = fixture.dir.path().join("age162-gemini-rate-sibling.txt");

    fixture.write_model(
        "age162-gemini-rate-repro",
        &[OPENAI_COMPAT_OWNER, OPENAI_COMPAT_SIBLING],
    );
    fixture.write_providers(&[
        (
            OPENAI_COMPAT_OWNER,
            &owner_marker,
            "429: too many requests; rate_limit_exceeded.",
        ),
        (OPENAI_COMPAT_SIBLING, &sibling_marker, "connection refused"),
    ]);

    let _output = fixture.run_one_shot("age162-gemini-rate-repro");

    let db = fixture.open_db();
    let owner_exhausted = provider_exhausted_at_is_set(&db, OPENAI_COMPAT_OWNER);
    assert!(
        !owner_exhausted,
        "AGE-162 Symptom 2 (OpenAI-compatible recognizer): provider \
         {OPENAI_COMPAT_OWNER} had its `exhausted_at` flipped after a \
         dispatch whose stderr was a transient HTTP 429 / \
         rate_limit_exceeded signature. Bug locus: \
         executor/providers/openai_compat.rs predicate must split \
         persistent-quota tokens from transient rate-limit tokens."
    );
}

/// (f) OpenAI-compatible branch-coverage companion: a stderr with a
/// legitimate persistent-quota signature (`quota exhausted`) DOES flip
/// `exhausted_at`. This pins that the OpenAI-compatible recognizer fix
/// narrows the rate-limit branch without disabling the legitimate quota
/// path.
#[test]
fn age162_canonical_quota_signature_still_flips_exhausted_at_under_openai_compat_recognizer() {
    let fixture = Fixture::new();
    let owner_marker = fixture.dir.path().join("age162-gemini-quota-owner.txt");
    let sibling_marker = fixture.dir.path().join("age162-gemini-quota-sibling.txt");

    fixture.write_model(
        "age162-gemini-quota-repro",
        &[OPENAI_COMPAT_OWNER, OPENAI_COMPAT_SIBLING],
    );
    fixture.write_providers(&[
        (
            OPENAI_COMPAT_OWNER,
            &owner_marker,
            "Gemini API quota exhausted for this project.",
        ),
        (OPENAI_COMPAT_SIBLING, &sibling_marker, "connection refused"),
    ]);

    let _output = fixture.run_one_shot("age162-gemini-quota-repro");

    let db = fixture.open_db();
    let owner_exhausted = provider_exhausted_at_is_set(&db, OPENAI_COMPAT_OWNER);
    assert!(
        owner_exhausted,
        "AGE-162 Symptom 2 branch coverage (OpenAI-compatible recognizer): \
         provider {OPENAI_COMPAT_OWNER} did NOT have `exhausted_at` set \
         after a dispatch whose stderr contained the canonical \
         persistent-quota signature (`quota exhausted`). The legitimate \
         quota-exhausted path must remain functional for the OpenAI-\
         compatible recognizer."
    );
}
