#![cfg(target_os = "linux")]

//! AGE-328 reproduction at the real external-provider/native OpenCode/Bash-tool boundary.
//!
//! Declared roles: fixture, orchestration, validator.
//!
//! This is a manual production-host canary, not a hermetic CI test: generic CI
//! does not have its audited external binaries and source checkouts. Set
//! `OULIPOLY_AGE328_HOST_FIXTURE=1` to enable the host-bound path. Defaults
//! intentionally bind the audited production host; override every affected path,
//! version, commit, and digest together to re-pin another trusted host:
//!
//! - `OULIPOLY_AGE328_PROVIDER_SOURCE`
//! - `OULIPOLY_AGE328_PROVIDER_SOURCE_HEAD`
//! - `OULIPOLY_AGE328_PROVIDER_BIN`
//! - `OULIPOLY_AGE328_PROVIDER_SHA256`
//! - `OULIPOLY_AGE328_NATIVE_OPENCODE_BIN`
//! - `OULIPOLY_AGE328_NATIVE_OPENCODE_VERSION`
//! - `OULIPOLY_AGE328_NATIVE_OPENCODE_SHA256`
//! - `OULIPOLY_AGE328_BUN_BIN`
//! - `OULIPOLY_AGE328_BUN_SHA256`
//! - `OULIPOLY_AGE328_AGENT_BASH_SOURCE`
//! - `OULIPOLY_AGE328_AGENT_BASH_SOURCE_HEAD`
//! - `OULIPOLY_AGE328_AGENT_BASH_BIN`
//! - `OULIPOLY_AGE328_AGENT_BASH_SHA256`
//! - `OULIPOLY_AGE328_BASH_ADAPTER_SOURCE`
//! - `OULIPOLY_AGE328_BASH_ADAPTER_SHA256`

mod provider_authority_fixture;

use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, CompletionEventRegistrationInput, EnqueueResult, MailboxDb,
    WakeClaimAcquireResult, WakeClaimRequest,
};
use oulipoly_state::{InvocationStart, ProviderSessionBinding, StateDb};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const MODEL: &str = "gpt-low";
const PROVIDER: &str = "opencode";
const SETTINGS_ID: &str = "age328-opencode-settings";
const INHERITED_INVOCATION: &str = "32832832-8328-4328-8328-328328328328";
const CLAIM_TOKEN: &str = "age328-contextual-resume-claim";
const HOST_FIXTURE_GATE_ENV: &str = "OULIPOLY_AGE328_HOST_FIXTURE";
const PRIVATE_AUTHORITY_ENV: &str = "OULIPOLY_COMPLETION_REGISTRATION_AUTHORITY";
const MISSING_AUTHORITY_ERROR: &str =
    "process_integrity: completion registration requires caller-bound invocation authority";
const BOOTSTRAP_PROMPT: &str = "AGE328_BOOTSTRAP_NO_TOOL_328: respond without using any tool";
const FRESH_PROMPT: &str = "AGE328_FRESH_TOOL_328: invoke the Bash tool exactly once";
const RESUME_PROMPT: &str = "AGE328_RESUME_TOOL_328: invoke the Bash tool exactly once on resume";

const DEFAULT_PROVIDER_SOURCE: &str =
    "/home/nes/projects/agent-runner-opencode/worktrees/fresh-launch-actor-cleanup";
const DEFAULT_PROVIDER_SOURCE_HEAD: &str = "1c6e2bdec62fc1a2d0712cee6b5d73cb748497aa";
const DEFAULT_PROVIDER_INSTALLED: &str = "/home/nes/.local/bin/agent-runner-opencode";
const DEFAULT_PROVIDER_SHA256: &str =
    "035baba15a908ddabdb5ec6e00f01508103457fccc843765a4b8dae46dc71aa7";
const DEFAULT_NATIVE_OPENCODE: &str = "/home/nes/.opencode/bin/opencode";
const DEFAULT_NATIVE_OPENCODE_VERSION: &str = "1.18.27";
const DEFAULT_NATIVE_OPENCODE_SHA256: &str =
    "bddf894e5c2bc3d8cf452bd6e5ab2273bbe4a37eeeb9aec848d3d7d20db1f256";
const DEFAULT_BUN: &str = "/home/nes/.bun/bin/bun";
const DEFAULT_BUN_SHA256: &str = "077e218c1220703765a8e2b65b2d124b3675c9b0b72172b94fa714a8608c388b";
const DEFAULT_AGENT_BASH_SOURCE: &str = "/home/nes/projects/agent-bash-tool/trunk";
const DEFAULT_AGENT_BASH_SOURCE_HEAD: &str = "2cc78116b0cba2bc4f0007a04a0cec9bce689ce3";
const DEFAULT_AGENT_BASH_INSTALLED: &str = "/home/nes/.local/bin/agent-bash";
const DEFAULT_AGENT_BASH_SHA256: &str =
    "6579e7d9ea3a0eef12a7e36fd44d67c7473ccd36601a85a23c126f472a24904f";
const DEFAULT_BASH_ADAPTER_SOURCE: &str =
    "/home/nes/projects/agent-bash-tool/trunk/integrations/opencode/tools/bash.ts";
const DEFAULT_BASH_ADAPTER_SHA256: &str =
    "b55975d004920476d2dc2f33de364591550146ff681c0d98d13f311c3d29fed1";

struct HostInputs {
    provider_source: PathBuf,
    provider_source_head: String,
    provider_installed: PathBuf,
    provider_sha256: String,
    native_opencode: PathBuf,
    native_opencode_version: String,
    native_opencode_sha256: String,
    bun: PathBuf,
    bun_sha256: String,
    agent_bash_source: PathBuf,
    agent_bash_source_head: String,
    agent_bash_installed: PathBuf,
    agent_bash_sha256: String,
    bash_adapter_source: PathBuf,
    bash_adapter_sha256: String,
}

impl HostInputs {
    fn from_environment() -> Self {
        Self {
            provider_source: host_path("OULIPOLY_AGE328_PROVIDER_SOURCE", DEFAULT_PROVIDER_SOURCE),
            provider_source_head: host_value(
                "OULIPOLY_AGE328_PROVIDER_SOURCE_HEAD",
                DEFAULT_PROVIDER_SOURCE_HEAD,
            ),
            provider_installed: host_path(
                "OULIPOLY_AGE328_PROVIDER_BIN",
                DEFAULT_PROVIDER_INSTALLED,
            ),
            provider_sha256: host_value("OULIPOLY_AGE328_PROVIDER_SHA256", DEFAULT_PROVIDER_SHA256),
            native_opencode: host_path(
                "OULIPOLY_AGE328_NATIVE_OPENCODE_BIN",
                DEFAULT_NATIVE_OPENCODE,
            ),
            native_opencode_version: host_value(
                "OULIPOLY_AGE328_NATIVE_OPENCODE_VERSION",
                DEFAULT_NATIVE_OPENCODE_VERSION,
            ),
            native_opencode_sha256: host_value(
                "OULIPOLY_AGE328_NATIVE_OPENCODE_SHA256",
                DEFAULT_NATIVE_OPENCODE_SHA256,
            ),
            bun: host_path("OULIPOLY_AGE328_BUN_BIN", DEFAULT_BUN),
            bun_sha256: host_value("OULIPOLY_AGE328_BUN_SHA256", DEFAULT_BUN_SHA256),
            agent_bash_source: host_path(
                "OULIPOLY_AGE328_AGENT_BASH_SOURCE",
                DEFAULT_AGENT_BASH_SOURCE,
            ),
            agent_bash_source_head: host_value(
                "OULIPOLY_AGE328_AGENT_BASH_SOURCE_HEAD",
                DEFAULT_AGENT_BASH_SOURCE_HEAD,
            ),
            agent_bash_installed: host_path(
                "OULIPOLY_AGE328_AGENT_BASH_BIN",
                DEFAULT_AGENT_BASH_INSTALLED,
            ),
            agent_bash_sha256: host_value(
                "OULIPOLY_AGE328_AGENT_BASH_SHA256",
                DEFAULT_AGENT_BASH_SHA256,
            ),
            bash_adapter_source: host_path(
                "OULIPOLY_AGE328_BASH_ADAPTER_SOURCE",
                DEFAULT_BASH_ADAPTER_SOURCE,
            ),
            bash_adapter_sha256: host_value(
                "OULIPOLY_AGE328_BASH_ADAPTER_SHA256",
                DEFAULT_BASH_ADAPTER_SHA256,
            ),
        }
    }
}

