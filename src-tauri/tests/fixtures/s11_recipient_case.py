"""Execute one S11 matrix cell through real ingress and its automatic recipient.

All queries are read-only except the explicitly selected sidecar trigger removal.
Fixture waits bound private experiment containment, not product scheduling.
"""
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import time

import s11_recipient_control as control

SESSION = "ses_s11externalwake"
MODEL = "s11-external-wake-model"
PAYLOAD = "owner consumed detached child result and continued\n"
MANUAL = "continue after original recipient observation"


def log(kind, **values):
    print("S11_RECIPIENT " + json.dumps(dict(kind=kind, **values)), flush=True)


def rows(path, sql, args=()):
    with sqlite3.connect("file:" + str(path) + "?mode=ro", uri=True) as db:
        db.row_factory = sqlite3.Row
        return [dict(row) for row in db.execute(sql, args)]


def one(path, sql, args=()):
    result = rows(path, sql, args)
    assert len(result) == 1, (sql, args, result)
    return result[0]


def wait(label, predicate):
    end = time.monotonic() + 20
    while True:
        value = predicate()
        if value:
            return value
        assert time.monotonic() < end, "fixture containment wait: " + label
        time.sleep(.025)


def command(argv, env=None):
    # Only this direct private command is killed on fixture timeout. Namespace
    # PID1 contains its descendants; no production process or custody is touched.
    try:
        result = subprocess.run(argv, env=env, capture_output=True, timeout=30)
    except subprocess.TimeoutExpired as error:
        log("command_timeout", argv=argv, stdout_hex=(error.stdout or b"").hex(),
            stderr_hex=(error.stderr or b"").hex())
        raise
    log("command", argv=argv, rc=result.returncode,
        stdout_hex=result.stdout.hex(), stderr_hex=result.stderr.hex())
    return result


def envelopes(stdout, stderr):
    return [json.loads(line.removeprefix("OULIPOLY_RESULT="))
            for line in (stdout + "\n" + stderr).splitlines()
            if line.startswith("OULIPOLY_RESULT=")]


def result_of(output):
    result = envelopes(output.stdout.decode(), output.stderr.decode())
    assert len(result) == 1, result
    return result[0]


def events(work):
    return [json.loads(line) for line in (work / "recipient-events.jsonl").read_text().splitlines()]


def attempts(sidecar, seq):
    return rows(sidecar, "SELECT a.* FROM mailbox_delivery_attempts a "
                "JOIN mailbox_delivery_attempt_items i USING(attempt_id) "
                "WHERE i.mailbox_seq=? AND submission_started_at IS NOT NULL", (seq,))


def native_result(sidecar, invocation):
    for activation in rows(sidecar, "SELECT * FROM completion_continuation_attempt "
                           "WHERE session_id=? AND operation='activation'", (SESSION,)):
        path = Path(activation["result_path"])
        if not path.exists():
            continue
        stdout = path.with_name("launcher.stdout").read_text()
        stderr = path.with_name("launcher.stderr").read_text()
        results = [r for r in envelopes(stdout, stderr) if r["id"] == invocation]
        identities = [json.loads(line.removeprefix("OULIPOLY_INVOCATION="))["id"]
                      for line in stderr.splitlines() if line.startswith("OULIPOLY_INVOCATION=")]
        if invocation in identities:
            assert len(results) <= 1, results
            return dict(activation=activation, receipt=json.loads(path.read_text()),
                        stdout=stdout, stderr=stderr, result=results[0] if results else None)
    return None


def assert_marker(selected, marker, request):
    accepted = [e for e in selected if e.get("name") == "oulipoly.prompt_accepted/v1"]
    if marker == "absent":
        assert accepted == [], accepted
        return
    assert len(accepted) == 1, accepted
    value = accepted[0]["value"]
    actual = request["params"]["prompt_acceptance"]
    assert value["protocol"] == actual["protocol"]
    assert value["provider_session_id"] == ("ses_s11-wrong-session" if marker == "session" else SESSION)
    assert value["delivery_nonce"] == ("s11-wrong-delivery-nonce" if marker == "nonce" else actual["delivery_nonce"])
    expected_hash = hashlib.sha256(b"different payload").hexdigest() if marker == "hash" else actual["prompt_sha256"]
    assert value["prompt_sha256"] == expected_hash


