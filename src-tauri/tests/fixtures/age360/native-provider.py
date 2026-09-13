import base64
import hashlib
import json
import os
import pathlib
import sys
import subprocess
import sqlite3
import time

CONTRACT = "oulipoly.provider/v1"
SESSION = "ses_age360_native_wake"

def envelope(request, result):
    return {"contract": CONTRACT, "request_id": request["request_id"], "ok": True, "result": result}

def event(request, seq, kind, **fields):
    value = {"contract": CONTRACT, "request_id": request["request_id"], "seq": seq, "time_unix_ms": 1000 + seq, "kind": kind}
    value.update(fields)
    print(json.dumps(value, separators=(",", ":")), flush=True)

def output_completion(request, seq, stdout):
    event(request, seq, "marker", name="oulipoly.launch_output_complete/v1", value={
        "protocol": "oulipoly.launch_output/v1",
        "stdout": {"bytes": len(stdout), "sha256": hashlib.sha256(stdout).hexdigest()},
        "stderr": {"bytes": 0, "sha256": hashlib.sha256(b"").hexdigest()},
        "data_event_count": 1,
    })

def launch(request):
    params = request.get("params", {})
    # The provider launch contract carries workload environment in params.env,
    # not the adapter process environment. Apply the real runner-supplied values.
    os.environ.update(params.get("env") or {})
    known = params.get("session", {}).get("known_provider_session_id")
    prompt = params.get("model", {}).get("inputs", {}).get("prompt", "")
    case = os.environ.get("AGE360_CASE")
    seq = 1
    if known:
        marker = pathlib.Path(os.environ["AGE360_NATIVE_WAKE_MARKER"])
        with marker.open("a") as stream:
            stream.write(json.dumps(prompt, separators=(",", ":")) + "\n")
        gate = pathlib.Path(os.environ["AGE360_NATIVE_WAKE_GATE"])
        deadline = time.monotonic() + 20
        while not gate.exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        stdout = b"native resumed\n"
        event(request, seq, "stdout", data_base64=base64.b64encode(stdout).decode("ascii"))
        seq += 1
        acceptance = params.get("prompt_acceptance", {})
        acceptance_marker = {
            "protocol": "oulipoly.prompt_acceptance/v1",
            "provider_session_id": known,
            "prompt_sha256": hashlib.sha256(prompt.encode("utf-8")).hexdigest(),
            "source": "age360.native.fixture",
            "message_id": "native-accepted",
        }
        if acceptance.get("delivery_nonce"):
            acceptance_marker["delivery_nonce"] = acceptance["delivery_nonce"]
        event(request, seq, "marker", name="oulipoly.prompt_accepted/v1", value=acceptance_marker)
        seq += 1
        event(request, seq, "marker", name="oulipoly.produced_assistant_response", value=True)
        seq += 1
        if os.environ.get("AGE360_DESCENDANT") == "1" or os.environ.get("AGE360_CASE") != "owner_only":
            # The actual recipient reads and ACKs an exact existing mailbox row.
            # This is not a producer byte receipt or a parent-test SQL ACK.
            runner = os.environ["AGENT_BASH_AGENT_RUNNER_BIN"]
            listed = subprocess.run([runner,"mailbox","list","--session-id",known,"--all","--json"],capture_output=True,check=True,timeout=10)
            rows = json.loads(listed.stdout)["rows"]
            assert len(rows) == 1
            received_output = None
            if os.environ.get("AGE360_CASE") == "owner_only":
                assert "native-custody-input" in prompt
            else:
                assert rows[0]["handle"] in prompt
                if case not in ("large_output", "hash_cancel"):
                    payload = json.loads(rows[0]["payload_json"])
                    raw = payload["snapshot"]["output"].encode("utf-8")
                    if case in ("publication_error", "publication_io_error"):
                        live = pathlib.Path(rows[0]["log_path"]).read_bytes()
                        assert len(live) >= len(raw) + 1024 * 1024
                        assert live[:len(raw)] == raw
                        pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("recipient-live-diagnostic-receipt.json").write_text(json.dumps({"byte_len": len(live), "sha256": hashlib.sha256(live).hexdigest(), "original_byte_len": len(raw)}))
                    assert raw == (b"" if case == "registration_reply_loss" else b"paired-source-output")
                    received_output = {"byte_len": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}
                else:
                    payload = json.loads(rows[0]["payload_json"])
                    artifact = payload["output_artifact"]
                    digest = hashlib.sha256()
                    size = 0
                    with open(artifact["path"], "rb") as body:
                        while block := body.read(65536):
                            if case == "large_output":
                                assert block == b"\0" * len(block)
                            digest.update(block)
                            size += len(block)
                    expected_size = 16 * 1024 * 1024 + (6 if case == "hash_cancel" else 0)
                    assert size == artifact["byte_len"] == expected_size
                    if case == "hash_cancel":
                        assert pathlib.Path(artifact["path"]).read_bytes() == b"\0" * (16 * 1024 * 1024) + b"READY\n"
                    assert digest.hexdigest() == artifact["sha256"]
                    received_output = {"byte_len": size, "sha256": digest.hexdigest()}
                if rows[0].get("payload_file_path"):
                    payload = pathlib.Path(rows[0]["payload_file_path"]).read_bytes()
                    assert hashlib.sha256(payload).hexdigest() == rows[0]["payload_sha256"]
            pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("recipient-byte-receipt.json").write_text(json.dumps({"seq": rows[0]["seq"], "prompt_sha256": hashlib.sha256(prompt.encode()).hexdigest(), "output_checked": case != "owner_only", "artifact": case in ("large_output", "hash_cancel"), "output": received_output}))
            sequence = str(rows[0]["seq"])
            ack = subprocess.run([runner,"mailbox","ack","--session-id",known,"--from-seq",sequence,"--to-seq",sequence,"--json"],capture_output=True,check=True,timeout=10)
            pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("recipient-exact-ack.json").write_bytes(ack.stdout)
        if os.environ.get("AGE360_DESCENDANT") == "1":
            cancelling = pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("cancel-probe-enabled").exists()
            if cancelling:
                import signal
                signal.signal(signal.SIGTERM, signal.SIG_IGN)
                os.environ["AGE360_CANCEL_TEST"] = "1"
            child = subprocess.Popen(["/bin/sh", "-c", 'if [ "$AGE360_CANCEL_TEST" = 1 ]; then trap "" TERM; touch "$AGE360_ROOT/cancel-descendant-ready"; fi; while [ ! -f "$AGE360_DESCENDANT_GATE" ]; do sleep 0.02; done'], start_new_session=True, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("descendant.pid").write_text(str(child.pid))
            if cancelling:
                deadline=time.monotonic()+45
                while time.monotonic()<deadline: time.sleep(0.02)
                raise RuntimeError("accepted native cancellation failed to stop held provider")
    else:
        known = SESSION
        event(request, seq, "marker", name="oulipoly.provider_session", value={"provider_session_id": known})
        seq += 1
        if os.environ.get("AGE360_CASE") != "owner_only":
            root = pathlib.Path(os.environ["AGE360_ROOT"])
            parent = json.loads(os.environ["OULIPOLY_PARENT_INVOCATION"])["id"]
            # Native stream ingestion binds the sidecar runtime first. Registration
            # itself binds State through verified live ancestry; waiting for that
            # State effect before registration would deadlock this fixture.
            deadline = time.monotonic() + 20
            while True:
                with sqlite3.connect("file:" + os.environ["OULIPOLY_DATA_DIR"] + "/pid-identity.db?mode=ro", uri=True) as db:
                    row = db.execute("SELECT session_id FROM runtime_generation WHERE spawn_invocation_uuid=?", (parent,)).fetchone()
                if row and row[0] == SESSION: break
                if time.monotonic() > deadline: raise RuntimeError("native session binding was not ingested")
                time.sleep(0.02)
            mode = "sync" if os.environ["AGE360_CASE"] == "sync" else "async"
            env = dict(os.environ, AGENT_BASH_OWNER_SESSION_ID=SESSION, AGENT_BASH_OWNER_INVOCATION_UUID=parent)
            workload = "printf paired-source-output"
            if mode == "async": workload = 'while [ ! -f "$AGE360_WORKLOAD_GATE" ]; do sleep 0.02; done; printf paired-source-output'
            extra = []
            if os.environ["AGE360_CASE"] == "early_exit":
                workload = 'printf paired-source-output; exit 37'
                extra = ["--ready-sentinel", "NEVER-SEEN"]
            elif os.environ["AGE360_CASE"] == "large_output":
                workload = 'head -c 16777216 /dev/zero'
            scope = "tree"
            if case == "publication_race":
                scope = "root"
                workload = 'sleep 600 >/dev/null 2>&1 & printf paired-source-output'
            elif case in ("publication_error", "publication_io_error"):
                extra = ["--ready-sentinel", "paired-source-output"]
                workload = 'printf paired-source-output; while [ ! -f "$AGE360_ROOT/write-more" ]; do sleep .02; done; head -c 1048576 /dev/zero; touch "$AGE360_ROOT/writer-done"; exec sleep 600'
            elif case == "hash_cancel":
                env["AGENT_BASH_LOG_MAX_BYTES"] = str(32 * 1024 * 1024)
                extra = ["--ready-sentinel", "READY"]
                workload = 'head -c 16777216 /dev/zero; printf "READY\\n"; exec sleep 600'
            workload = 'printf "launch\\n" >> "$AGE360_ROOT/source-launches"; ' + workload
            result = subprocess.run([os.environ["AGE360_AGENT_BASH_BIN"], "run", "--delivery", mode, "--completion-scope", scope, *extra, "--", "/bin/sh", "-c", workload], env=env, capture_output=True, timeout=30)
            (root / "bash-dispatch.stdout").write_bytes(result.stdout)
            (root / "bash-dispatch.stderr").write_bytes(result.stderr)
            allowed = (0, 37) if os.environ["AGE360_CASE"] == "early_exit" else (0,)
            if result.returncode not in allowed: raise RuntimeError("actual paired Bash dispatch failed: " + result.stderr.decode(errors="replace"))
            (root / "provider-dispatched").touch()
            if case in ("publication_race", "publication_error", "publication_io_error", "hash_cancel"):
                # Actual original owner issues cancellation through the exact Bash
                # CLI; the namespace controller cannot impersonate its ancestry.
                deadline = time.monotonic() + 45
                while not (root / "cancel-source").exists():
                    if time.monotonic() > deadline: raise RuntimeError("test did not request owner cancellation")
                    time.sleep(.02)
                handle = json.loads(result.stdout)["handle"]
                cancelled = subprocess.run([os.environ["AGE360_AGENT_BASH_BIN"], "cancel", handle], env=env, capture_output=True, timeout=30)
                (root / "source-cancel-result.json").write_text(json.dumps({"rc": cancelled.returncode, "stdout": cancelled.stdout.decode(), "stderr": cancelled.stderr.decode(), "owner_pid": os.getpid()}))
            if mode == "sync":
                deadline = time.monotonic() + 30
                while not (root / "release-initial-provider").exists():
                    if time.monotonic() > deadline: raise RuntimeError("test did not release initial provider")
                    time.sleep(0.02)
        stdout = b"native initial\n"
        event(request, seq, "stdout", data_base64=base64.b64encode(stdout).decode("ascii"))
        seq += 1
    if not params.get("session", {}).get("known_provider_session_id") and pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("nested-probe-enabled").exists():
        environment = dict(os.environ)
        environment.pop("OULIPOLY_COMPLETION_ENDPOINT", None)
        nested = subprocess.run([os.environ["AGENT_BASH_AGENT_RUNNER_BIN"], "-m", "absent-nested-model", "must reject ancestry"], env=environment, capture_output=True, timeout=10)
        pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("nested-rejection.json").write_text(json.dumps({"rc": nested.returncode,"stderr":nested.stderr.decode(errors="replace")}))
    if not params.get("session", {}).get("known_provider_session_id"):
        pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("provider-initial-ready").touch()
    if not params.get("session", {}).get("known_provider_session_id") and os.environ.get("AGE360_HOLD_INITIAL") == "1":
        gate = pathlib.Path(os.environ["AGE360_ROOT"]) / "release-initial-provider"
        deadline = time.monotonic() + 45
        while not gate.exists():
            if time.monotonic() > deadline: raise RuntimeError("fixture initial hold expired")
            time.sleep(0.02)
    output_completion(request, seq, stdout)
    seq += 1
    event(request, seq, "exit", status={"kind": "exited", "code": 0}, terminal_signal={"kind": "clean_exit", "evidence": "native fixture clean exit", "observed_at_unix_ms": 1000 + seq}, session={"provider_session_id": known, "state": {"cursor": "native"}})

def session_turn_page(request):
    params = request.get("params", {})
    marker = pathlib.Path(os.environ["AGE360_NATIVE_WAKE_MARKER"])
    prompts = []
    if marker.exists():
        prompts = [json.loads(line) for line in marker.read_text().splitlines() if line]
    projection = params.get("turn_projection")
    if params.get("start_mode") == "tail":
        selected = []
    elif projection == "user_observation":
        token = params.get("after_token") or "age360-anchor:0"
        selected = prompts[int(token.rsplit(":", 1)[1]):]
    else:
        selected = []
    turns = []
    for offset, prompt in enumerate(selected[:params.get("max_turns", 1)]):
        normalized = prompt.replace("\r\n", "\n").replace("\r", "\n").strip()
        turns.append({
            "session_id": SESSION,
            "turn_id": "age360-observed-user-" + str(offset + 1),
            "snapshot_sequence": offset,
            "timestamp": "2026-08-30T12:00:00Z",
            "role": "user",
            "parent_turn_id": None,
            "is_sidechain": False,
            "is_compaction_boundary": False,
            "body_state": "omitted_oversize",
            "body": None,
            "body_bytes": len(normalized.encode("utf-8")),
            "body_sha256": None,
            "canonical_text_sha256": hashlib.sha256(normalized.encode("utf-8")).hexdigest(),
        })
    count = len(prompts)
    return envelope(request, {
        "read_protocol": "oulipoly.session_turn_pages/v1",
        "provider_instance_id": request.get("provider_instance_id"),
        "settings_id": params.get("settings_id"),
        "session_id": SESSION,
        "turn_projection": projection,
        "snapshot_id": "age360-observation:" + str(count),
        "page_index": 0,
        "page_start_sequence": 0,
        "turns": turns,
        "page_turn_count": len(turns),
        "source_bytes_examined": sum(len(json.dumps(turn)) for turn in turns),
        "scan_progress": False,
        "snapshot_complete": True,
        "next_page_token": None,
        "resume_token": "age360-anchor:" + str(count),
        "source_final": False,
        "warnings": [],
    })

request = json.loads(sys.stdin.read() or "{}")
method = sys.argv[1] if len(sys.argv) > 1 else ""
if method == "describe":
    print(json.dumps(envelope(request, {
        "provider_id": "age360-native-wake-fixture",
        "display_name": "AGE-309 Native Wake Fixture",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {"launch": True, "launch_output_v1": True, "policy": True, "quota": False, "session": True, "session_turn_pages_v1": True, "terminal": False, "rotation": False, "discovery": False, "settings": False, "setup_brain": False, "setup": False, "migration": False, "prompt_acceptance_v1": True},
    })))
elif method == "policy.evaluate":
    print(json.dumps(envelope(request, {"accepted": True, "env": {}, "stdin": None, "prompt": None, "diagnostics": [], "markers": []})))
elif method == "launch":
    try:
        launch(request)
    except BaseException:
        import traceback
        pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("provider-error.txt").write_text(traceback.format_exc())
        raise
elif method == "session.capture":
    print(json.dumps(envelope(request, {"provider_session_id": SESSION, "state": {"captured": True}, "artifacts": []})))
elif method == "session.read_turns":
    print(json.dumps(session_turn_page(request)))
else:
    print(json.dumps({"contract": CONTRACT, "request_id": request.get("request_id", "missing"), "ok": False, "error": {"category": "failed", "code": "unsupported_subcommand", "message": method, "retryable": False}}))