fn host_path(name: &str, default: &str) -> PathBuf {
    PathBuf::from(host_value(name, default))
}

fn host_value(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Carrier {
    Bootstrap,
    Fresh,
    Resume,
}

impl Carrier {
    fn marker(self) -> &'static str {
        match self {
            Self::Bootstrap => "bootstrap",
            Self::Fresh => "fresh",
            Self::Resume => "resume",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolResultClass {
    MissingAuthority,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RequestObservationKind {
    NoToolResponse,
    ToolCallIssued,
    ToolResult(ToolResultClass),
    Unmatched,
    ProtocolError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RequestObservation {
    carrier: Option<Carrier>,
    kind: RequestObservationKind,
    call_id: Option<String>,
}

struct ResumeIdentity {
    chain_id: String,
    provider_session_id: String,
}

struct CarrierObservation {
    carrier: Carrier,
    invocation_uuid: String,
    provider_session_id: Option<String>,
    chain_id: Option<String>,
    status_code: Option<i32>,
    persisted_success: Option<bool>,
    result_success: Option<bool>,
    result_error_category: Option<String>,
    terminal_signal_kind: Option<String>,
    agent_bash_context: Option<String>,
    issued_call_ids: Vec<String>,
    result_call_ids: Vec<String>,
    fixture_protocol_errors: usize,
    missing_authority_results: usize,
    other_tool_results: usize,
    obligation_count: i64,
    exact_owner_obligation_count: i64,
    workload_count: usize,
    child_parent_matches_current: bool,
    agent_bash_state_entries: usize,
}

impl CarrierObservation {
    fn classification(&self) -> &'static str {
        if self.fixture_protocol_errors != 0 {
            return "fixture-protocol-error";
        }
        if self.issued_call_ids.len() != 1 {
            return "fixture-bash-call-count-mismatch";
        }
        if self.result_call_ids.len() != 1 || self.result_call_ids != self.issued_call_ids {
            return "fixture-bash-result-count-or-call-id-mismatch";
        }
        if self.missing_authority_results == 1 {
            if self.obligation_count == 0
                && self.workload_count == 0
                && self.agent_bash_state_entries == 0
            {
                return "missing-authority-before-supervisor-workload-spawn";
            }
            return "missing-authority-with-spawn-or-registration-evidence";
        }
        if self.other_tool_results != 1 {
            return "tool-result-unclassified";
        }
        if self.status_code == Some(0)
            && self.persisted_success == Some(true)
            && self.provider_session_id.is_some()
            && self.chain_id.is_some()
            && self.obligation_count == 1
            && self.exact_owner_obligation_count == 1
            && self.workload_count == 1
            && self.child_parent_matches_current
        {
            return "green";
        }
        "non-authority-green-invariants-unsatisfied"
    }

    fn report(&self) {
        eprintln!(
            "AGE328_OBSERVATION carrier={} classification={} invocation={} provider_session={} chain={} status_code={} persisted_success={} result_success={} result_error_category={} terminal_signal_kind={} calls={} results={} fixture_protocol_errors={} missing_authority_results={} other_tool_results={} obligations={} exact_owner_obligations={} workloads={} parent_matches_current={} agent_bash_state_entries={} agent_bash_context={}",
            self.carrier.marker(),
            self.classification(),
            self.invocation_uuid,
            self.provider_session_id.as_deref().unwrap_or("none"),
            self.chain_id.as_deref().unwrap_or("none"),
            self.status_code
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
            self.persisted_success
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
            self.result_success
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string()),
            self.result_error_category.as_deref().unwrap_or("none"),
            self.terminal_signal_kind.as_deref().unwrap_or("none"),
            self.issued_call_ids.len(),
            self.result_call_ids.len(),
            self.fixture_protocol_errors,
            self.missing_authority_results,
            self.other_tool_results,
            self.obligation_count,
            self.exact_owner_obligation_count,
            self.workload_count,
            self.child_parent_matches_current,
            self.agent_bash_state_entries,
            self.agent_bash_context.as_deref().unwrap_or("none"),
        );
    }
}

struct Fixture {
    root: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    state_home: PathBuf,
    home: PathBuf,
    models_dir: PathBuf,
    marker: PathBuf,
    agent_bash: PathBuf,
    agent_bash_diagnostic: PathBuf,
    server: MockResponsesServer,
    host_inputs: HostInputs,
}

impl Fixture {
    fn new() -> Self {
        let host_inputs = HostInputs::from_environment();
        verify_source_bound_inputs(&host_inputs);
        let root = tempfile::tempdir().unwrap();
        let config_home = root.path().join("config");
        let data_home = root.path().join("data");
        let state_home = root.path().join("state");
        let home = root.path().join("home");
        let app_config = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config.join("models");
        let marker = root.path().join("workload-markers");
        let tool_bin = root.path().join("tool-bin");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(config_home.join("opencode/tools")).unwrap();
        fs::create_dir_all(&data_home).unwrap();
        fs::create_dir_all(&state_home).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&tool_bin).unwrap();
        fs::create_dir_all(&marker).unwrap();

        let workload_probe = root.path().join("workload-probe.sh");
        fs::write(
            &workload_probe,
            format!(
                "#!/bin/sh\nset -eu\n[ -n \"${{OULIPOLY_PARENT_INVOCATION-}}\" ]\n[ -z \"${{{PRIVATE_AUTHORITY_ENV}+x}}\" ]\nif grep -zq '^{PRIVATE_AUTHORITY_ENV}=.' /proc/$PPID/environ; then exit 91; fi\nprintf '%s\\n' \"$OULIPOLY_PARENT_INVOCATION\" >> \"$1\"\n"
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&workload_probe).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&workload_probe, permissions).unwrap();

        let provider = tool_bin.join("agent-runner-opencode");
        let agent_bash = tool_bin.join("agent-bash");
        let agent_bash_real = tool_bin.join("agent-bash.real");
        let agent_bash_diagnostic = marker.join("agent-bash-context");
        copy_executable(&host_inputs.provider_installed, &provider);
        copy_executable(&host_inputs.agent_bash_installed, &agent_bash_real);
        write_agent_bash_wrapper(
            &agent_bash,
            &agent_bash_real,
            &state_home,
            &home,
            &data_home.join("oulipoly-agent-runner"),
            &agent_bash_diagnostic,
        );
        write_bash_adapter_wrapper(
            &config_home.join("opencode/tools/bash.ts"),
            &agent_bash,
            &state_home,
            &home,
            &data_home.join("oulipoly-agent-runner"),
            &host_inputs.bash_adapter_source,
        );

        let server = MockResponsesServer::start(marker.clone());
        write_opencode_config(&config_home, server.base_url());
        write_runner_config(
            &app_config,
            &provider,
            &config_home,
            &data_home,
            &state_home,
            &home,
            &agent_bash,
        );
        write_provider_settings(&app_config);
        write_native_runtime_binding(&data_home, &home, &host_inputs);

        Self {
            root,
            config_home,
            data_home,
            state_home,
            home,
            models_dir,
            marker,
            agent_bash,
            agent_bash_diagnostic,
            server,
            host_inputs,
        }
    }

    fn state_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn sidecar_path(&self) -> PathBuf {
        MailboxDb::path_for_state_db(&self.state_path())
    }

    fn run_bootstrap(&self) -> Output {
        let mut command = self.runner_command();
        command
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg(MODEL)
            .arg(BOOTSTRAP_PROMPT);
        command.output().unwrap()
    }

    fn run_fresh(&self) -> Output {
        let mut command = self.runner_command();
        command
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg(MODEL)
            .arg(FRESH_PROMPT);
        command.output().unwrap()
    }

    fn run_resume(&self, chain_id: &str) -> Output {
        self.seed_wake_claim(chain_id);
        let mut command = self.runner_command();
        command
            .env("OULIPOLY_AUTO_WAKE_SESSION_ID", chain_id)
            .env("OULIPOLY_AUTO_WAKE_TOKEN", CLAIM_TOKEN)
            .env("OULIPOLY_AUTO_WAKE_COUNT", "1")
            .arg("resume")
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg(MODEL)
            .arg("--session-id")
            .arg(chain_id)
            .arg("--prompt")
            .arg(RESUME_PROMPT);
        command.output().unwrap()
    }

    fn runner_command(&self) -> Command {
        let mut command = Command::new(runner_bin());
        command
            .current_dir(self.root.path())
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("XDG_STATE_HOME", &self.state_home)
            .env("HOME", &self.home)
            .env(
                "OULIPOLY_DATA_DIR",
                self.data_home.join("oulipoly-agent-runner"),
            )
            .env("PATH", native_path(&self.host_inputs))
            .env("OPENAI_API_KEY", "age328-loopback-only")
            .env("AGENT_BASH_BIN", &self.agent_bash)
            .env("AGENT_BASH_AGENT_RUNNER_BIN", runner_bin())
            .env("AGENT_BASH_TOOL_POLL_MS", "10")
            .env("AGENT_BASH_TOOL_PROCESS_TIMEOUT_MS", "30000")
            .env("OULIPOLY_AUTO_WAKE", "1")
            .env("OULIPOLY_PARENT_INVOCATION", inherited_parent_identity())
            .env_remove(PRIVATE_AUTHORITY_ENV)
            .env_remove("AGENT_BASH_OWNER_INVOCATION_UUID")
            .env_remove("AGENT_BASH_OWNER_SESSION_ID");
        command
    }

    fn bootstrap_resume_identity(&self, output: &Output) -> ResumeIdentity {
        if output.status.code() != Some(0) {
            panic!(
                "BLOCKED:bootstrap-runner-failed status={:?}",
                output.status.code()
            );
        }
        let invocation_uuid = parse_current_invocation(output)
            .unwrap_or_else(|reason| panic!("BLOCKED:bootstrap-{reason}"));
        assert_ne!(
            invocation_uuid, INHERITED_INVOCATION,
            "BLOCKED:bootstrap-invocation-reused-inherited-identity"
        );
        let payload = parse_single_marker(&output.stderr, "OULIPOLY_SESSION=")
            .unwrap_or_else(|reason| panic!("BLOCKED:bootstrap-session-{reason}"))
            .unwrap_or_else(|| panic!("BLOCKED:bootstrap-session-marker-missing"));
        let marker_invocation = required_string(&payload, "agent_runner_invocation_id")
            .unwrap_or_else(|reason| panic!("BLOCKED:bootstrap-session-{reason}"));
        let legacy_invocation = required_string(&payload, "id")
            .unwrap_or_else(|reason| panic!("BLOCKED:bootstrap-session-{reason}"));
        let provider_name = required_string(&payload, "provider_name")
            .unwrap_or_else(|reason| panic!("BLOCKED:bootstrap-session-{reason}"));
        let provider_session_id = required_string(&payload, "provider_session_id")
            .unwrap_or_else(|reason| panic!("BLOCKED:bootstrap-session-{reason}"));
        let legacy_session = required_string(&payload, "session_id")
            .unwrap_or_else(|reason| panic!("BLOCKED:bootstrap-session-{reason}"));
        let chain_id = required_string(&payload, "agent_runner_chain_id")
            .unwrap_or_else(|reason| panic!("BLOCKED:bootstrap-session-{reason}"));

        assert_eq!(
            marker_invocation, invocation_uuid,
            "BLOCKED:bootstrap-session-invocation-mismatch"
        );
        assert_eq!(
            legacy_invocation, invocation_uuid,
            "BLOCKED:bootstrap-legacy-invocation-mismatch"
        );
        assert_eq!(
            provider_name, PROVIDER,
            "BLOCKED:bootstrap-provider-mismatch"
        );
        assert_eq!(
            legacy_session, provider_session_id,
            "BLOCKED:bootstrap-session-dual-id-mismatch"
        );

        let state = StateDb::open(&self.state_path()).unwrap();
        let row = state
            .get_invocation_by_uuid(&invocation_uuid)
            .unwrap()
            .unwrap_or_else(|| panic!("BLOCKED:bootstrap-invocation-row-missing"));
        assert_eq!(
            row.provider_name.as_deref(),
            Some(provider_name.as_str()),
            "BLOCKED:bootstrap-state-provider-mismatch"
        );
        assert_eq!(
            row.provider_session_id.as_deref(),
            Some(provider_session_id.as_str()),
            "BLOCKED:bootstrap-state-session-mismatch"
        );
        assert_eq!(
            state
                .chain_id_for_segment(&provider_name, &provider_session_id)
                .unwrap()
                .as_deref(),
            Some(chain_id.as_str()),
            "BLOCKED:bootstrap-state-chain-mismatch"
        );
        assert!(
            self.server.request_count(Carrier::Bootstrap) >= 1,
            "BLOCKED:bootstrap-request-missing"
        );
        assert_eq!(
            self.server.tool_calls(Carrier::Bootstrap),
            0,
            "BLOCKED:bootstrap-issued-tool-call"
        );
        assert_eq!(
            self.server.protocol_error_count(),
            0,
            "BLOCKED:bootstrap-fixture-protocol-error"
        );
        eprintln!(
            "AGE328_BOOTSTRAP invocation={invocation_uuid} provider={provider_name} provider_session={provider_session_id} chain={chain_id} status=ready"
        );
        ResumeIdentity {
            chain_id,
            provider_session_id,
        }
    }

    fn seed_wake_claim(&self, chain_id: &str) {
        let state_dir = self.root.path().join("wake-input");
        fs::create_dir_all(&state_dir).unwrap();
        let meta = state_dir.join("meta.json");
        let log = state_dir.join("log");
        let rc = state_dir.join("rc");
        fs::write(&meta, "{\"caller_chain\":[]}").unwrap();
        fs::write(&log, "nonsecret wake input\n").unwrap();
        fs::write(&rc, "0\n").unwrap();
        let mut mailbox = MailboxDb::open(&self.sidecar_path()).unwrap();
        let row = match mailbox
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: chain_id,
                handle: "age328-wake-input",
                payload_json: "{}",
                owner_invocation_uuid: None,
                matched_os_pid: None,
                matched_os_boot_id: None,
                matched_os_pid_starttime_ticks: None,
                matched_chain_index: None,
                state_dir: state_dir.to_str().unwrap(),
                meta_path: meta.to_str().unwrap(),
                log_path: log.to_str().unwrap(),
                rc_path: rc.to_str().unwrap(),
                rc: 0,
            })
            .unwrap()
        {
            EnqueueResult::Inserted(row) => row,
            other => panic!("expected one isolated wake row, got {other:?}"),
        };
        let claim = mailbox
            .wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: chain_id,
                claim_token: CLAIM_TOKEN,
                reason: "age328_reproduction",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();
        assert!(matches!(claim, WakeClaimAcquireResult::Acquired(_)));
        mailbox
            .mark_delivered(chain_id, None, &[row.seq], "age328-reproduction")
            .unwrap();
    }

    fn observe_carrier(
        &self,
        carrier: Carrier,
        output: &Output,
        agent_bash_state_entries_before: usize,
        protocol_errors_before: usize,
    ) -> CarrierObservation {
        let invocation_uuid = parse_current_invocation(output)
            .unwrap_or_else(|reason| panic!("BLOCKED:{}-{reason}", carrier.marker()));
        assert_ne!(
            invocation_uuid,
            INHERITED_INVOCATION,
            "BLOCKED:{}-invocation-reused-inherited-identity",
            carrier.marker()
        );
        let state = StateDb::open(&self.state_path()).unwrap();
        let row = state
            .get_invocation_by_uuid(&invocation_uuid)
            .unwrap()
            .unwrap_or_else(|| panic!("BLOCKED:{}-invocation-row-missing", carrier.marker()));
        let provider_session_id = row
            .provider_session_id
            .clone()
            .or_else(|| row.session_id.clone());
        let chain_id = match (row.provider_name.as_deref(), provider_session_id.as_deref()) {
            (Some(provider_name), Some(provider_session_id)) => state
                .chain_id_for_segment(provider_name, provider_session_id)
                .unwrap(),
            _ => None,
        };
        let result = parse_single_marker(&output.stderr, "OULIPOLY_RESULT=")
            .unwrap_or_else(|reason| panic!("BLOCKED:{}-result-{reason}", carrier.marker()));
        let result_success = result
            .as_ref()
            .and_then(|value| value.get("success").and_then(Value::as_bool));
        let result_error_category = result.as_ref().and_then(|value| {
            value
                .get("error_category")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        let terminal_signal_kind = parse_single_marker(&output.stderr, "OULIPOLY_TERMINAL_SIGNAL=")
            .unwrap_or_else(|reason| {
                panic!("BLOCKED:{}-terminal-signal-{reason}", carrier.marker())
            })
            .and_then(|value| {
                value
                    .get("kind")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        let requests = self.server.observations(carrier);
        let issued_call_ids = requests
            .iter()
            .filter(|request| request.kind == RequestObservationKind::ToolCallIssued)
            .filter_map(|request| request.call_id.clone())
            .collect::<Vec<_>>();
        let result_call_ids = requests
            .iter()
            .filter(|request| matches!(request.kind, RequestObservationKind::ToolResult(_)))
            .filter_map(|request| request.call_id.clone())
            .collect::<Vec<_>>();
        let missing_authority_results = requests
            .iter()
            .filter(|request| {
                request.kind
                    == RequestObservationKind::ToolResult(ToolResultClass::MissingAuthority)
            })
            .count();
        let other_tool_results = requests
            .iter()
            .filter(|request| {
                request.kind == RequestObservationKind::ToolResult(ToolResultClass::Other)
            })
            .count();
        let connection = Connection::open(self.state_path()).unwrap();
        let obligation_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM invocation_completion_obligations
                 WHERE invocation_uuid = ?1",
                params![invocation_uuid],
                |row| row.get(0),
            )
            .unwrap();
        let exact_owner_obligation_count = provider_session_id
            .as_deref()
            .map(|provider_session_id| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM invocation_completion_obligations
                         WHERE invocation_uuid = ?1 AND owner_invocation_uuid = ?1
                           AND owner_session_id = ?2",
                        params![invocation_uuid, provider_session_id],
                        |row| row.get(0),
                    )
                    .unwrap()
            })
            .unwrap_or(0);
        let workload_path = workload_marker(&self.marker, carrier);
        let workload_count = marker_lines(&workload_path);
        let child_parent_matches_current =
            workload_parent_invocation(&workload_path).as_deref() == Some(invocation_uuid.as_str());
        let observation = CarrierObservation {
            carrier,
            invocation_uuid,
            provider_session_id,
            chain_id,
            status_code: output.status.code(),
            persisted_success: row.success,
            result_success,
            result_error_category,
            terminal_signal_kind,
            agent_bash_context: fs::read_to_string(&self.agent_bash_diagnostic)
                .unwrap_or_default()
                .lines()
                .last()
                .map(str::to_string),
            issued_call_ids,
            result_call_ids,
            fixture_protocol_errors: self
                .server
                .protocol_error_count()
                .saturating_sub(protocol_errors_before),
            missing_authority_results,
            other_tool_results,
            obligation_count,
            exact_owner_obligation_count,
            workload_count,
            child_parent_matches_current,
            agent_bash_state_entries: descendant_file_count(&self.state_home.join("agent-bash"))
                .saturating_sub(agent_bash_state_entries_before),
        };
        observation.report();
        observation
    }
}

#[test]
fn contextual_fresh_and_resume_deliver_current_authority_to_real_bash_host() {
    assert_invalid_authority_matrix_rejects_before_spawn();
    if std::env::var(HOST_FIXTURE_GATE_ENV).as_deref() != Ok("1") {
        eprintln!("AGE328_SKIPPED set {HOST_FIXTURE_GATE_ENV}=1 to run the host-bound canary");
        return;
    }
    let fixture = Fixture::new();

    let bootstrap = fixture.run_bootstrap();
    let resume_identity = fixture.bootstrap_resume_identity(&bootstrap);

    let fresh_state_entries = descendant_file_count(&fixture.state_home.join("agent-bash"));
    let fresh_protocol_errors = fixture.server.protocol_error_count();
    let fresh = fixture.run_fresh();
    let fresh_observation = fixture.observe_carrier(
        Carrier::Fresh,
        &fresh,
        fresh_state_entries,
        fresh_protocol_errors,
    );

    let resume_state_entries = descendant_file_count(&fixture.state_home.join("agent-bash"));
    let resume_protocol_errors = fixture.server.protocol_error_count();
    let resumed = fixture.run_resume(&resume_identity.chain_id);
    let resume_observation = fixture.observe_carrier(
        Carrier::Resume,
        &resumed,
        resume_state_entries,
        resume_protocol_errors,
    );

    if resume_observation.classification() == "green" {
        assert_eq!(
            resume_observation.chain_id.as_deref(),
            Some(resume_identity.chain_id.as_str()),
            "BLOCKED:resume-green-chain-continuity-mismatch"
        );
        assert_eq!(
            resume_observation.provider_session_id.as_deref(),
            Some(resume_identity.provider_session_id.as_str()),
            "BLOCKED:resume-green-session-continuity-mismatch"
        );
    }
    assert_ne!(
        resume_observation.invocation_uuid, fresh_observation.invocation_uuid,
        "BLOCKED:fresh-resume-invocation-identity-collision"
    );
    assert_eq!(
        (
            fresh_observation.classification(),
            resume_observation.classification()
        ),
        ("green", "green"),
        "fresh={}; resume={}; required behavior rejects this pre-fix result when observed: {MISSING_AUTHORITY_ERROR}",
        fresh_observation.classification(),
        resume_observation.classification(),
    );
}

fn assert_invalid_authority_matrix_rejects_before_spawn() {
    let root = tempfile::tempdir().unwrap();

    let missing_state = StateDb::open(&root.path().join("missing.db")).unwrap();
    let missing_target = "32800000-0000-4000-8000-000000000001";
    missing_state
        .start_invocation(&invocation_start(missing_target))
        .unwrap();
    let unrelated = missing_state
        .start_invocation_with_completion_registration_authority(&invocation_start(
            "32800000-0000-4000-8000-000000000002",
        ))
        .unwrap();
    let mut missing_state = missing_state;
    assert_process_integrity(
        missing_state
            .register_completion_event_with_authority(
                &unrelated.completion_registration_authority,
                "age328-missing",
                registration(missing_target, "missing-session"),
            )
            .unwrap_err(),
    );

    assert_process_integrity(
        oulipoly_state::CompletionRegistrationAuthority::from_process_environment_value(
            "malformed",
        )
        .unwrap_err(),
    );

    let stale_path = root.path().join("stale.db");
    let stale_state = StateDb::open(&stale_path).unwrap();
    let stale_owner = "32800000-0000-4000-8000-000000000010";
    let current_owner = "32800000-0000-4000-8000-000000000011";
    let (stale_row_id, stale_authority) =
        start_bound_invocation(&stale_state, stale_owner, "stale-session");
    stale_state
        .finalize_invocation(stale_row_id, true, 0, None, Some("completed"))
        .unwrap();
    start_bound_invocation(&stale_state, current_owner, "current-session");
    let mut stale_state = stale_state;
    assert_process_integrity(
        stale_state
            .register_completion_event_with_authority(
                &stale_authority,
                "stale",
                registration(current_owner, "current-session"),
            )
            .unwrap_err(),
    );

    let wrong_invocation_path = root.path().join("wrong-invocation.db");
    let wrong_invocation_state = StateDb::open(&wrong_invocation_path).unwrap();
    let authority_owner = "32800000-0000-4000-8000-000000000020";
    let registration_owner = "32800000-0000-4000-8000-000000000021";
    let (_, foreign_authority) = start_bound_invocation(
        &wrong_invocation_state,
        authority_owner,
        "authority-session",
    );
    start_bound_invocation(
        &wrong_invocation_state,
        registration_owner,
        "registration-session",
    );
    let mut wrong_invocation_state = wrong_invocation_state;
    assert_process_integrity(
        wrong_invocation_state
            .register_completion_event_with_authority(
                &foreign_authority,
                "wrong-invocation",
                registration(registration_owner, "registration-session"),
            )
            .unwrap_err(),
    );

    let wrong_session_path = root.path().join("wrong-session.db");
    let state = StateDb::open(&wrong_session_path).unwrap();
    let owner = "32800000-0000-4000-8000-000000000030";
    let (_, authority) = start_bound_invocation(&state, owner, "authoritative-session");
    let mut state = state;
    assert_process_integrity(
        state
            .register_completion_event_with_authority(
                &authority,
                "wrong-session",
                registration(owner, "different-session"),
            )
            .unwrap_err(),
    );

    for entry in fs::read_dir(root.path()).unwrap().flatten() {
        if entry.path().extension().and_then(|value| value.to_str()) == Some("db")
            && entry.file_name() != "pid-identity.db"
        {
            let count: i64 = Connection::open(entry.path())
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM invocation_completion_obligations",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0, "invalid row persisted a registration");
        }
    }
}

fn invocation_start(invocation_uuid: &str) -> InvocationStart {
    InvocationStart {
        invocation_uuid: invocation_uuid.to_string(),
        model_name: MODEL.to_string(),
        provider_name: PROVIDER.to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    }
}

fn start_bound_invocation(
    state: &StateDb,
    invocation_uuid: &str,
    session_id: &str,
) -> (i64, oulipoly_state::CompletionRegistrationAuthority) {
    let started = state
        .start_invocation_with_completion_registration_authority(&invocation_start(invocation_uuid))
        .unwrap();
    state
        .bind_invocation_provider_session_start(
            started.invocation_row_id,
            &ProviderSessionBinding {
                provider_session_id: session_id.to_string(),
                capture_method: "age328-negative-fixture",
                resume_input_id: None,
                provider_session_resolved_account: Some(PROVIDER.to_string()),
            },
        )
        .unwrap();
    (
        started.invocation_row_id,
        started.completion_registration_authority,
    )
}

fn registration<'a>(
    owner_invocation_uuid: &'a str,
    owner_session_id: &'a str,
) -> CompletionEventRegistrationInput<'a> {
    CompletionEventRegistrationInput {
        event_id: "age328-invalid-event",
        delivery_mode: "async",
        owner_session_id: Some(owner_session_id),
        owner_invocation_uuid: Some(owner_invocation_uuid),
        state_dir: "/tmp/age328-invalid-state",
        meta_path: "/tmp/age328-invalid-meta",
        log_path: "/tmp/age328-invalid-log",
        rc_path: "/tmp/age328-invalid-rc",
    }
}