# These settled outcome fields, unlike progress/timestamps, must survive receipt
# reconciliation unchanged. Do not compare entire mutable invocation rows.
OUTCOME_FIELDS = ("status", "exit_code", "error_category", "terminal_reason")
SUCCESS = dict(status="succeeded", exit_code=0, error_category=None, terminal_reason=None)


def assert_terminal_fields(actual, expected):
    for key in OUTCOME_FIELDS:
        assert actual[key] == expected[key], (key, actual, expected)


def assert_envelope(result, expected):
    assert_terminal_fields(result, expected)
    assert result["success"] is (expected["status"] == "succeeded"), result


def assert_preserved_outcome(before, after):
    assert after["invocation_uuid"] == before["invocation_uuid"], (before, after)
    assert_terminal_fields(after, before)


def assert_manual_outcome(output, original_invocation):
    assert output.returncode == 0, output
    assert output.stdout.decode() == PAYLOAD, output
    result = result_of(output)
    assert result["id"] and result["id"] != original_invocation, result
    assert_envelope(result, SUCCESS)
    return result


def expected_outcome(marker, exit_shape):
    if exit_shape == "success":
        return SUCCESS
    if exit_shape == "no-assistant":
        reason = category = "resume_completion_unconfirmed"
        code = 0
    elif exit_shape == "missing":
        reason = category = ("resume_prompt_accepted_provider_failed" if marker == "trusted"
                             else "external_provider_missing_final_exit")
        code = -1
    else:
        assert exit_shape == "nonzero", exit_shape
        reason = "resume_prompt_accepted_provider_failed" if marker == "trusted" else "exit_nonzero"
        category = None
        code = 29
    return dict(status="failed", exit_code=code, error_category=category, terminal_reason=reason)


def assert_outcome(native, durable, marker, exit_shape, projection, negative):
    receipt = native["receipt"]
    first_result = native["result"]
    assert first_result is not None, "actual provider drained without a caller result envelope"
    expected = expected_outcome(marker, exit_shape)
    assert first_result["id"] == durable["invocation_uuid"], (first_result, durable)
    assert_envelope(first_result, expected)
    assert_terminal_fields(durable, expected)
    if exit_shape == "missing":
        assert "missing_final_exit;provider_process=exited:0" in native["stderr"], native
        assert first_result["exit_code"] == durable["exit_code"] == -1
    if exit_shape == "success":
        assert native["stdout"] == PAYLOAD, native
        assert first_result["success"] is True and first_result["exit_code"] == 0
        assert durable["status"] == "succeeded" and durable["exit_code"] == 0, durable
    else:
        assert first_result["status"] == durable["status"] == "failed"
        reason = "resume_completion_unconfirmed" if exit_shape == "no-assistant" else (
            "resume_prompt_accepted_provider_failed" if marker == "trusted" else "exit_nonzero")
        if negative and exit_shape == "missing":
            assert first_result["terminal_reason"] == durable["terminal_reason"] == "external_provider_missing_final_exit", first_result
            assert durable["exit_code"] == -1, durable
        else:
            assert first_result["terminal_reason"] == durable["terminal_reason"] == reason, (first_result, durable)
        if exit_shape == "nonzero":
            assert first_result["exit_code"] == durable["exit_code"] == 29
    # The missing protocol exit maps to State/result -1. POSIX preserves its
    # low eight bits in the real launcher wait (255), not generic shell1.
    shell_exit = 1 if projection or exit_shape == "no-assistant" else (
        255 if exit_shape == "missing" else (29 if exit_shape == "nonzero" else 0))
    assert receipt["root_exit_code"] == shell_exit, receipt
    assert receipt["root_wait_status"] == shell_exit * 256, receipt


