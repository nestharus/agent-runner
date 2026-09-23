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
    global SESSION
    params = request.get("params", {})
    # The provider launch contract carries workload environment in params.env,
    # not the adapter process environment. Apply the real runner-supplied values.
    os.environ.update(params.get("env") or {})
    known = params.get("session", {}).get("known_provider_session_id")
    prompt = params.get("model", {}).get("inputs", {}).get("prompt", "")
    if os.environ.get("AGE360_MCP_E2E") and prompt == "synthetic independent owner founder" and not known:
        # Founding context only; these flags never enter the guardian environment
        # and therefore cannot contaminate another session's resumed recipient.
        SESSION = "ses_age360_founder"
        os.environ.update(AGE360_CASE="owner_only", AGE360_HOLD_INITIAL="1", AGE360_FOUNDER="1")
    case = os.environ.get("AGE360_CASE")
    if prompt == "synthetic independent listener":
        case = "listener_only"
    seq = 1
    if known and case != "listener_only":
        marker = pathlib.Path(os.environ["AGE360_NATIVE_WAKE_MARKER"])
        with marker.open("a") as stream:
            stream.write(json.dumps(prompt, separators=(",", ":")) + "\n")
        gate = pathlib.Path(os.environ["AGE360_NATIVE_WAKE_GATE"])
        deadline = time.monotonic() + 20
        while not gate.exists():
            assert time.monotonic() < deadline, "recipient gate deadline; no ACK inferred"
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
        if os.environ.get("AGE365_LEGACY_WAKE") == "1" or os.environ.get("AGE360_DESCENDANT") == "1" or os.environ.get("AGE360_CASE") != "owner_only":
            # The actual recipient reads and ACKs an exact existing mailbox row.
            # This is not a producer byte receipt or a parent-test SQL ACK.
            runner = os.environ["AGENT_BASH_AGENT_RUNNER_BIN"]
            listed = subprocess.run([runner,"mailbox","list","--session-id",known,"--all","--json"],capture_output=True,check=True,timeout=10)
            rows = json.loads(listed.stdout)["rows"]
            if case == "two_source":
                assert len(rows) == 2, rows
                second = rows[1]
                assert second["handle"] in prompt
                second_payload = json.loads(second["payload_json"])
                second_output = second_payload["snapshot"]["output"]
                assert second_output["representation"] == "retained-output-v1"
                assert second_output["encoding"] == "raw"
                second_artifact = second_payload["output_artifact"]
                second_raw = pathlib.Path(second_artifact["path"]).read_bytes()
                assert second_raw == b"paired-source-output"
                assert len(second_raw) == second_output["byte_len"] == second_artifact["byte_len"]
                assert hashlib.sha256(second_raw).hexdigest() == second_output["sha256"] == second_artifact["sha256"]
                pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("recipient-second-byte-receipt.json").write_text(json.dumps({"seq": second["seq"], "handle": second["handle"], "byte_len": len(second_raw), "sha256": hashlib.sha256(second_raw).hexdigest()}))
                rows = rows[:1]
            assert len(rows) == 1
            received_output = None
            received_artifact = False
            if os.environ.get("AGE365_LEGACY_WAKE") == "1":
                assert rows[0]["handle"] == "age365-legacy-mailbox"
                assert rows[0]["handle"] in prompt
                assert json.loads(rows[0]["payload_json"])["fixture"] == "legacy-materialized"
            elif os.environ.get("AGE360_CASE") == "owner_only":
                assert "native-custody-input" in prompt
            else:
                assert rows[0]["handle"] in prompt
                if case in ("native_missing", "missing_selection", "missing_pin", "missing_short"):
                    payload = json.loads(rows[0]["payload_json"])
                    missing = payload["snapshot"]["output"]
                    assert missing["representation"] == "missing-original-output-v1"
                    assert payload["snapshot"]["status"] == "original_output_unavailable"
                    assert payload["output_artifact"] is None
                    assert missing["capture_state"] == "irrecoverable"
                    assert missing["original_observer"] == payload["outcome"]["observer"]
                    assert missing["completion_revision"] == payload["outcome"]["completion_revision"]
                    assert missing["outcome_sha256"] == payload["snapshot"]["outcome_sha256"]
                    if case == "native_missing":
                        assert payload["rc"] == 37
                        assert payload["outcome"]["root_wait_status"] == 37 << 8
                        assert payload["outcome"]["original_tree_drained"] is False
                        assert payload["outcome"]["cancellation_id"] is None
                    pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("recipient-missing-output-receipt.json").write_text(json.dumps(payload))
                    received_output = {"missing_original_output": True, "proof": missing}
                elif case not in ("large_output", "hash_cancel"):
                    payload = json.loads(rows[0]["payload_json"])
                    selected = payload["snapshot"]["output"]
                    if isinstance(selected, str):
                        # Older inline snapshots carry a lossy UTF-8 string.
                        assert payload.get("output_artifact") is None
                        raw = selected.encode("utf-8")
                    else:
                        # The v2 source freezes raw bytes; both descriptor
                        # encodings name the retained file's original bytes.
                        artifact = payload["output_artifact"]
                        assert selected["representation"] == "retained-output-v1"
                        assert selected["relative"] == "completion-output-v2.bin"
                        assert selected["encoding"] in ("raw", "utf8-lossy")
                        assert artifact["encoding"] == selected["encoding"]
                        raw = pathlib.Path(artifact["path"]).read_bytes()
                        assert len(raw) == selected["byte_len"] == artifact["byte_len"]
                        digest = hashlib.sha256(raw).hexdigest()
                        assert digest == selected["sha256"] == artifact["sha256"]
                        received_artifact = True
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
                    received_artifact = True
                if rows[0].get("payload_file_path"):
                    payload = pathlib.Path(rows[0]["payload_file_path"]).read_bytes()
                    assert hashlib.sha256(payload).hexdigest() == rows[0]["payload_sha256"]
            pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("recipient-byte-receipt.json").write_text(json.dumps({"seq": rows[0]["seq"], "prompt_sha256": hashlib.sha256(prompt.encode()).hexdigest(), "output_checked": case != "owner_only", "artifact": received_artifact, "output": received_output}))
            sequence = str(rows[0]["seq"])
            ack = subprocess.run([runner,"mailbox","ack","--session-id",known,"--from-seq",sequence,"--to-seq",sequence,"--json"],capture_output=True,check=True,timeout=10)
            pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("recipient-exact-ack.json").write_bytes(ack.stdout)
            if case == "two_source":
                second_seq = str(second["seq"])
                second_ack = subprocess.run([runner,"mailbox","ack","--session-id",known,"--from-seq",second_seq,"--to-seq",second_seq,"--json"],capture_output=True,check=True,timeout=10)
                pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("recipient-second-exact-ack.json").write_bytes(second_ack.stdout)
        root = pathlib.Path(os.environ["AGE360_ROOT"])
        if root.joinpath("native-channel-mode").exists():
            channel = os.environ["OULIPOLY_RETURN_CHANNEL"]
            helper_env = dict(os.environ,
                AGE360_NATIVE_RETURN_HELPER_CHANNEL=channel,
                AGE360_NATIVE_RETURN_HELPER_PRODUCER=json.loads(os.environ["OULIPOLY_PARENT_INVOCATION"])["id"],
                AGE360_NATIVE_RETURN_HELPER_DB=str(root / "native-artifact-store.db"))
            subprocess.run([root.joinpath("native-channel-helper").read_text(), "--exact", "native_channel_producer_helper"], env=helper_env, check=True, stdout=subprocess.DEVNULL)
            mode = root.joinpath("native-channel-mode").read_text()
            if mode == "quarantined":
                with open(channel, "a") as stream: stream.write("malformed\n")
            elif mode == "cleanup_failed":
                pathlib.Path(channel).parent.joinpath("retained-cleanup-obligation").write_bytes(b"retain me")
        if os.environ.get("AGE360_DESCENDANT") == "1":
            cancelling = pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("cancel-probe-enabled").exists()
            if cancelling:
                import signal
                signal.signal(signal.SIGTERM, signal.SIG_IGN)
                os.environ["AGE360_CANCEL_TEST"] = "1"
            child = subprocess.Popen(["/bin/sh", "-c", 'if [ "$AGE360_CANCEL_TEST" = 1 ]; then trap "" TERM; touch "$AGE360_ROOT/cancel-descendant-ready"; fi; while [ ! -f "$AGE360_DESCENDANT_GATE" ]; do sleep 0.02; done'], start_new_session=True, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            # Publish only complete real PID bytes; existence is a readiness contract.
            pid_path = pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("descendant.pid")
            pending_pid = pid_path.with_suffix(".pending")
            pending_pid.write_text(str(child.pid))
            pending_pid.replace(pid_path)
            if cancelling:
                deadline=time.monotonic()+45
                while time.monotonic()<deadline: time.sleep(0.02)
                raise RuntimeError("accepted native cancellation failed to stop held provider")
    else:
        known = SESSION
        event(request, seq, "marker", name="oulipoly.provider_session", value={"provider_session_id": known})
        seq += 1
        if case == "listener_only":
            root = pathlib.Path(os.environ["AGE360_ROOT"])
            parent = json.loads(os.environ["OULIPOLY_PARENT_INVOCATION"])["id"]
            registration = next((root / "spool/agent-bash").glob("*/source-registration-v2.json"))
            deadline = time.monotonic() + 30
            while True:
                with sqlite3.connect("file:" + os.environ["OULIPOLY_DATA_DIR"] + "/pid-identity.db?mode=ro", uri=True) as db:
                    bound = db.execute("SELECT session_id FROM runtime_generation WHERE spawn_invocation_uuid=?", (parent,)).fetchone()
                # A requested-session Starting reservation is not marker
                # publication. Wait for launch-owned State authority too, then
                # assert it against this actual pinned launch request.
                with sqlite3.connect("file:" + os.environ["OULIPOLY_DATA_DIR"] + "/state.db?mode=ro", uri=True) as db:
                    db.row_factory = sqlite3.Row
                    actor = dict(db.execute("SELECT invocation_uuid,provider_session_id,session_id,status FROM invocations WHERE invocation_uuid=?", (parent,)).fetchone())
                    authority_row = db.execute("SELECT a.provider_instance_id,a.settings_id FROM invocation_provider_session_authority a JOIN invocations i ON i.id=a.invocation_id WHERE i.invocation_uuid=?", (parent,)).fetchone()
                if bound and bound[0] == SESSION and authority_row is not None:
                    authority = dict(authority_row)
                    assert actor['provider_session_id'] == SESSION and actor['status'] == 'running', actor
                    assert authority['provider_instance_id'] == request['provider_instance_id'], authority
                    assert authority['settings_id'] == params['settings_id'], authority
                    break
                assert time.monotonic() < deadline, "independent native authority publication deadline"
                time.sleep(.02)
            (root / "independent-admission-before.json").write_text(json.dumps(dict(
                actor=actor, endpoint_authority=authority, runtime_session=bound[0], caller_pid=os.getpid())))
            result = subprocess.run([os.environ["AGENT_BASH_AGENT_RUNNER_BIN"], "notify", "agent-bash-listen",
                "--registration-file", str(registration), "--session-id", SESSION,
                "--owner-invocation-uuid", parent, "--json"], capture_output=True, timeout=30)
            (root / "independent-listener-result.json").write_text(json.dumps(dict(
                rc=result.returncode, stdout=result.stdout.decode(), stderr=result.stderr.decode(), invocation=parent)))
            assert result.returncode == 0, result.stderr
            # This resumed synthetic prompt has now actually subscribed. Fulfil
            # the same affirmative resume contract as the recipient branch;
            # clean process exit alone is deliberately insufficient in Runner.
            acceptance_marker = {
                "protocol": "oulipoly.prompt_acceptance/v1",
                "provider_session_id": known,
                "prompt_sha256": hashlib.sha256(prompt.encode("utf-8")).hexdigest(),
                "source": "age360.native.fixture",
                "message_id": "independent-listener-subscribed",
            }
            if params.get("prompt_acceptance", {}).get("delivery_nonce"):
                acceptance_marker["delivery_nonce"] = params["prompt_acceptance"]["delivery_nonce"]
            event(request, seq, "marker", name="oulipoly.prompt_accepted/v1", value=acceptance_marker)
            seq += 1
            event(request, seq, "marker", name="oulipoly.produced_assistant_response", value=True)
            seq += 1
        elif case == "native_missing":
            import runpy
            runpy.run_path(str(pathlib.Path(os.environ["AGE360_ROOT"]) / "native-missing-output.py"))["publish"]()
        elif os.environ.get("AGE360_CASE") != "owner_only":
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
            if os.environ.get("AGE360_MCP_E2E"):
                deadline = time.monotonic() + 45
                while (root / "hold-mcp-admission").exists():
                    if time.monotonic() > deadline: raise RuntimeError("MCP audit admission gate expired")
                    time.sleep(.02)
            mode = "sync" if case in ("sync", "sync-independent") or case.startswith("detach-") else "async"
            env = dict(os.environ, AGENT_BASH_OWNER_SESSION_ID=SESSION, AGENT_BASH_OWNER_INVOCATION_UUID=parent)
            workload = "printf paired-source-output"
            if mode == "async" or case in ("detach-before", "detach-race"): workload = 'while [ ! -f "$AGE360_WORKLOAD_GATE" ]; do sleep 0.02; done; printf paired-source-output'
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
            elif case in ("publication_error", "publication_io_error", "missing_selection", "missing_pin", "missing_short"):
                extra = ["--ready-sentinel", "paired-source-output"]
                workload = 'printf paired-source-output; while [ ! -f "$AGE360_ROOT/write-more" ]; do sleep .02; done; head -c 1048576 /dev/zero; touch "$AGE360_ROOT/writer-done"; exec sleep 600'
            elif case == "hash_cancel":
                env["AGENT_BASH_LOG_MAX_BYTES"] = str(32 * 1024 * 1024)
                extra = ["--ready-sentinel", "READY"]
                workload = 'head -c 16777216 /dev/zero; printf "READY\\n"; exec sleep 600'
            if case in ("missing_selection", "missing_pin", "missing_short"):
                env["AGENT_BASH_LOG_MAX_BYTES"] = "65536"
            workload = 'printf "launch\\n" >> "$AGE360_ROOT/source-launches"; ' + workload
            if os.environ.get("AGE360_MCP_E2E"):
                from mcp_client import dispatch
                result = dispatch(root, mode, workload, env)
            else:
                result = subprocess.run([os.environ["AGE360_AGENT_BASH_BIN"], "run", "--delivery", mode, "--completion-scope", scope, *extra, "--", "/bin/sh", "-c", workload], env=env, capture_output=True, timeout=30)
            (root / "bash-dispatch.stdout").write_bytes(result.stdout)
            (root / "bash-dispatch.stderr").write_bytes(result.stderr)
            allowed = (0, 37) if os.environ["AGE360_CASE"] == "early_exit" else (0,)
            if result.returncode not in allowed: raise RuntimeError("actual paired Bash dispatch failed: " + result.stderr.decode(errors="replace"))
            if case == "two_source":
                second = subprocess.run([os.environ["AGE360_AGENT_BASH_BIN"], "run", "--delivery", "async", "--completion-scope", "tree", "--", "/bin/sh", "-c", workload], env=env, capture_output=True, timeout=30)
                (root / "bash-second-dispatch.stdout").write_bytes(second.stdout)
                (root / "bash-second-dispatch.stderr").write_bytes(second.stderr)
                if second.returncode != 0: raise RuntimeError("second native Bash dispatch failed: " + second.stderr.decode(errors="replace"))
            (root / "provider-dispatched").touch()
            if case in ("publication_race", "publication_error", "publication_io_error", "hash_cancel", "missing_selection", "missing_pin", "missing_short"):
                # Actual original owner issues cancellation through the exact Bash
                # CLI; the namespace controller cannot impersonate its ancestry.
                deadline = time.monotonic() + 45
                while not (root / "cancel-source").exists():
                    if time.monotonic() > deadline: raise RuntimeError("test did not request owner cancellation")
                    time.sleep(.02)
                handle = json.loads(result.stdout)["handle"]
                cancelled = subprocess.run([os.environ["AGE360_AGENT_BASH_BIN"], "cancel", handle], env=env, capture_output=True, timeout=30)
                (root / "source-cancel-result.json").write_text(json.dumps({"rc": cancelled.returncode, "stdout": cancelled.stdout.decode(), "stderr": cancelled.stderr.decode(), "owner_pid": os.getpid()}))
            if mode == "sync" and not os.environ.get("AGE360_MCP_E2E"):
                # Real in-call output protocol, not a mailbox recipient ACK.
                handle = json.loads(result.stdout)["handle"]
                binary = os.environ["AGE360_AGENT_BASH_BIN"]
                deadline = time.monotonic() + 30
                while True:
                    status = subprocess.run([binary, "status", handle, "--observe-only"], env=env, capture_output=True, timeout=30, check=True)
                    if status.stdout.startswith(b"DONE "): break
                    if time.monotonic() > deadline: raise RuntimeError("sync source did not complete: " + status.stdout.decode())
                    time.sleep(.02)
                snapshot = subprocess.run([binary, "snapshot", handle], env=env, capture_output=True, timeout=30, check=True)
                acquired = json.loads(snapshot.stdout)
                body = bytes.fromhex(acquired["output"])
                assert body == b"paired-source-output", acquired
                assert hashlib.sha256(body).hexdigest() == acquired["snapshot"]["sha256"]
                receipt = subprocess.run([binary, "accept-output", handle, "--snapshot", json.dumps(acquired["snapshot"])], env=env, capture_output=True, timeout=30, check=True)
                subprocess.run([binary, "status", handle], env=env, capture_output=True, timeout=30, check=True)
                (root / "sync-byte-response.json").write_text(json.dumps({"output": body.decode(), "receipt": json.loads(receipt.stdout)}))
                deadline = time.monotonic() + 30
                while not (root / "release-initial-provider").exists():
                    if time.monotonic() > deadline: raise RuntimeError("test did not release initial provider")
                    time.sleep(0.02)
        if os.environ.get("AGE360_MCP_E2E") and (case in ("sync", "sync-independent") or case.startswith("detach-")):
            deadline = time.monotonic() + 45
            while not pathlib.Path(os.environ["AGE360_ROOT"]).joinpath("release-initial-provider").exists():
                if time.monotonic() > deadline: raise RuntimeError("MCP initial hold expired")
                time.sleep(.02)
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
        gate = pathlib.Path(os.environ["AGE360_ROOT"]) / ("release-founder" if os.environ.get("AGE360_FOUNDER") else "release-initial-provider")
        deadline = time.monotonic() + (240 if os.environ.get("AGE360_FOUNDER") else 45)
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
root = pathlib.Path(__file__).parent
hold = root / ("hold-native-" + method)
if hold.exists():
    # Session-observer/preflight calls can share this provider. Hold only an
    # actually attributed native attempt actor, not whichever describe ran first.
    ancestors = {}
    pid = os.getpid()
    while pid > 0:
        raw = pathlib.Path(f"/proc/{pid}/stat").read_text()
        fields = raw.rsplit(")", 1)[1].split()
        ancestors[pid] = {"pid": pid, "starttime": int(fields[19]), "stat": raw}
        pid = int(fields[1])
    matches = []
    for stat in root.glob("data/state.native-producer-custody/*/actors/*/proxy.stat"):
        raw = stat.read_text()
        if not raw:
            continue
        pid = int(raw.split()[0])
        starttime = int(raw.rsplit(")", 1)[1].split()[19])
        if pid in ancestors and ancestors[pid]["starttime"] == starttime:
            intent = json.loads(stat.with_name("intent.json").read_text())
            assert intent["attempt_id"] == stat.parents[2].name
            matches.append({"journal": str(stat), "attempt_id": intent["attempt_id"],
                            "proxy": ancestors[pid], "operation": intent["operation"]})
    if matches:
        assert len(matches) == 1, matches
        observation = {"provider": ancestors[os.getpid()], "actor": matches[0],
                       "ancestors": list(ancestors.values()), "method": method,
                       "request_id": request["request_id"],
                       "boot_id": pathlib.Path("/proc/sys/kernel/random/boot_id").read_text().strip()}
        (root / ("native-" + method + ".actor.json")).write_text(json.dumps(observation))
        (root / ("native-" + method + ".reached")).write_text(str(os.getpid()))
        while hold.exists(): time.sleep(0.02)
        if method == "launch" and root.joinpath("native-launch-exit-zero-without-output").exists():
            # Actual original process exits normally without producing the
            # required protocol final. No fixture authors a runtime/custody fact.
            sys.exit(0)
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