fn assert_process_integrity(error: String) {
    assert!(error.starts_with("process_integrity:"), "{error}");
}

fn parse_current_invocation(output: &Output) -> Result<String, String> {
    let payload = parse_single_marker(&output.stderr, "OULIPOLY_INVOCATION=")?
        .ok_or_else(|| "invocation-marker-missing".to_string())?;
    required_string(&payload, "id")
}

fn parse_single_marker(bytes: &[u8], prefix: &str) -> Result<Option<Value>, String> {
    let text = String::from_utf8_lossy(bytes);
    let rows = text
        .lines()
        .filter_map(|line| line.strip_prefix(prefix))
        .collect::<Vec<_>>();
    if rows.len() > 1 {
        return Err(format!("duplicate-marker-{prefix}"));
    }
    rows.first()
        .map(|row| serde_json::from_str(row).map_err(|_| format!("invalid-marker-{prefix}")))
        .transpose()
}

fn required_string(value: &Value, field: &str) -> Result<String, String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("missing-{field}"))
}

fn workload_marker(root: &Path, carrier: Carrier) -> PathBuf {
    root.join(carrier.marker())
}

fn marker_lines(path: &Path) -> usize {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

fn workload_parent_invocation(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;
    let mut lines = contents.lines().filter(|line| !line.trim().is_empty());
    let value = serde_json::from_str::<Value>(lines.next()?).ok()?;
    if lines.next().is_some() {
        return None;
    }
    value.get("id")?.as_str().map(str::to_string)
}

fn descendant_file_count(path: &Path) -> usize {
    if path.is_file() {
        return 1;
    }
    fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| descendant_file_count(&entry.path()))
        .sum()
}