def run_case(runner, models, provider, marker, exit_shape, projection):
    root = Path(__file__).parent
    work = Path(os.environ["S11_WORK_DIR"])
    sidecar = Path(os.environ["OULIPOLY_DATA_DIR"]) / "pid-identity.db"
    state = sidecar.with_name("state.db")
    negative = marker != "trusted" and exit_shape != "success"
    env = {}
    if marker != "absent":
        env["S11_EMIT_PROMPT_ACCEPTANCE_MARKER"] = "1"
    mismatch = {"hash": "S11_MARKER_PROMPT_SHA_MISMATCH", "session": "S11_MARKER_SESSION_MISMATCH",
                "nonce": "S11_MARKER_DELIVERY_NONCE_MISMATCH"}.get(marker)
    if mismatch:
        env[mismatch] = "1"
    env["S11_EMIT_AFFIRMATIVE_ASSISTANT_RESULT" if exit_shape == "success" else "S11_NO_ASSISTANT_RESULT"] = "1"
    if exit_shape in ("nonzero", "missing"):
        env["S11_EXIT_NONZERO" if exit_shape == "nonzero" else "S11_OMIT_EXIT_EVENT"] = "1"
    config = dict(work_dir=str(work), sidecar=str(sidecar), runner=runner,
                  agent_bash=os.environ["AGE360_AGENT_BASH_BIN"], env=env,
                  negative=negative, projection=projection)
    control.CONTROL.write_text(json.dumps(config))
    log("cell_start", provider=provider, marker=marker, exit_shape=exit_shape, config=config)
    seed = command([runner, "-m", MODEL, "--models-dir", models, "owner admits actual detached completion"])
    assert seed.returncode == 0
    owner = result_of(seed)["id"]
    registration = json.loads((work / "ingress-registration.json").read_text())
    ingress = json.loads((work / "ingress-result.json").read_text())
    assert ingress["owner"] == registration["owner_invocation_uuid"] == owner
    assert registration["owner_session_id"] == SESSION and ingress["rc"] == 0
    handle = registration["handle"]
    notification = wait("real source mailbox row", lambda: rows(sidecar, "SELECT * FROM mailbox WHERE handle=?", (handle,)))
    assert len(notification) == 1, notification
    notification = notification[0]
    seq = notification["seq"]
    assert notification["owner_invocation_uuid"] == owner
    submitted = wait("automatic submission", lambda: attempts(sidecar, seq))
    assert len(submitted) == 1, submitted
    attempt = submitted[0]
    invocation = attempt["delivery_invocation_uuid"]
    native = wait("exact automatic result and native drain", lambda: native_result(sidecar, invocation))
    durable = one(state, "SELECT * FROM invocations WHERE invocation_uuid=?", (invocation,))
    receipt = native["receipt"]
    activation = native["activation"]
    assert receipt["attempt_id"] == activation["attempt_id"]
    assert receipt["custodian"] == json.loads(activation["custodian_identity"])
    assert receipt["owned_children"] == "ECHILD" and receipt["spawn_failed"] is False
    assert activation["domain_id"] == registration["domain_id"]
    assert activation["source_registration_id"] == registration["registration_id"]
    assert activation["source_listener_revision"] == registration["listener_revision"]
    generation = one(sidecar, "SELECT * FROM runtime_generation WHERE spawn_invocation_uuid=?", (invocation,))
    assert generation["lifecycle_state"] == "exited"
    all_events = events(work)
    launches = [e for e in all_events if e["kind"] == "launch" and e["nonce"]]
    assert len(launches) == 1, launches
    launch = launches[0]
    request = launch["request"]
    assert json.loads(request["params"]["env"]["OULIPOLY_PARENT_INVOCATION"])["id"] == invocation
    assert launch["nonce"] == attempt["attempt_id"] and launch["nonce"]
    assert launch["selected"] == env
    prompt = request["params"]["model"]["inputs"]["prompt"]
    assert handle in prompt and MANUAL not in prompt
    assert request["params"]["prompt_acceptance"]["prompt_sha256"] == hashlib.sha256(prompt.encode()).hexdigest()
    selected = [e["event"] for e in all_events if e["kind"] == "emitted" and e["request_id"] == request["request_id"]]
    first_row = one(sidecar, "SELECT * FROM mailbox WHERE seq=?", (seq,))
    first_attempt = attempts(sidecar, seq)
    acknowledgements = rows(state, "SELECT * FROM session_delivery_acknowledgements WHERE turn_generation_id=?", (invocation,))
    log("first_observation", registration=registration, ingress=ingress, notification=first_row,
        attempts=first_attempt, native=native, durable=durable, generation=generation,
        acknowledgements=acknowledgements, launch=launch, events=all_events)
    assert_marker(selected, marker, request)
    exits = [e for e in selected if e["kind"] == "exit"]
    if exit_shape == "missing":
        assert exits == [], exits

    else:
        assert len(exits) == 1, exits
        assert exits[0]["status"] == dict(kind="exited", code=29 if exit_shape == "nonzero" else 0)
    assistant = [e for e in selected if e.get("name") == "oulipoly.produced_assistant_response"]
    assert bool(assistant) == (exit_shape == "success")
    assert durable["resume_acceptance_status"] is None and durable["resume_acceptance_evidence"] is None
    assert durable["provider_name"] == provider and durable["provider_session_id"] == SESSION
    # A drained post-provider error may lack its promised result. Keep this
    # contract violation red, but observe independent recovery before returning
    # it to the root. Never synthesize the missing envelope or accept spawn_error
    # as a substitute for the selected nonzero/missing-exit terminal path.
    violations = []
    try:
        assert_outcome(native, durable, marker, exit_shape, projection, negative)
    except AssertionError as error:
        violations.append("recipient outcome: " + str(error))
        log("outcome_contract_violation", message=str(error), native=native, durable=durable)
    if negative:
        assert first_row["delivered_at"] is None, first_row
        assert first_attempt[0]["observation_confirmed_at"] is None
        assert first_attempt[0]["acknowledged_at"] is None and not acknowledgements
        if first_row["delivery_attempts"] != 1:
            violations.append("first negative delivery_attempts expected 1, observed " + str(first_row["delivery_attempts"]))
        pages = [e for e in all_events if e["kind"] == "page"]
        assert pages and all(e["hidden"] and not e["result"]["turns"] for e in pages)
        for page in pages:
            assert page["result"]["resume_token"] == (page["request"]["params"].get("after_token") or "s11-anchor:0")
        # Only after actual first negative/drain capture expose the original log.
        control.record("first_negative_captured", invocation=invocation)
        (work / "expose-observation").touch()
    elif projection:
        assert launch["projection_fault"] is True and control.projection_present(config)
        installed = [e for e in all_events if e["kind"] == "projection_installed"]
        assert len(installed) == 1 and installed[0]["monotonic_ns"] < launch["monotonic_ns"]
        assert "forced recipient mailbox projection failure" in native["stderr"], native
        assert first_row["delivered_at"] is None, first_row
        assert len(acknowledgements) == 1 and acknowledgements[0]["confirmed_at"], acknowledgements
        assert acknowledgements[0]["session_id"] == SESSION
        assert acknowledgements[0]["delivery_id"] == attempt["attempt_id"]
        assert acknowledgements[0]["submitted_evidence"] == request["params"]["prompt_acceptance"]["prompt_sha256"]
        with sqlite3.connect(sidecar) as db:
            db.execute("DROP TRIGGER s11_recipient_projection_fault")
        control.record("projection_removed", invocation=invocation)
    else:
        assert first_row["delivered_at"] and first_row["delivered_by_invocation_uuid"] == invocation
        assert first_row["delivery_attempts"] == 1, first_row
    if exit_shape == "success" and not projection:
        trace = command([runner, "trace", invocation, "--json"])
        assert trace.returncode == 0
        session = json.loads(trace.stdout)["root"]["session"]
        assert session["transcript_state"] == "no_locator"
        assert session["turn_count"] == session["assistant_turn_count"] == 0
        assert not (root / "xdg-config/oulipoly-agent-runner/sessions.toml").exists()
    manual_env = dict(os.environ, S11_EMIT_AFFIRMATIVE_ASSISTANT_RESULT="1")
    manual = command([runner, "resume", "--models-dir", models, "--model", MODEL,
                      "--session-id", SESSION, "--prompt", MANUAL], manual_env)
    manual_result = assert_manual_outcome(manual, invocation)
    manual_id = manual_result["id"]
    recovered_durable = one(state, "SELECT * FROM invocations WHERE invocation_uuid=?", (invocation,))
    log("recovered_original_outcome", before=durable, after=recovered_durable, manual_result=manual_result)
    try:
        assert_preserved_outcome(durable, recovered_durable)
    except AssertionError as error:
        violations.append("recovery rewrote original outcome: " + str(error))
    final_row = one(sidecar, "SELECT * FROM mailbox WHERE seq=?", (seq,))
    final_attempts = attempts(sidecar, seq)
    all_events = events(work)
    prompts = [json.loads(line) for line in (work / "resume-prompts.jsonl").read_text().splitlines()]
    log("recovery", notification=final_row, attempts=final_attempts, prompts=prompts, events=all_events, manual_id=manual_id)
    assert final_row["delivered_at"] and final_row["delivered_by_invocation_uuid"] == invocation, final_row
    assert len(final_attempts) == 1 and final_attempts[0]["attempt_id"] == attempt["attempt_id"]
    expected_count = 2 if negative else 1
    if final_row["delivery_attempts"] != expected_count:
        violations.append(f"recovered delivery_attempts expected {expected_count}, observed {final_row['delivery_attempts']}")
    assert len(prompts) == 2 and prompts == [prompt, MANUAL], prompts
    manual_launches = [e for e in all_events if e["kind"] == "launch" and json.loads(
        e["request"]["params"]["env"]["OULIPOLY_PARENT_INVOCATION"])["id"] == manual_id]
    assert len(manual_launches) == 1 and not manual_launches[0]["nonce"]
    assert sum(bool(e["nonce"]) for e in all_events if e["kind"] == "launch") == 1
    assert (work / "source-launches").read_text() == "launch\n"
    assert Path(notification["log_path"]).read_bytes() == b"s11-admitted-source-output"
    assert Path(notification["rc_path"]).read_text().strip() == "0"
    if negative:
        assert final_attempts[0]["observation_confirmed_turn_id"] == "s11-observed-user-1"
        assert final_attempts[0]["observation_confirmed_at"]
    source_root = Path(registration["handle_dir"])
    source_bytes = (source_root / "source-outcome-v2.json").read_bytes()
    outcome = json.loads(source_bytes)
    snapshot = json.loads((source_root / "completion-snapshot-v2.json").read_text())
    for key in ("handle", "domain_id", "registration_id", "source_id"):
        assert outcome[key] == snapshot[key] == registration[key]
    assert outcome["kind"] == "exit_tree" and outcome["root_wait_status"] == 0
    assert outcome["original_tree_drained"] is True and outcome["output_closed"] is True
    assert snapshot["outcome_sha256"] == hashlib.sha256(source_bytes).hexdigest()
    assert snapshot["outcome_byte_len"] == len(source_bytes)
    assert snapshot["registration_digest"] == outcome["registration_digest"]
    assert snapshot["output"] == "s11-admitted-source-output" and snapshot["rc"] == 0
    assert registration["helper"]["sha256"] == hashlib.sha256(Path(runner).read_bytes()).hexdigest()
    assert registration["recovery"]["sha256"] == hashlib.sha256(Path(config["agent_bash"]).read_bytes()).hexdigest()
    # Preserve actual source-owned snapshot/outcome bytes alongside all joins.
    for path in sorted((work / "spool").rglob("*.json")):
        log("source_artifact", path=str(path), bytes_hex=path.read_bytes().hex())
    log("cell_observed", provider=provider, marker=marker, exit_shape=exit_shape, violations=violations)
    assert not violations, violations
    log("cell_completed", provider=provider, marker=marker, exit_shape=exit_shape)


if __name__ == "__main__":
    try:
        run_case(*sys.argv[1:6], sys.argv[6] == "1")
    finally:
        # Raw evidence on failure as well as success; no read beyond this fixture.
        from s11_diagnostics import capture
        capture(Path(__file__).parent)
