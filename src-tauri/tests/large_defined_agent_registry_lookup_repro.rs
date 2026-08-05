#![cfg(unix)]

use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PROVIDER_INLINE_ARG_LIMIT: usize = 64 * 1024;
const INCIDENT_DEFINITION_BYTES: usize = 69_876;
const RAW_PROMPT_BYTES: usize = 106;
const FIXED_PROVIDER_ARGV: [&str; 10] = [
    "opencode-fixture",
    "--pure",
    "run",
    "--dangerously-skip-permissions",
    "--format",
    "json",
    "-m",
    "openai/gpt-5.5",
    "--variant",
    "high",
];

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
    project_dir: PathBuf,
    observations_dir: PathBuf,
}

struct CaseResult {
    name: &'static str,
    definition_bytes: Option<usize>,
    rendered_bytes: usize,
    launch: Value,
    output: Output,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let project_dir = dir.path().join("project");
        let observations_dir = dir.path().join("observations");
        fs::create_dir_all(&models_dir).expect("models dir");
        fs::create_dir_all(&project_dir).expect("project dir");
        fs::create_dir_all(&observations_dir).expect("observations dir");

        let fixture = Self {
            _dir: dir,
            config_home,
            data_home,
            models_dir,
            project_dir,
            observations_dir,
        };
        fixture.write_config();
        fixture
    }

    fn write_config(&self) {
        let provider = self.write_external_provider();
        fs::write(
            self.models_dir.join("fixture.toml"),
            format!(
                r#"provider = {{ path = {:?} }}

[[providers]]
name = "fixture-opencode"
args = ["-m", "openai/gpt-5.5", "--variant", "high"]
"#,
                provider.to_string_lossy()
            ),
        )
        .expect("model config");
        fs::write(
            self.config_home
                .join("oulipoly-agent-runner")
                .join("providers.toml"),
            r#"[fixture-opencode]
command = "opencode-fixture"
args = ["--pure", "run", "--dangerously-skip-permissions", "--format", "json"]
prompt_mode = "arg"
environment = { FIXTURE_LAUNCH_CONTEXT = "fixed-external-provider-context" }
"#,
        )
        .expect("provider config");
    }

    fn write_external_provider(&self) -> PathBuf {
        let path = self._dir.path().join("fixture-external-provider.py");
        let observations = serde_json::to_string(&self.observations_dir.display().to_string())
            .expect("python observations path");
        fs::write(
            &path,
            format!(
                r#"#!/usr/bin/env python3
import base64
import json
import os
import pathlib
import sys

CONTRACT = "oulipoly.provider/v1"
OBSERVATIONS = pathlib.Path({observations})

def read_request():
    request = json.loads(sys.stdin.read())
    if request.get("contract") != CONTRACT:
        raise ValueError("unexpected contract")
    if not request.get("request_id"):
        raise ValueError("missing request id")
    return request

def response(request, result):
    print(json.dumps({{
        "contract": CONTRACT,
        "request_id": request["request_id"],
        "ok": True,
        "result": result,
    }}, separators=(",", ":")), flush=True)

def describe(request):
    response(request, {{
        "provider_id": "fixture-opencode-provider",
        "display_name": "Fixture OpenCode External Provider",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {{
            "launch": True,
            "policy": True,
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
    }})

def policy(request):
    case_name = os.environ["FIXTURE_CASE"]
    (OBSERVATIONS / f"{{case_name}}.policy.json").write_text(
        json.dumps(request, sort_keys=True), encoding="utf-8"
    )
    response(request, {{
        "accepted": True,
        "stdin": None,
        "prompt": None,
        "diagnostics": [],
        "markers": [],
    }})

def launch(request):
    case_name = os.environ["FIXTURE_CASE"]
    params = request["params"]
    prompt = params["model"]["inputs"]["prompt"]
    argv = params["argv"]
    stdin = params.get("stdin")
    prompt_argv = [arg for arg in argv if arg == prompt]
    stdin_bytes = b""
    if stdin is not None:
        if stdin.get("encoding") != "utf8":
            raise ValueError("fixture requires utf8 stdin")
        stdin_bytes = stdin["data"].encode("utf-8")
    if stdin_bytes and prompt_argv:
        transport = "both"
        carrier = stdin_bytes
    elif stdin_bytes:
        transport = "stdin"
        carrier = stdin_bytes
    elif len(prompt_argv) == 1:
        transport = "argv"
        carrier = prompt_argv[0].encode("utf-8")
    else:
        transport = "missing"
        carrier = b""

    (OBSERVATIONS / f"{{case_name}}.launch.json").write_text(
        json.dumps(request, sort_keys=True), encoding="utf-8"
    )
    (OBSERVATIONS / f"{{case_name}}.carrier").write_bytes(carrier)
    (OBSERVATIONS / f"{{case_name}}.carrier.json").write_text(json.dumps({{
        "transport": transport,
        "logical_prompt_bytes": len(prompt.encode("utf-8")),
        "prompt_argv_occurrences": len(prompt_argv),
        "prompt_argv_bytes": sum(len(arg.encode("utf-8")) for arg in prompt_argv),
        "stdin_bytes": len(stdin_bytes),
    }}, sort_keys=True), encoding="utf-8")

    request_id = request["request_id"]
    session_id = f"fixture-session-{{case_name}}"
    events = [
        {{
            "contract": CONTRACT,
            "request_id": request_id,
            "seq": 1,
            "time_unix_ms": 1001,
            "kind": "stdout",
            "data_base64": base64.b64encode(b"FIXTURE_OK\n").decode("ascii"),
        }},
        {{
            "contract": CONTRACT,
            "request_id": request_id,
            "seq": 2,
            "time_unix_ms": 1002,
            "kind": "marker",
            "name": "oulipoly.provider_session",
            "value": {{"provider_session_id": session_id}},
        }},
        {{
            "contract": CONTRACT,
            "request_id": request_id,
            "seq": 3,
            "time_unix_ms": 1003,
            "kind": "exit",
            "status": {{"kind": "exited", "code": 0}},
            "terminal_signal": {{
                "kind": "clean_exit",
                "evidence": "fixture external provider completed",
                "observed_at_unix_ms": 1003,
            }},
            "session": {{"provider_session_id": session_id}},
        }},
    ]
    for event in events:
        print(json.dumps(event, separators=(",", ":")), flush=True)

def main():
    request = read_request()
    subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
    if subcommand == "describe":
        describe(request)
        return 0
    if subcommand == "policy.evaluate":
        policy(request)
        return 0
    if subcommand == "launch":
        launch(request)
        return 0
    return 64

if __name__ == "__main__":
    raise SystemExit(main())
"#
            ),
        )
        .expect("external provider script");
        let mut permissions = fs::metadata(&path).expect("script metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("executable external provider");
        path
    }

    fn run_defined_case(
        &self,
        name: &'static str,
        instructions: &str,
        raw_prompt: &str,
    ) -> CaseResult {
        let agent_file = self._dir.path().join(format!("{name}.md"));
        fs::write(
            &agent_file,
            format!("---\nmodel: fixture\n---\n{instructions}"),
        )
        .expect("agent file");
        let expected = format!("{instructions}\n\n{raw_prompt}");
        self.run_case(name, Some(&agent_file), expected, Some(instructions.len()))
    }

    fn run_ad_hoc_case(&self, name: &'static str, prompt: String) -> CaseResult {
        self.run_case(name, None, prompt, None)
    }

    fn run_case(
        &self,
        name: &'static str,
        agent_file: Option<&Path>,
        expected_prompt: String,
        definition_bytes: Option<usize>,
    ) -> CaseResult {
        let prompt_file = self._dir.path().join(format!("{name}-prompt.md"));
        let raw_prompt = if let Some(agent_file) = agent_file {
            let definition = fs::read_to_string(agent_file).expect("agent file readback");
            let instructions = definition
                .split_once("---\n")
                .and_then(|(_, rest)| rest.split_once("---\n"))
                .map(|(_, instructions)| instructions)
                .expect("fixture agent frontmatter");
            expected_prompt
                .strip_prefix(instructions)
                .and_then(|tail| tail.strip_prefix("\n\n"))
                .expect("defined prompt shape")
                .to_string()
        } else {
            expected_prompt.clone()
        };
        fs::write(&prompt_file, raw_prompt).expect("prompt file");

        let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        command.arg("--model").arg("fixture");
        if let Some(agent_file) = agent_file {
            command.arg("--agent-file").arg(agent_file);
        }
        command
            .arg("--file")
            .arg(&prompt_file)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--project")
            .arg(&self.project_dir)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("HOME", &self.data_home)
            .env("OULIPOLY_DATA_DIR", &self.data_home)
            .env("FIXTURE_CASE", name)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env_remove("OULIPOLY_PARENT_INVOCATION")
            .env_remove("OULIPOLY_RETURN_CHANNEL");
        let output = command.output().expect("run external provider fixture");

        let policy = read_json(&self.observations_dir.join(format!("{name}.policy.json")));
        let launch = read_json(&self.observations_dir.join(format!("{name}.launch.json")));
        let carrier = fs::read(self.observations_dir.join(format!("{name}.carrier")))
            .expect("carrier observation");
        assert_eq!(policy["contract"], "oulipoly.provider/v1", "{name}");
        assert_eq!(launch["contract"], "oulipoly.provider/v1", "{name}");
        assert_eq!(launch["provider_instance_id"], "fixture-opencode", "{name}");
        assert_eq!(
            launch["params"]["settings_id"], "fixture-opencode",
            "{name}"
        );
        assert_eq!(launch["params"]["mode"], "arg", "{name}");
        assert_eq!(launch["params"]["model"]["name"], "fixture", "{name}");
        assert_eq!(
            launch["params"]["working_directory"],
            self.project_dir.to_string_lossy().as_ref(),
            "{name}"
        );
        assert_eq!(
            launch["params"]["env"]["FIXTURE_LAUNCH_CONTEXT"], "fixed-external-provider-context",
            "{name}"
        );
        assert_eq!(
            launch["params"]["model"]["inputs"]["prompt"]
                .as_str()
                .expect("logical prompt")
                .as_bytes(),
            expected_prompt.as_bytes(),
            "{name}: launch request logical prompt must preserve every byte"
        );
        assert_eq!(
            carrier,
            expected_prompt.as_bytes(),
            "{name}: final external-provider prompt carrier must preserve every byte"
        );
        assert_eq!(
            launch_argv(&launch)[..FIXED_PROVIDER_ARGV.len()],
            FIXED_PROVIDER_ARGV,
            "{name}: fixed OpenCode launch argv changed"
        );
        assert!(
            output.status.success(),
            "{name}: external provider process failed: stdout={:?}, stderr={:?}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("FIXTURE_OK"),
            "{name}: streamed provider stdout missing"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("OULIPOLY_SESSION=")
                && stderr.contains(&format!(
                    "\"provider_session_id\":\"fixture-session-{name}\""
                )),
            "{name}: provider session projection missing: {stderr:?}"
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("OULIPOLY_RESULT="),
            "{name}: runner result evidence missing"
        );

        CaseResult {
            name,
            definition_bytes,
            rendered_bytes: expected_prompt.len(),
            launch,
            output,
        }
    }
}

impl CaseResult {
    fn carrier_observation(&self) -> Value {
        let prompt = self.launch["params"]["model"]["inputs"]["prompt"]
            .as_str()
            .expect("logical prompt");
        let prompt_argv = launch_argv(&self.launch)
            .iter()
            .filter(|arg| **arg == prompt)
            .count();
        let stdin = self.launch["params"]["stdin"].as_object();
        let stdin_bytes = stdin
            .and_then(|value| value.get("data"))
            .and_then(Value::as_str)
            .map_or(0, str::len);
        let transport = match (prompt_argv, stdin_bytes) {
            (1, 0) => "argv",
            (0, bytes) if bytes > 0 => "stdin",
            (count, bytes) if count > 0 && bytes > 0 => "both",
            _ => "missing",
        };
        serde_json::json!({
            "transport": transport,
            "prompt_argv_occurrences": prompt_argv,
            "prompt_argv_bytes": prompt_argv * prompt.len(),
            "stdin_bytes": stdin_bytes,
        })
    }

    fn transport_violation(&self) -> Option<String> {
        let observation = self.carrier_observation();
        let expect_stdin = self.rendered_bytes >= PROVIDER_INLINE_ARG_LIMIT;
        let correct = if expect_stdin {
            observation["transport"] == "stdin"
                && observation["prompt_argv_occurrences"] == 0
                && observation["stdin_bytes"] == self.rendered_bytes
        } else {
            observation["transport"] == "argv"
                && observation["prompt_argv_occurrences"] == 1
                && observation["prompt_argv_bytes"] == self.rendered_bytes
                && observation["stdin_bytes"] == 0
        };
        (!correct).then(|| self.summary())
    }

    fn summary(&self) -> String {
        let observation = self.carrier_observation();
        format!(
            "{}: definition_bytes={}, rendered_bytes={}, status={:?}, transport={}, prompt_argv_occurrences={}, prompt_argv_bytes={}, stdin_bytes={}, session_marker={}, result_marker={}",
            self.name,
            self.definition_bytes
                .map_or_else(|| "ad-hoc".to_string(), |bytes| bytes.to_string()),
            self.rendered_bytes,
            self.output.status.code(),
            observation["transport"].as_str().expect("transport"),
            observation["prompt_argv_occurrences"],
            observation["prompt_argv_bytes"],
            observation["stdin_bytes"],
            String::from_utf8_lossy(&self.output.stderr).contains("OULIPOLY_SESSION=")
                && String::from_utf8_lossy(&self.output.stderr).contains(&format!(
                    "\"provider_session_id\":\"fixture-session-{}\"",
                    self.name
                )),
            String::from_utf8_lossy(&self.output.stdout).contains("OULIPOLY_RESULT="),
        )
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("json observation"))
        .expect("valid json observation")
}

fn launch_argv(launch: &Value) -> Vec<&str> {
    launch["params"]["argv"]
        .as_array()
        .expect("launch argv")
        .iter()
        .map(|value| value.as_str().expect("string argv"))
        .collect()
}

fn exact_payload(label: &str, pattern: &str, bytes: usize) -> String {
    let prefix = format!("BEGIN {label}\n");
    let suffix = format!("\nEND {label}");
    assert!(prefix.len() + suffix.len() <= bytes);
    let body_bytes = bytes - prefix.len() - suffix.len();
    let repeated = pattern.repeat(body_bytes.div_ceil(pattern.len()));
    format!("{prefix}{}{suffix}", &repeated[..body_bytes])
}

fn definition_for_rendered_prompt(
    label: &str,
    pattern: &str,
    rendered_bytes: usize,
    raw_prompt: &str,
) -> String {
    exact_payload(label, pattern, rendered_bytes - 2 - raw_prompt.len())
}

#[test]
fn large_defined_agent_launch_respects_provider_argv_limit_without_losing_prompt() {
    let raw_prompt = exact_payload("USER PROMPT", "controlled-user-input ", RAW_PROMPT_BYTES);
    let below_boundary = definition_for_rendered_prompt(
        "BELOW BOUNDARY DEFINITION",
        "boundary-control ",
        PROVIDER_INLINE_ARG_LIMIT - 1,
        &raw_prompt,
    );
    let at_boundary = definition_for_rendered_prompt(
        "AT BOUNDARY DEFINITION",
        "boundary-control ",
        PROVIDER_INLINE_ARG_LIMIT,
        &raw_prompt,
    );
    let incident_plain = exact_payload(
        "INCIDENT-SIZED PLAIN DEFINITION",
        "plain-content ",
        INCIDENT_DEFINITION_BYTES,
    );
    let incident_structured = exact_payload(
        "INCIDENT-SIZED STRUCTURED DEFINITION",
        "## Contract\n```yaml\nmode: fixture\nroute: local\n```\n",
        INCIDENT_DEFINITION_BYTES,
    );
    let incident_numeric = exact_payload(
        "INCIDENT-SIZED NUMERIC DEFINITION",
        "rule 1 mode 0 limit 1099 continue\n",
        INCIDENT_DEFINITION_BYTES,
    );
    let small_definition = exact_payload(
        "SMALL DEFINED AGENT",
        "## Rule\n- action: inspect\n- constraint: local-only\n",
        512,
    );
    let oversized_ad_hoc = exact_payload(
        "INCIDENT-SIZED AD HOC PROMPT",
        "ordinary-ad-hoc 1 0 1099 ",
        INCIDENT_DEFINITION_BYTES + 2 + RAW_PROMPT_BYTES,
    );

    assert_eq!(incident_plain.len(), INCIDENT_DEFINITION_BYTES);
    assert_eq!(incident_structured.len(), INCIDENT_DEFINITION_BYTES);
    assert_eq!(incident_numeric.len(), INCIDENT_DEFINITION_BYTES);
    assert_ne!(incident_plain, incident_structured);
    assert_ne!(incident_plain, incident_numeric);
    assert!(incident_numeric.contains(" 1 "));
    assert!(incident_numeric.contains(" 0 "));
    assert!(incident_numeric.contains(" 1099 "));

    let fixture = Fixture::new();
    let results = [
        fixture.run_defined_case("below_boundary", &below_boundary, &raw_prompt),
        fixture.run_defined_case("at_boundary", &at_boundary, &raw_prompt),
        fixture.run_defined_case("incident_plain", &incident_plain, &raw_prompt),
        fixture.run_defined_case("incident_structured", &incident_structured, &raw_prompt),
        fixture.run_defined_case("incident_numeric", &incident_numeric, &raw_prompt),
        fixture.run_defined_case("small_defined", &small_definition, &raw_prompt),
        fixture.run_ad_hoc_case("small_ad_hoc", raw_prompt.clone()),
        fixture.run_ad_hoc_case("oversized_ad_hoc", oversized_ad_hoc),
    ];

    assert_eq!(results[0].rendered_bytes, 65_535);
    assert_eq!(results[1].rendered_bytes, 65_536);
    assert_eq!(results[2].rendered_bytes, 69_984);
    assert_eq!(results[3].rendered_bytes, 69_984);
    assert_eq!(results[4].rendered_bytes, 69_984);

    let violations = results
        .iter()
        .filter_map(CaseResult::transport_violation)
        .collect::<Vec<_>>();
    assert!(
        violations.is_empty(),
        "affected external-provider launches must keep rendered prompts below {PROVIDER_INLINE_ARG_LIMIT} bytes in one positional argv value and carry prompts at or above that boundary byte-for-byte in versioned launch stdin with no prompt argv; controlled external-provider results:\n{}",
        results
            .iter()
            .map(CaseResult::summary)
            .collect::<Vec<_>>()
            .join("\n")
    );
}