fn runner_bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
}

fn verify_source_bound_inputs(inputs: &HostInputs) {
    assert_eq!(
        git_head(&inputs.provider_source),
        inputs.provider_source_head
    );
    assert_eq!(
        git_head(&inputs.agent_bash_source),
        inputs.agent_bash_source_head
    );
    for (label, path, expected) in [
        ("runner", runner_bin(), None),
        (
            "agent-runner-opencode",
            inputs.provider_installed.as_path(),
            Some(inputs.provider_sha256.as_str()),
        ),
        (
            "native-opencode",
            inputs.native_opencode.as_path(),
            Some(inputs.native_opencode_sha256.as_str()),
        ),
        (
            "bun",
            inputs.bun.as_path(),
            Some(inputs.bun_sha256.as_str()),
        ),
        (
            "agent-bash",
            inputs.agent_bash_installed.as_path(),
            Some(inputs.agent_bash_sha256.as_str()),
        ),
        (
            "bash-adapter",
            inputs.bash_adapter_source.as_path(),
            Some(inputs.bash_adapter_sha256.as_str()),
        ),
    ] {
        let canonical = fs::canonicalize(path).unwrap_or_else(|error| {
            panic!(
                "BLOCKED:source-bound-real-boundary-fixture-unavailable: {label} {}: {error}",
                path.display()
            )
        });
        let digest = sha256_file(&canonical);
        if let Some(expected) = expected {
            assert_eq!(
                digest, expected,
                "BLOCKED:source-bound-real-boundary-fixture-unavailable: {label} identity changed"
            );
        }
        eprintln!(
            "AGE328_EXECUTABLE {label} path={} sha256={digest}",
            canonical.display()
        );
    }
    eprintln!(
        "AGE328_SOURCE agent-runner-opencode path={} head={}",
        inputs.provider_source.display(),
        inputs.provider_source_head
    );
    eprintln!(
        "AGE328_SOURCE agent-bash-tool path={} head={}",
        inputs.agent_bash_source.display(),
        inputs.agent_bash_source_head
    );
}

