#![cfg(target_os = "linux")]

mod age153_support;

use age153_support::Age153Fixture;
use rusqlite::Connection;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

const MODEL: &str = "classification-custody";
const PROVIDER: &str = "opencode";
const REASON: &str = "external_provider_terminal_classification_failed";

fn fixture() -> Age153Fixture {
    let fixture = Age153Fixture::new();
    fixture.write_model(MODEL, &[PROVIDER]);
    fixture.write_providers_with_bodies(&[(PROVIDER, "printf 'legitimate output\\n'\nexit 0")]);
    fixture
}

// Copy the generated fake endpoint into this fixture before instrumentation.
// Never edit the shared endpoint cache or execute an installed provider.
fn instrument(fixture: &Age153Fixture, hook: &str, advertised: bool) -> PathBuf {
    let config_path = fixture.app_config_dir.join("providers.toml");
    let mut config: toml::Table = fs::read_to_string(&config_path).unwrap().parse().unwrap();
    let implementation = config[PROVIDER]["implementation"].as_table_mut().unwrap();
    let source = fs::read_to_string(implementation["executable"].as_str().unwrap()).unwrap();
    let marker = fixture.dir.path().join("classifier-executed");
    let injection = format!(
        "def terminal_classify():\n    pathlib.Path({}).write_text('executed')\n{}\n",
        serde_json::to_string(&marker).unwrap(),
        hook.lines()
            .map(|line| format!("    {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    assert!(source.contains("def terminal_classify():\n"));
    let mut source = source.replace("def terminal_classify():\n", &injection);
    if !advertised {
        source = source.replace("\"terminal\": \"t\" in enabled,", "\"terminal\": False,");
    }
    let endpoint = fixture.dir.path().join("instrumented-endpoint.py");
    fs::write(&endpoint, source).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o700)).unwrap();
    implementation.insert("executable".into(), endpoint.display().to_string().into());
    fs::write(config_path, toml::to_string(&config).unwrap()).unwrap();
    marker
}

fn envelope(output: &std::process::Output) -> Value {
    // Successful raw-output runs emit their envelope on stderr; terminal
    // failures emit a structured result on stdout instead of raw launch output.
    let stream = if output.status.success() {
        &output.stderr
    } else {
        &output.stdout
    };
    let text = String::from_utf8_lossy(stream);
    let lines = text
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_RESULT="))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "expected exactly one result: {output:?}");
    serde_json::from_str(lines[0]).unwrap()
}

fn assert_failed(fixture: &Age153Fixture, output: &std::process::Output) {
    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let result = envelope(output);
    assert_eq!(result["success"], false, "{result}");
    assert_eq!(result["terminal_reason"], REASON, "{result}");
    let row: (i64, i64, String) = fixture
        .conn()
        .query_row(
            "SELECT success, exit_code, terminal_reason FROM invocations WHERE provider_name = ?1",
            [PROVIDER],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    eprintln!("failed classification: envelope={result}, row={row:?}");
    assert_eq!(row, (0, -1, REASON.into()));
    assert_settled(fixture, -1);
}

fn assert_settled(fixture: &Age153Fixture, expected_exit: i32) {
    let db = Connection::open(fixture.db_path().with_file_name("pid-identity.db")).unwrap();
    let rows = db
        .prepare(
            "SELECT g.lifecycle_state, g.exit_code, c.proof_path FROM runtime_generation g
         JOIN runtime_generation_custody c USING (generation_uuid) WHERE provider_name = ?1",
        )
        .unwrap()
        .query_map([PROVIDER], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    eprintln!("settled generation: {rows:?}");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "exited");
    assert_eq!(
        rows[0].1, expected_exit,
        "generation retains classified work outcome"
    );
    assert_eq!(fs::read(&rows[0].2).unwrap(), b"Q");
}

#[test]
fn advertised_classifier_executes_before_settlement() {
    let fixture = fixture();
    let marker = instrument(&fixture, "", true);
    let output = fixture.run_one_shot(MODEL);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(fs::read_to_string(marker).unwrap(), "executed");
    assert_settled(&fixture, 0);
}

#[test]
fn unadvertised_classifier_is_optional_but_not_executed() {
    let fixture = fixture();
    let marker = instrument(&fixture, "raise SystemExit(23)", false);
    let output = fixture.run_one_shot(MODEL);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(envelope(&output)["success"], true);
    assert!(!marker.exists());
    assert_settled(&fixture, 0);
}

#[test]
fn advertised_classifier_execution_failure_is_not_exit_zero_success() {
    let fixture = fixture();
    let marker = instrument(&fixture, "raise SystemExit(23)", true);
    let output = fixture.run_one_shot(MODEL);
    assert!(marker.exists(), "classifier must actually run");
    assert_failed(&fixture, &output);
}

#[test]
fn advertised_classifier_malformed_response_is_not_exit_zero_success() {
    let fixture = fixture();
    let marker = instrument(
        &fixture,
        "print('not-json', flush=True)\nraise SystemExit(0)",
        true,
    );
    let output = fixture.run_one_shot(MODEL);
    assert!(marker.exists());
    assert_failed(&fixture, &output);
}

#[test]
fn advertised_classifier_cannot_launder_failure_as_unsupported() {
    let fixture = fixture();
    let marker = instrument(&fixture, "unsupported()", true);
    let output = fixture.run_one_shot(MODEL);
    assert!(marker.exists());
    assert_failed(&fixture, &output);
}

fn descendant_hook(fixture: &Age153Fixture) -> (String, PathBuf) {
    let observation = fixture.dir.path().join("descendant-observation.json");
    let db = fixture.db_path().with_file_name("pid-identity.db");
    let hook = format!(
        r#"import time, sqlite3
classifier_pid = os.getpid()
if os.fork() == 0:
    # Do not keep output pipes open: settlement must depend on custody,
    # not an accidental stdout/stderr EOF wait. The classifier parent exits.
    for fd in (0, 1, 2):
        os.close(fd)
    time.sleep(0.3)
    db = sqlite3.connect({db})
    rows = db.execute("SELECT g.lifecycle_state, c.proof_path FROM runtime_generation g JOIN runtime_generation_custody c USING (generation_uuid) WHERE provider_name = 'opencode'").fetchall()
    try:
        parent_state = pathlib.Path(f"/proc/{{classifier_pid}}/stat").read_text().rsplit(')', 1)[1].split()[0]
    except FileNotFoundError:
        parent_state = "absent"
    observation = {{"rows": rows, "proofs": [pathlib.Path(row[1]).read_text() for row in rows], "pid": os.getpid(), "classifier_state": parent_state}}
    pathlib.Path({observation}).write_text(json.dumps(observation))
    db.close()
    os._exit(0)"#,
        db = serde_json::to_string(&db).unwrap(),
        observation = serde_json::to_string(&observation).unwrap(),
    );
    (hook, observation)
}

fn assert_descendant_observation(path: PathBuf) {
    let observation: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(
        observation["rows"].as_array().unwrap().len(),
        1,
        "{observation}"
    );
    assert_eq!(observation["rows"][0][0], "running", "{observation}");
    assert_eq!(
        observation["proofs"][0], "",
        "no Q while descendant lives: {observation}"
    );
    assert!(
        matches!(
            observation["classifier_state"].as_str(),
            Some("Z" | "absent")
        ),
        "classifier must have exited before descendant observation: {observation}"
    );
    eprintln!("live descendant observation: {observation}");
    let pid = observation["pid"].as_i64().unwrap();
    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "descendant must be reaped before return"
    );
}

#[test]
fn classifier_descendant_is_owned_until_quiescence() {
    let fixture = fixture();
    let (hook, observation) = descendant_hook(&fixture);
    instrument(&fixture, &hook, true);
    let output = fixture.run_one_shot(MODEL);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_descendant_observation(observation);
    assert_settled(&fixture, 0);
}

#[test]
fn failed_classifier_descendant_is_owned_until_quiescence() {
    let fixture = fixture();
    let (mut hook, observation) = descendant_hook(&fixture);
    hook.push_str("\nraise SystemExit(23)");
    instrument(&fixture, &hook, true);
    let output = fixture.run_one_shot(MODEL);
    assert_descendant_observation(observation);
    assert_failed(&fixture, &output);
}

#[test]
fn advertised_classifier_failure_on_resume_reconciles_session_projection() {
    let fixture = Age153Fixture::new();
    fixture.write_resume_pool(
        MODEL,
        &[(
            PROVIDER,
            "printf 'legitimate output\\n'\nexit 0".to_string(),
        )],
    );
    fixture.seed_active_chain(PROVIDER, MODEL);
    let marker = instrument(&fixture, "raise SystemExit(23)", true);
    let output = fixture.run_resume(MODEL);
    assert!(marker.exists());
    assert_failed(&fixture, &output);
    let db = Connection::open(fixture.db_path().with_file_name("pid-identity.db")).unwrap();
    let exit: i32 = db
        .query_row(
            "SELECT last_exit_code FROM session_runtime WHERE session_id = ?1",
            [age153_support::SESSION_ID],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(exit, -1);
}

#[test]
fn classifier_failure_preserves_known_launch_cancellation() {
    let fixture = fixture();
    let marker = instrument(&fixture, "raise SystemExit(23)", true);
    let path = fixture.dir.path().join("instrumented-endpoint.py");
    let source = fs::read_to_string(&path).unwrap();
    let before = "    exit_fields = {\n        \"status\": status,";
    assert!(source.contains(before));
    fs::write(
        path,
        source.replace(
            before,
            "    exit_fields = {\n        \"status\": {\"kind\": \"cancelled\"},",
        ),
    )
    .unwrap();
    let output = fixture.run_one_shot(MODEL);
    assert!(marker.exists());
    let result = envelope(&output);
    assert_eq!(result["success"], false, "{result}");
    assert_eq!(result["exit_code"], 130, "{result}");
    assert_eq!(result["terminal_reason"], "cancelled", "{result}");
    assert_settled(&fixture, 130);
}