fn git_head(path: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cannot identify source at {}",
        path.display()
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn sha256_file(path: &Path) -> String {
    let mut file = fs::File::open(path).unwrap();
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    format!("{:x}", digest.finalize())
}

fn copy_executable(source: &Path, target: &Path) {
    fs::copy(source, target).unwrap();
    let mut permissions = fs::metadata(target).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(target, permissions).unwrap();
}

fn native_path(inputs: &HostInputs) -> std::ffi::OsString {
    std::env::join_paths([
        inputs.native_opencode.parent().unwrap(),
        inputs.bun.parent().unwrap(),
        Path::new("/usr/local/bin"),
        Path::new("/usr/bin"),
        Path::new("/bin"),
    ])
    .unwrap()
}

fn inherited_parent_identity() -> String {
    json!({"source": "age328-inherited", "id": INHERITED_INVOCATION}).to_string()
}

fn write_runner_config(
    app_config: &Path,
    provider: &Path,
    config_home: &Path,
    data_home: &Path,
    state_home: &Path,
    home: &Path,
    agent_bash: &Path,
) {
    fs::write(
        app_config.join("models").join(format!("{MODEL}.toml")),
        format!(
            "provider = {{ path = {:?} }}\nprompt_mode = \"arg\"\n\n[[providers]]\nname = {PROVIDER:?}\nargs = [\"-m\", \"openai/gpt-5.6-sol\", \"--variant\", \"low\"]\n",
            provider.display().to_string()
        ),
    )
    .unwrap();
    let config = format!(
        "[{PROVIDER}]\ncommand = \"opencode1\"\nargs = [\"--pure\", \"run\", \"--dangerously-skip-permissions\"]\ninteractive_args = []\nprompt_mode = \"arg\"\nsettings_id = {SETTINGS_ID:?}\nunset_environment = [\"XDG_DATA_HOME\"]\nenvironment = {{ XDG_CONFIG_HOME = {:?}, XDG_STATE_HOME = {:?}, HOME = {:?}, OULIPOLY_DATA_DIR = {:?}, AGENT_BASH_BIN = {:?}, AGENT_BASH_AGENT_RUNNER_BIN = {:?}, AGENT_BASH_TOOL_POLL_MS = \"10\", AGENT_BASH_TOOL_PROCESS_TIMEOUT_MS = \"30000\" }}\n",
        config_home.display().to_string(),
        state_home.display().to_string(),
        home.display().to_string(),
        data_home
            .join("oulipoly-agent-runner")
            .display()
            .to_string(),
        agent_bash.display().to_string(),
        runner_bin().display().to_string(),
    );
    fs::write(
        app_config.join("providers.toml"),
        provider_authority_fixture::with_explicit_provider_authority_at(
            &config, "opencode", provider,
        ),
    )
    .unwrap();
}

fn write_agent_bash_wrapper(
    wrapper: &Path,
    agent_bash: &Path,
    state_home: &Path,
    home: &Path,
    data_dir: &Path,
    diagnostic: &Path,
) {
    fs::write(
        wrapper,
        format!(
            "#!/bin/sh\nset -eu\nexport XDG_STATE_HOME='{}'\nexport HOME='{}'\nexport OULIPOLY_DATA_DIR='{}'\nauthority_present=false\nif [ \"${{{PRIVATE_AUTHORITY_ENV}+x}}\" = x ]; then authority_present=true; fi\nprintf '%s\\t%s\\t%s\\t%s\\n' \"${{OULIPOLY_PARENT_INVOCATION-}}\" \"${{AGENT_BASH_OWNER_INVOCATION_UUID-}}\" \"${{AGENT_BASH_OWNER_SESSION_ID-}}\" \"$authority_present\" >> '{}'\nexec '{}' \"$@\"\n",
            state_home.display(),
            home.display(),
            data_dir.display(),
            diagnostic.display(),
            agent_bash.display(),
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(wrapper).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(wrapper, permissions).unwrap();
}

fn write_bash_adapter_wrapper(
    wrapper: &Path,
    agent_bash: &Path,
    state_home: &Path,
    home: &Path,
    data_dir: &Path,
    source_path: &Path,
) {
    let source = fs::read_to_string(source_path).unwrap();
    fs::write(
        wrapper,
        format!(
            "process.env.XDG_STATE_HOME = {};\nprocess.env.HOME = {};\nprocess.env.OULIPOLY_DATA_DIR = {};\nprocess.env.AGENT_BASH_BIN = {};\nprocess.env.AGENT_BASH_AGENT_RUNNER_BIN = {};\nprocess.env.AGENT_BASH_TOOL_POLL_MS = \"10\";\nprocess.env.AGENT_BASH_TOOL_PROCESS_TIMEOUT_MS = \"30000\";\n{source}",
            json!(state_home.display().to_string()),
            json!(home.display().to_string()),
            json!(data_dir.display().to_string()),
            json!(agent_bash.display().to_string()),
            json!(runner_bin().display().to_string()),
        ),
    )
    .unwrap();
}

fn write_provider_settings(app_config: &Path) {
    let directory = app_config.join("agent-runner-opencode");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("settings-store.json"),
        json!({
            "schema_version": 3,
            "records": [{
                "id": SETTINGS_ID,
                "display_name": "AGE-328 isolated OpenCode",
                "version": "v328000000000000000000000",
                "values": {
                    "provider": "opencode",
                    "profile": "opencode1",
                    "wrapper": "opencode1",
                    "model": {
                        "name": MODEL,
                        "provider_model": "openai/gpt-5.6-sol",
                        "variant": "low"
                    },
                    "quota": {
                        "source": "opencode_auth",
                        "auth_path": "~/.local/share/opencode/auth.json",
                        "probe": "native_chatgpt_usage"
                    },
                    "launch": {"format": "json", "dangerously_skip_permissions": true}
                }
            }],
            "history": [],
            "mutation_receipts": {}
        })
        .to_string(),
    )
    .unwrap();
}

fn write_native_runtime_binding(data_home: &Path, home: &Path, inputs: &HostInputs) {
    let program = fs::canonicalize(&inputs.native_opencode).unwrap();
    let metadata = fs::metadata(&program).unwrap();
    let mut execution_env = BTreeMap::new();
    execution_env.insert("HOME", home.display().to_string());
    execution_env.insert(
        "OPENCODE_EXPERIMENTAL_BASH_DEFAULT_TIMEOUT_MS",
        "2000000000".to_string(),
    );
    execution_env.insert("OULIPOLY_OPENCODE_ACCOUNT", "opencode1".to_string());
    let fixed_args = vec!["--pure".to_string()];
    let implementation_manifest_id = format!(
        "opencode-auto-update-{}-{}",
        inputs.native_opencode_version,
        &inputs.native_opencode_sha256[..8]
    );
    let identity = json!({
        "account_wrapper": "opencode1",
        "program": program.display().to_string(),
        "program_sha256": inputs.native_opencode_sha256,
        "execution_env": execution_env,
        "native_contract_id": "agent-runner-opencode.opencode-native-state/v1",
        "fixed_args": fixed_args,
        "implementation_manifest_id": implementation_manifest_id,
        "implementation_version": inputs.native_opencode_version,
    })
    .to_string();
    let identity_sha256 = format!("{:x}", Sha256::digest(identity.as_bytes()));
    let record = json!({
        "schema_version": 6,
        "account_wrapper": "opencode1",
        "program": program.display().to_string(),
        "program_sha256": inputs.native_opencode_sha256,
        "execution_env": execution_env,
        "native_contract_id": "agent-runner-opencode.opencode-native-state/v1",
        "fixed_args": fixed_args,
        "implementation_manifest_id": implementation_manifest_id,
        "implementation_version": inputs.native_opencode_version,
        "program_stamp": {
            "kind": "unix-metadata-v1",
            "byte_length": metadata.len(),
            "device": metadata.dev(),
            "inode": metadata.ino(),
            "modified_seconds": metadata.mtime(),
            "modified_nanoseconds": metadata.mtime_nsec(),
            "changed_seconds": metadata.ctime(),
            "changed_nanoseconds": metadata.ctime_nsec(),
        },
        "identity_sha256": identity_sha256,
    });
    let directory = data_home
        .join("oulipoly-agent-runner")
        .join("provider-state/opencode/native-runtimes");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("opencode1.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
}

fn write_opencode_config(config_home: &Path, base_url: &str) {
    fs::write(
        config_home.join("opencode/opencode.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema": "https://opencode.ai/config.json",
            "permission": {"*": "allow"},
            "tools": {"bash": true},
            "provider": {
                "openai": {
                    "options": {"baseURL": base_url, "apiKey": "age328-loopback-only"},
                    "models": {
                        "gpt-5.6-sol": {
                            "name": "AGE-328 loopback model",
                            "limit": {"context": 16000, "output": 2000}
                        }
                    }
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
}

struct MockResponsesServer {
    base_url: String,
    requests: Arc<Mutex<Vec<RequestObservation>>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl MockResponsesServer {
    fn start(marker: PathBuf) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let shutdown = Arc::new(AtomicBool::new(false));
        let server_shutdown = Arc::clone(&shutdown);
        let thread =
            thread::spawn(move || serve_responses(listener, marker, recorded, server_shutdown));
        Self {
            base_url: format!("http://{address}/v1"),
            requests,
            shutdown,
            thread: Some(thread),
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn tool_calls(&self, carrier: Carrier) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|row| {
                row.carrier == Some(carrier) && row.kind == RequestObservationKind::ToolCallIssued
            })
            .count()
    }

    fn request_count(&self, carrier: Carrier) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|row| row.carrier == Some(carrier))
            .count()
    }

    fn observations(&self, carrier: Carrier) -> Vec<RequestObservation> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|row| row.carrier == Some(carrier))
            .cloned()
            .collect()
    }

    fn protocol_error_count(&self) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|row| row.kind == RequestObservationKind::ProtocolError)
            .count()
    }
}

impl Drop for MockResponsesServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve_responses(
    listener: TcpListener,
    marker: PathBuf,
    requests: Arc<Mutex<Vec<RequestObservation>>>,
    shutdown: Arc<AtomicBool>,
) {
    while !shutdown.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                if stream
                    .set_read_timeout(Some(Duration::from_secs(30)))
                    .is_err()
                    || stream
                        .set_write_timeout(Some(Duration::from_secs(30)))
                        .is_err()
                {
                    record_protocol_error(&requests);
                    continue;
                }
                respond(stream, &marker, &requests);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => {
                record_protocol_error(&requests);
                return;
            }
        }
    }
}

fn respond(mut stream: TcpStream, marker: &Path, requests: &Arc<Mutex<Vec<RequestObservation>>>) {
    let Ok(cloned) = stream.try_clone() else {
        record_protocol_error(requests);
        return;
    };
    let mut reader = BufReader::new(cloned);
    let mut content_length = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) if line == "\r\n" => break,
            Ok(_) => {}
            Err(_) => {
                record_protocol_error(requests);
                return;
            }
        }
        let lowercase = line.to_ascii_lowercase();
        if let Some(value) = lowercase.strip_prefix("content-length:") {
            let Ok(length) = value.trim().parse() else {
                eprintln!("AGE328_FIXTURE_INVALID_CONTENT_LENGTH");
                record_protocol_error(requests);
                return;
            };
            content_length = Some(length);
        }
        if let Some(value) = lowercase.strip_prefix("transfer-encoding:") {
            chunked = value
                .split(',')
                .any(|encoding| encoding.trim() == "chunked");
        }
    }
    if chunked {
        eprintln!("AGE328_FIXTURE_UNSUPPORTED_REQUEST_FRAMING encoding=chunked");
        record_protocol_error(requests);
        return;
    }
    let Some(content_length) = content_length else {
        eprintln!("AGE328_FIXTURE_UNSUPPORTED_REQUEST_FRAMING content_length=missing");
        record_protocol_error(requests);
        return;
    };
    let mut request_body = vec![0_u8; content_length];
    if reader.read_exact(&mut request_body).is_err() {
        record_protocol_error(requests);
        return;
    }
    let observation = request_observation(&request_body);
    let payload = match (observation.carrier, &observation.kind) {
        (
            Some(carrier @ (Carrier::Fresh | Carrier::Resume)),
            RequestObservationKind::ToolCallIssued,
        ) => tool_response(carrier, marker),
        (Some(carrier), _) => final_response(carrier),
        (None, _) => final_response(Carrier::Bootstrap),
    };
    requests.lock().unwrap().push(observation);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        payload.len(),
        payload
    );
    if stream.write_all(response.as_bytes()).is_err() || stream.flush().is_err() {
        record_protocol_error(requests);
    }
}

fn record_protocol_error(requests: &Arc<Mutex<Vec<RequestObservation>>>) {
    requests.lock().unwrap().push(RequestObservation {
        carrier: None,
        kind: RequestObservationKind::ProtocolError,
        call_id: None,
    });
}

fn request_observation(request_body: &[u8]) -> RequestObservation {
    let Ok(payload) = serde_json::from_slice::<Value>(request_body) else {
        return RequestObservation {
            carrier: None,
            kind: RequestObservationKind::ProtocolError,
            call_id: None,
        };
    };
    if let Some((carrier, call_id, output)) = request_tool_result(&payload) {
        return RequestObservation {
            carrier: Some(carrier),
            kind: RequestObservationKind::ToolResult(if output.contains(MISSING_AUTHORITY_ERROR) {
                ToolResultClass::MissingAuthority
            } else {
                ToolResultClass::Other
            }),
            call_id: Some(call_id),
        };
    }
    let carrier = [Carrier::Resume, Carrier::Fresh, Carrier::Bootstrap]
        .into_iter()
        .find(|carrier| value_contains(&payload, carrier_prompt(*carrier)));
    let kind = match carrier {
        Some(Carrier::Bootstrap) => RequestObservationKind::NoToolResponse,
        Some(Carrier::Fresh | Carrier::Resume) if request_has_bash_tool(&payload) => {
            RequestObservationKind::ToolCallIssued
        }
        Some(Carrier::Fresh | Carrier::Resume) => RequestObservationKind::NoToolResponse,
        None => RequestObservationKind::Unmatched,
    };
    let call_id = matches!(kind, RequestObservationKind::ToolCallIssued)
        .then(|| expected_call_id(carrier.expect("tool request carrier")));
    RequestObservation {
        carrier,
        call_id,
        kind,
    }
}

fn request_has_bash_tool(payload: &Value) -> bool {
    payload
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("name").and_then(Value::as_str) == Some("bash")
                    || tool
                        .get("function")
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_str)
                        == Some("bash")
            })
        })
}

fn request_tool_result(payload: &Value) -> Option<(Carrier, String, String)> {
    match payload {
        Value::Object(fields) => {
            if fields.get("type").and_then(Value::as_str) == Some("function_call_output") {
                let call_id = fields.get("call_id")?.as_str()?;
                let carrier = carrier_from_call_id(call_id)?;
                let output = fields
                    .get("output")
                    .and_then(tool_output_text)
                    .unwrap_or_default();
                return Some((carrier, call_id.to_string(), output));
            }
            fields.values().find_map(request_tool_result)
        }
        Value::Array(values) => values.iter().find_map(request_tool_result),
        _ => None,
    }
}

fn tool_output_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Array(values) => Some(
            values
                .iter()
                .filter_map(tool_output_text)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        Value::Object(fields) => fields
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| fields.values().find_map(tool_output_text)),
        _ => None,
    }
}

fn carrier_from_call_id(call_id: &str) -> Option<Carrier> {
    [Carrier::Fresh, Carrier::Resume]
        .into_iter()
        .find(|carrier| call_id == expected_call_id(*carrier))
}

fn expected_call_id(carrier: Carrier) -> String {
    format!("call_age328_{}", carrier.marker())
}

fn carrier_prompt(carrier: Carrier) -> &'static str {
    match carrier {
        Carrier::Bootstrap => BOOTSTRAP_PROMPT,
        Carrier::Fresh => FRESH_PROMPT,
        Carrier::Resume => RESUME_PROMPT,
    }
}

fn value_contains(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(value) => value.contains(needle),
        Value::Array(values) => values.iter().any(|value| value_contains(value, needle)),
        Value::Object(fields) => fields.values().any(|value| value_contains(value, needle)),
        _ => false,
    }
}

fn tool_response(carrier: Carrier, marker: &Path) -> String {
    let call_id = expected_call_id(carrier);
    let item_id = format!("fc_age328_{}", carrier.marker());
    let arguments = json!({
        "command": workload_command(carrier, marker),
        "delivery": "sync"
    })
    .to_string();
    let item = json!({
        "id": item_id,
        "type": "function_call",
        "status": "completed",
        "name": "bash",
        "call_id": call_id,
        "arguments": arguments
    });
    let response_id = format!("resp_age328_{}_tool", carrier.marker());
    sse(&[
        response_created(&response_id),
        json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"id":item["id"],"type":"function_call","status":"in_progress","name":"bash","call_id":item["call_id"],"arguments":""}}),
        json!({"type":"response.function_call_arguments.delta","sequence_number":2,"item_id":item["id"],"output_index":0,"delta":arguments}),
        json!({"type":"response.function_call_arguments.done","sequence_number":3,"item_id":item["id"],"output_index":0,"name":"bash","arguments":item["arguments"]}),
        json!({"type":"response.output_item.done","sequence_number":4,"output_index":0,"item":item}),
        response_completed(&response_id, vec![item]),
    ])
}

fn final_response(carrier: Carrier) -> String {
    let response_id = format!("resp_age328_{}_done", carrier.marker());
    let message_id = format!("msg_age328_{}_done", carrier.marker());
    let item = json!({
        "id": message_id,
        "type": "message",
        "status": "completed",
        "role": "assistant",
        "content": [{"type":"output_text","text":"done","annotations":[],"logprobs":[]}]
    });
    sse(&[
        response_created(&response_id),
        json!({"type":"response.output_item.added","sequence_number":1,"output_index":0,"item":{"id":message_id,"type":"message","status":"in_progress","role":"assistant","content":[]}}),
        json!({"type":"response.content_part.added","sequence_number":2,"item_id":message_id,"output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[],"logprobs":[]}}),
        json!({"type":"response.output_text.delta","sequence_number":3,"item_id":message_id,"output_index":0,"content_index":0,"delta":"done","logprobs":[]}),
        json!({"type":"response.output_text.done","sequence_number":4,"item_id":message_id,"output_index":0,"content_index":0,"text":"done","logprobs":[]}),
        json!({"type":"response.content_part.done","sequence_number":5,"item_id":message_id,"output_index":0,"content_index":0,"part":item["content"][0]}),
        json!({"type":"response.output_item.done","sequence_number":6,"output_index":0,"item":item}),
        response_completed(&response_id, vec![item]),
    ])
}

fn response_created(id: &str) -> Value {
    json!({"type":"response.created","sequence_number":0,"response":response_object(id, "in_progress", vec![])})
}

fn response_completed(id: &str, output: Vec<Value>) -> Value {
    json!({"type":"response.completed","sequence_number":7,"response":response_object(id, "completed", output)})
}

fn response_object(id: &str, status: &str, output: Vec<Value>) -> Value {
    json!({
        "id": id,
        "object": "response",
        "created_at": 1788400000,
        "status": status,
        "background": false,
        "error": null,
        "incomplete_details": null,
        "instructions": null,
        "max_output_tokens": 2000,
        "max_tool_calls": null,
        "model": "gpt-5.6-sol",
        "output": output,
        "parallel_tool_calls": false,
        "previous_response_id": null,
        "prompt_cache_key": null,
        "reasoning": {"effort":"low","summary":null},
        "safety_identifier": null,
        "service_tier": "default",
        "store": false,
        "temperature": 1.0,
        "text": {"format":{"type":"text"},"verbosity":"medium"},
        "tool_choice": "auto",
        "tools": [],
        "top_logprobs": 0,
        "top_p": 1.0,
        "truncation": "disabled",
        "usage": if status == "completed" { json!({"input_tokens":10,"input_tokens_details":{"cached_tokens":0},"output_tokens":5,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":15}) } else { Value::Null },
        "user": null,
        "metadata": {}
    })
}

fn sse(events: &[Value]) -> String {
    let mut body = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    body.push_str("data: [DONE]\n\n");
    body
}

fn workload_command(carrier: Carrier, marker: &Path) -> String {
    let probe = marker.parent().unwrap().join("workload-probe.sh");
    let output = workload_marker(marker, carrier);
    format!(
        "{probe:?} {output:?}",
        probe = probe.display().to_string(),
        output = output.display().to_string(),
    )
}
