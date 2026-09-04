#!/usr/bin/env python3
import base64
import binascii
import hashlib
import json
import os
import pathlib
import subprocess
import sys
from datetime import datetime
from typing import NoReturn


try:
    request = json.load(sys.stdin)
except json.JSONDecodeError:
    request = {}
contract = request.get("contract", "oulipoly.provider/v1")
request_id = request.get("request_id", "historical-test-fixture")
operation = sys.argv[1] if len(sys.argv) > 1 else ""

profile = "__OULIPOLY_FIXTURE_PROFILE__"
fixture_config = json.loads(bytes.fromhex("__OULIPOLY_FIXTURE_CONFIG_HEX__"))
enabled = set() if profile.startswith("__") else set(profile)
prompt_acceptance_enabled = bool(fixture_config.get("prompt_acceptance_patterns"))
capabilities = {
    "launch": "l" in enabled,
    "policy": "p" in enabled,
    "quota": "q" in enabled,
    "session": "s" in enabled,
    "session_enumerate": "e" in enabled,
    "terminal": "t" in enabled,
    "rotation": False,
    "discovery": False,
    "settings": False,
    "setup_brain": False,
    "setup": False,
    "migration": False,
}
host_env = request.get("host", {}).get("env", {})
if (
    prompt_acceptance_enabled
    and host_env.get("OULIPOLY_HOST_PROMPT_ACCEPTANCE_V1") == "1"
):
    capabilities["prompt_acceptance_v1"] = True
if "l" in enabled and host_env.get("OULIPOLY_HOST_LAUNCH_OUTPUT_V1") == "1":
    capabilities["launch_output_v1"] = True
if "s" in enabled and host_env.get("OULIPOLY_HOST_SESSION_TURN_PAGES_V1") == "1":
    capabilities["session_turn_pages_v1"] = True


def success(result):
    print(json.dumps({
        "contract": contract,
        "request_id": request_id,
        "ok": True,
        "result": result,
    }, separators=(",", ":")), flush=True)


def unsupported():
    print(json.dumps({
        "contract": contract,
        "request_id": request_id,
        "ok": False,
        "error": {
            "code": "unsupported_operation",
            "category": "unsupported",
            "message": f"historical fixture profile {profile} does not support {operation}",
            "retryable": False,
            "details": {"operation": operation, "profile": profile},
        },
    }, separators=(",", ":")), flush=True)
    raise SystemExit(1)


def failure(code, message, category="failed", details=None) -> NoReturn:
    error = {
        "code": code,
        "category": category,
        "message": message,
        "retryable": False,
    }
    if details:
        error["details"] = details
    print(json.dumps({
        "contract": contract,
        "request_id": request_id,
        "ok": False,
        "error": error,
    }, separators=(",", ":")), flush=True)
    raise SystemExit(1)


def event(seq, kind, **fields):
    value = {
        "contract": contract,
        "request_id": request_id,
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": kind,
    }
    value.update(fields)
    print(json.dumps(value, separators=(",", ":")), flush=True)


def launch_stdin(params):
    payload = params.get("stdin")
    if payload is None:
        return None
    if payload.get("encoding") == "utf8":
        return payload.get("data", "").encode("utf-8")
    return base64.b64decode(payload.get("data", ""), validate=True)


def known_launch_session_id(params):
    known = params.get("session", {}).get("known_provider_session_id")
    if known:
        return known
    return None


def insert_before_prompt(params, argv, values):
    if not values:
        return argv
    prompt = params.get("model", {}).get("inputs", {}).get("prompt")
    insert_at = len(argv)
    if argv and prompt is not None and argv[-1] == prompt:
        insert_at -= 1
    argv[insert_at:insert_at] = values
    return argv


def launch_argv(params):
    argv = list(params.get("argv", []))
    session_id = known_launch_session_id(params)
    if session_id:
        resume_args = list(fixture_config.get("resume_args", []))
        insert_before_prompt(params, argv, resume_args + [session_id])
        prompt = params.get("model", {}).get("inputs", {}).get("prompt")
        if prompt == "" and argv and argv[-1] == "":
            argv.pop()
        return argv

    capture = fixture_config.get("session_capture", {})
    insert_before_prompt(params, argv, list(capture.get("args", [])))
    if capture.get("kind") == "forced_flag_verified" and capture.get("flag"):
        insert_before_prompt(
            params,
            argv,
            [capture["flag"], "historical-test-session"],
        )
    return argv


def value_at_path(value, path):
    current = value
    for component in path.split("."):
        if not isinstance(current, dict):
            return None
        current = current.get(component)
    return current


def captured_launch_session_id(params, stdout):
    known = known_launch_session_id(params)
    if known:
        return known

    capture = fixture_config.get("session_capture", {})
    if capture.get("kind") == "forced_flag_verified" and capture.get("flag"):
        return "historical-test-session"
    if capture.get("kind") != "stdout_json_event":
        return None

    event_type = capture.get("event_type")
    event_id_path = capture.get("event_id_path")
    if not event_id_path:
        return None
    for line in stdout.decode("utf-8", errors="replace").splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event_type and value.get("type") != event_type:
            continue
        session_id = value_at_path(value, event_id_path)
        if isinstance(session_id, str) and session_id:
            return session_id
    return None


def launch():
    params = request.get("params", {})
    argv = launch_argv(params)
    environment = os.environ.copy()
    environment.update(params.get("env") or {})
    try:
        completed = subprocess.run(
            argv,
            cwd=params.get("working_directory"),
            env=environment,
            input=launch_stdin(params),
            capture_output=True,
            check=False,
        )
        stdout = completed.stdout
        stderr = completed.stderr
        if completed.returncode < 0:
            status = {"kind": "signal_terminated", "signal": -completed.returncode}
            terminal_kind = "signal_exit"
        else:
            status = {"kind": "exited", "code": completed.returncode}
            terminal_kind = "clean_exit" if completed.returncode == 0 else "nonzero_exit"
        spawn_succeeded = True
    except (OSError, ValueError) as error:
        stdout = b""
        stderr = str(error).encode("utf-8")
        status = {"kind": "spawn_error", "reason": str(error)}
        terminal_kind = "spawn_error"
        spawn_succeeded = False

    seq = 1
    data_event_count = 0
    for kind, data in (("stdout", stdout), ("stderr", stderr)):
        for offset in range(0, len(data), 256 * 1024):
            chunk = data[offset:offset + 256 * 1024]
            event(seq, kind, data_base64=base64.b64encode(chunk).decode("ascii"))
            seq += 1
            data_event_count += 1

    if stdout and spawn_succeeded:
        event(seq, "marker", name="oulipoly.produced_assistant_response", value=True)
        seq += 1

    acceptance = params.get("prompt_acceptance")
    session_id = captured_launch_session_id(params, stdout)
    acceptance_patterns = fixture_config.get("prompt_acceptance_patterns") or []
    acceptance_text = (stdout + b"\n" + stderr).decode("utf-8", errors="replace")
    acceptance_confirmed = any(pattern in acceptance_text for pattern in acceptance_patterns)
    if (
        acceptance is not None
        and spawn_succeeded
        and prompt_acceptance_enabled
        and acceptance_confirmed
    ):
        marker = {
            "protocol": "oulipoly.prompt_acceptance/v1",
            "provider_session_id": session_id,
            "prompt_sha256": acceptance["prompt_sha256"],
            "source": "historical-test-fixture",
        }
        if acceptance.get("delivery_nonce"):
            marker["delivery_nonce"] = acceptance["delivery_nonce"]
        event(seq, "marker", name="oulipoly.prompt_accepted/v1", value=marker)
        seq += 1

    if params.get("output_delivery") is not None:
        event(seq, "marker", name="oulipoly.launch_output_complete/v1", value={
            "protocol": "oulipoly.launch_output/v1",
            "stdout": {"bytes": len(stdout), "sha256": hashlib.sha256(stdout).hexdigest()},
            "stderr": {"bytes": len(stderr), "sha256": hashlib.sha256(stderr).hexdigest()},
            "data_event_count": data_event_count,
        })
        seq += 1

    exit_fields = {
        "status": status,
        "terminal_signal": {
            "kind": terminal_kind,
            "evidence": "historical fixture launch adapter",
            "observed_at_unix_ms": 1000 + seq,
        },
    }
    if session_id:
        exit_fields["session"] = {"provider_session_id": session_id}
    event(seq, "exit", **exit_fields)


def policy_evaluate():
    launch = request.get("params", {}).get("launch", {})
    argv = list(launch.get("argv", []))
    original_argv = list(argv)
    prompt = launch.get("prompt")
    transformed_prompt = prompt
    system_prompt = launch.get("system_prompt_override")
    restrictions = launch.get("tool_restrictions") or {}
    kind = restrictions.get("kind")
    policy_args = []

    if kind == "claude":
        if system_prompt:
            policy_args.extend(["--append-system-prompt", system_prompt])
        claude = restrictions.get("claude", {})
        if claude.get("disallowed_tools"):
            policy_args.extend(["--disallowed-tools", ",".join(claude["disallowed_tools"])])
        if claude.get("allowed_tools"):
            policy_args.extend(["--allowed-tools", ",".join(claude["allowed_tools"])])
        if claude.get("disable_slash_commands"):
            policy_args.append("--disable-slash-commands")
    elif kind == "codex":
        codex = restrictions.get("codex", {})
        for pair in codex.get("config_pairs", []):
            policy_args.extend(["-c", pair])
        for feature in codex.get("disabled_features", []):
            policy_args.extend(["--disable", feature])
        if system_prompt and prompt is not None:
            transformed_prompt = (
                f"<<<NESTHARUS-POLICY>>>\n{system_prompt}\n"
                f"<<<END-POLICY>>>\n\n{prompt}"
            )

    if transformed_prompt != prompt and argv and argv[-1] == prompt:
        argv[-1] = transformed_prompt
    if policy_args:
        insert_at = len(argv) - 1 if argv and argv[-1] == transformed_prompt else len(argv)
        argv[insert_at:insert_at] = policy_args

    result = {
        "accepted": True,
        "diagnostics": [],
        "markers": [],
    }
    if argv != original_argv:
        result["argv"] = argv
    if transformed_prompt != prompt:
        result["prompt"] = transformed_prompt
        if launch.get("stdin") == prompt:
            result["stdin"] = transformed_prompt
    success(result)


def quota_source():
    quota_script = fixture_config.get("quota_script")
    success({
        "has_source": quota_script is not None,
        "source_id": "historical-test-fixture" if quota_script is not None else None,
        "freshness": "probe_required" if quota_script is not None else "unavailable",
    })


def quota_probe():
    quota_script = fixture_config.get("quota_script")
    if not isinstance(quota_script, str):
        unsupported()
    completed = subprocess.run(
        ["sh", "-c", quota_script],
        env=os.environ.copy(),
        capture_output=True,
        check=False,
        text=True,
    )
    if completed.returncode != 0:
        failure(
            "quota_probe_failed",
            completed.stderr.strip() or f"quota script exited {completed.returncode}",
            details={"exit_code": completed.returncode},
        )
    try:
        parsed = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        failure("quota_probe_invalid_json", f"quota script emitted invalid JSON: {error}")
    windows = parsed.get("windows")
    if windows is None and parsed.get("used_percent") is not None:
        windows = [{
            "used_percent": parsed["used_percent"],
            "resets_at": parsed.get("resets_at"),
        }]
    projected = []
    for window in windows or []:
        resets_at = window.get("resets_at")
        if not resets_at:
            failure("quota_probe_invalid_window", "quota window is missing resets_at")
        try:
            reset = datetime.fromisoformat(resets_at.replace("Z", "+00:00"))
        except ValueError as error:
            failure("quota_probe_invalid_window", f"invalid quota reset timestamp: {error}")
        projected_window = {
            "remaining_ratio": 1.0 - float(window["used_percent"]) / 100.0,
            "resets_at_unix_ms": int(reset.timestamp() * 1000),
        }
        if window.get("name") is not None:
            projected_window["name"] = window["name"]
        projected.append(projected_window)
    success({
        "available": True,
        "checked_at_unix_ms": 0,
        "windows": projected,
        "detail": "historical fixture quota is available",
    })


def quota_refresh_auth():
    command = fixture_config.get("auth_refresh_command")
    if isinstance(command, str):
        completed = subprocess.run(
            ["sh", "-c", command],
            env=os.environ.copy(),
            capture_output=True,
            check=False,
            text=True,
        )
        if completed.returncode != 0:
            failure(
                "quota_refresh_auth_failed",
                completed.stderr.strip() or f"auth refresh exited {completed.returncode}",
                details={"exit_code": completed.returncode},
            )
    success({
        "refreshed": isinstance(command, str),
        "available": True,
        "checked_at_unix_ms": 0,
        "detail": "historical fixture authentication refreshed",
    })


def evidence_session_id_from(params):
    live_report = params.get("live_report") or {}
    if live_report.get("provider_session_id"):
        return live_report["provider_session_id"]
    if params.get("session_id"):
        return params["session_id"]
    for marker in params.get("markers", []):
        if marker.get("name") == "provider_session_id" and marker.get("value"):
            return marker["value"]
    return None


def storage_config() -> dict | None:
    return fixture_config.get("session_storage") or None


def storage_format(storage):
    if not storage:
        return None
    kind = storage.get("kind")
    if kind == "claude_code":
        return "claude_code"
    if kind == "codex":
        return "codex_session"
    if kind == "script":
        return storage.get("storage_type")
    return None


def unsupported_storage(reason) -> NoReturn:
    failure(
        "unsupported_storage",
        reason,
        category="unsupported",
        details={"provider_name": fixture_config.get("account_name", "historical-test-fixture")},
    )


def single_path(candidates, not_found, ambiguous):
    unique = sorted({str(path.resolve()) for path in candidates})
    if not unique:
        unsupported_storage(not_found)
    if len(unique) != 1:
        unsupported_storage(f"{ambiguous}: {', '.join(unique)}")
    return pathlib.Path(unique[0])


def locate_script_transcript(storage, session_id):
    script = storage.get("transcript_script")
    if not script:
        unsupported_storage("no transcript locator for script storage")
    completed = subprocess.run(
        ["sh", "-c", f'{script} "$1"', "oulipoly-session-script", session_id],
        env={**os.environ, "SESSION_ID": session_id},
        stdin=subprocess.DEVNULL,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        stderr = completed.stderr.decode("utf-8", errors="replace").strip()
        unsupported_storage(
            f"transcript locator exited {completed.returncode}: {stderr}"
        )
    lines = [line.strip() for line in completed.stdout.decode(
        "utf-8", errors="replace"
    ).splitlines() if line.strip()]
    if len(lines) != 1:
        unsupported_storage("transcript locator must return exactly one path")
    return pathlib.Path(lines[0])


def codex_transcript_matches(path, session_id):
    try:
        with path.open("r", encoding="utf-8") as transcript:
            for raw_line in transcript:
                try:
                    value = json.loads(raw_line)
                except json.JSONDecodeError:
                    continue
                if (value.get("type") == "session_meta"
                        and value.get("payload", {}).get("id") == session_id):
                    return True
    except (OSError, UnicodeError):
        return False
    return False


def locate_storage_transcript(session_id):
    storage = storage_config()
    if not storage:
        unsupported_storage("storage type is other and no transcript locator is configured")
    kind = storage.get("kind")
    if kind == "claude_code":
        root = pathlib.Path(storage.get("projects_dir", ""))
        if not root.is_dir():
            unsupported_storage(f"claude projects directory unavailable: {root}")
        return single_path(
            root.rglob(f"{session_id}.jsonl"),
            "claude storage scan did not locate the requested session",
            "claude storage scan is ambiguous",
        )
    if kind == "codex":
        root = pathlib.Path(storage.get("sessions_dir", ""))
        if not root.is_dir():
            unsupported_storage(f"codex sessions directory unavailable: {root}")
        return single_path(
            (path for path in root.rglob("*.jsonl")
             if codex_transcript_matches(path, session_id)),
            "codex storage scan did not locate the requested session",
            "codex storage scan is ambiguous",
        )
    if kind == "script":
        return locate_script_transcript(storage, session_id)
    unsupported_storage(f"unsupported storage kind: {kind or 'other'}")


def locate_result(params):
    session_id = params.get("session_id")
    if not session_id:
        failure("invalid_session_id", "session id is required", category="invalid_request")
    path = locate_storage_transcript(session_id)
    mode = params.get("lookup_mode", "require_existing")
    require_existing = mode == "require_existing"
    if not path.is_absolute():
        unsupported_storage(f"transcript locator returned relative path: {path}")
    if require_existing and not path.exists():
        unsupported_storage(f"transcript path does not exist: {path}")
    resolved = path.resolve() if path.exists() else path
    return {
        "located": True,
        "path": str(resolved),
        "format_id": storage_format(storage_config()),
        "source_id": str(resolved),
        "require_existing_observed": require_existing,
    }


def session_locate_transcript():
    success(locate_result(request.get("params", {})))


def session_enumerate():
    success({
        "sessions": [],
        "complete": True,
        "warnings": [],
    })


def session_read_turns():
    params = request.get("params", {})
    session_id = evidence_session_id_from(params) or "historical-test-session"
    success({
        "read_protocol": "oulipoly.session_turn_pages/v1",
        "provider_instance_id": request.get("provider_instance_id") or "historical-test-fixture",
        "settings_id": params.get("settings_id", "historical-test-fixture"),
        "session_id": session_id,
        "turn_projection": params.get("turn_projection", "canonical_ingest"),
        "snapshot_id": f"historical-test-snapshot:{session_id}",
        "page_index": 0,
        "page_start_sequence": 0,
        "turns": [],
        "page_turn_count": 0,
        "source_bytes_examined": 0,
        "scan_progress": False,
        "snapshot_complete": True,
        "next_page_token": None,
        "resume_token": "historical-test-resume:0",
        "source_final": True,
        "warnings": [],
    })


def session_capture():
    params = request.get("params", {})
    success({
        "provider_session_id": evidence_session_id_from(params),
        "state": {"kind": "historical-test-fixture"},
        "artifacts": [],
    })


def json_bytes(value):
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def scan_jsonl(path):
    try:
        data = path.read_bytes()
    except OSError as error:
        failure(
            "malformed_provider_transcript",
            f"failed to read transcript {path}: {error}",
            details={"path": str(path), "line": 0, "reason": str(error)},
        )
    lines = []
    line_no = 1
    offset = 0
    while offset < len(data):
        start = offset
        while offset < len(data) and data[offset] != 10:
            offset += 1
        end = offset
        if end > start and data[end - 1] == 13:
            end -= 1
        raw = data[start:end]
        if raw.strip():
            try:
                text = raw.decode("utf-8")
                value = json.loads(text)
            except (UnicodeError, json.JSONDecodeError) as error:
                malformed_transcript(path, line_no, str(error))
            lines.append({
                "line": line_no,
                "byte_start": start,
                "byte_end": end,
                "sha256": hashlib.sha256(raw).hexdigest(),
                "value": value,
            })
        if offset < len(data):
            offset += 1
        line_no += 1
    return lines


def malformed_transcript(path, line, reason) -> NoReturn:
    failure(
        "malformed_provider_transcript",
        f"malformed provider transcript {path} at line {line}: {reason}",
        details={"path": str(path), "line": line, "reason": reason},
    )


def required_string(line, field, path) -> str:
    value = line["value"].get(field)
    if not isinstance(value, str) or not value:
        malformed_transcript(path, line["line"], f"missing required {field}")
    return value


def timestamp_key(timestamp, path, line) -> datetime:
    try:
        return datetime.fromisoformat(timestamp.replace("Z", "+00:00"))
    except ValueError as error:
        malformed_transcript(path, line, f"timestamp is not RFC3339: {error}")


def validate_timestamp_order(records, path):
    previous = None
    for record in records:
        current = timestamp_key(
            record["timestamp"], path, record["source"]["line"]
        )
        if previous is not None and current < previous:
            malformed_transcript(
                path,
                record["source"]["line"],
                "transcript timestamps are not in provider order",
            )
        previous = current


def content_chunks(value):
    if isinstance(value, str):
        return [{"type": "text", "text": value}]
    if isinstance(value, list):
        chunks = []
        for item in value:
            if isinstance(item, str):
                chunks.append({"type": "text", "text": item})
                continue
            if not isinstance(item, dict):
                continue
            kind = item.get("type", "text")
            if kind in ("input_text", "output_text"):
                kind = "text"
            text = item.get("text")
            if not isinstance(text, str):
                text = item.get("content")
            if not isinstance(text, str):
                text = None
            chunks.append({"type": kind, "text": text})
        return chunks
    if isinstance(value, dict) and isinstance(value.get("text"), str):
        return [{"type": "text", "text": value["text"]}]
    return []


def record_source(path, storage_type, line):
    return {
        "storage_type": storage_type,
        "jsonl_path": str(path),
        "line": line["line"],
        "byte_start": line["byte_start"],
        "byte_end": line["byte_end"],
        "sha256": line["sha256"],
    }


def claude_records(path, session_id, provider_name, storage_type):
    records = []
    latest_compaction = None
    for line in scan_jsonl(path):
        value = line["value"]
        native_session = value.get("sessionId")
        if native_session is None:
            continue
        if native_session != session_id:
            malformed_transcript(
                path,
                line["line"],
                f"transcript sessionId {native_session} does not match requested session {session_id}",
            )
        native_type = value.get("type")
        if not isinstance(native_type, str):
            continue
        turn_id = required_string(line, "uuid", path)
        timestamp = required_string(line, "timestamp", path)
        timestamp_key(timestamp, path, line["line"])
        unsupported_record = native_type not in ("user", "assistant")
        content = []
        if not unsupported_record:
            message = value.get("message")
            if isinstance(message, str):
                content = content_chunks(message)
            elif isinstance(message, dict) and "content" in message:
                content = content_chunks(message.get("content"))
            else:
                content = content_chunks(value.get("content"))
        record = {
            "session_id": session_id,
            "provider_name": provider_name,
            "turn_id": turn_id,
            "role": native_type,
            "timestamp": timestamp,
            "content": content,
            "source": record_source(path, storage_type, line),
            "unsupported_record": unsupported_record,
        }
        if value.get("isCompactSummary") is True:
            latest_compaction = len(records)
        records.append(record)
    if latest_compaction is not None:
        records = records[latest_compaction:]
    validate_timestamp_order(records, path)
    return records


def codex_records(path, session_id, provider_name, storage_type):
    records = []
    saw_session_meta = False
    for line in scan_jsonl(path):
        value = line["value"]
        native_type = value.get("type")
        if native_type == "session_meta":
            if value.get("payload", {}).get("id") == session_id:
                saw_session_meta = True
            continue
        if native_type != "response_item":
            continue
        payload = value.get("payload")
        if not isinstance(payload, dict) or payload.get("type") != "message":
            continue
        role = payload.get("role")
        if role not in ("user", "assistant"):
            continue
        timestamp = required_string(line, "timestamp", path)
        timestamp_key(timestamp, path, line["line"])
        turn_id = payload.get("id")
        if not isinstance(turn_id, str) or not turn_id:
            turn_id = f"{path}:{line['line']}"
        records.append({
            "session_id": session_id,
            "provider_name": provider_name,
            "turn_id": turn_id,
            "role": role,
            "timestamp": timestamp,
            "content": content_chunks(payload.get("content")),
            "source": record_source(path, storage_type, line),
            "unsupported_record": False,
        })
    if not saw_session_meta:
        malformed_transcript(
            path, 0, f"transcript is missing matching codex session_meta for {session_id}"
        )
    validate_timestamp_order(records, path)
    return records


def canonical_records(path, session_id, provider_name):
    storage_type = storage_format(storage_config())
    if storage_type == "claude_code":
        return claude_records(path, session_id, provider_name, storage_type)
    if storage_type == "codex_session":
        return codex_records(path, session_id, provider_name, storage_type)
    unsupported_storage(f"unsupported storage format: {storage_type or 'other'}")


def canonical_bytes(records):
    return b"".join(json_bytes(record) + b"\n" for record in records)


def session_export():
    params = request.get("params", {})
    session_id = params.get("session_id")
    provider_name = params.get(
        "provider_name", fixture_config.get("account_name", "historical-test-fixture")
    )
    path = pathlib.Path(locate_result(params)["path"])
    transcript = canonical_bytes(canonical_records(path, session_id, provider_name))
    success({
        "canonical_format": "oulipoly.canonical_transcript/v1",
        "data_base64": base64.b64encode(transcript).decode("ascii"),
        "turn_count": len([line for line in transcript.splitlines() if line.strip()]),
        "sha256": hashlib.sha256(transcript).hexdigest(),
    })


def canonical_input(params):
    descriptor = params.get("canonical_transcript") or {}
    try:
        data = base64.b64decode(descriptor.get("data_base64", ""), validate=True)
    except (ValueError, binascii.Error) as error:
        failure("invalid_canonical_input", str(error), category="invalid_request")
    if hashlib.sha256(data).hexdigest() != descriptor.get("sha256"):
        failure("invalid_canonical_input", "canonical input hash mismatch", category="invalid_request")
    try:
        records = [json.loads(line) for line in data.decode("utf-8").splitlines() if line.strip()]
    except (UnicodeError, json.JSONDecodeError) as error:
        failure("invalid_canonical_input", str(error), category="invalid_request")
    if len(records) != descriptor.get("turn_count"):
        failure("invalid_canonical_input", "canonical input turn count mismatch", category="invalid_request")
    for index, record in enumerate(records, start=1):
        if record.get("session_id") != params.get("session_id"):
            failure(
                "invalid_canonical_input",
                "canonical record session/provider does not match the target session",
                category="invalid_request",
                details={"line": index},
            )
        if record.get("provider_name") != params.get("provider_name"):
            failure(
                "invalid_canonical_input",
                "canonical record session/provider does not match the target provider",
                category="invalid_request",
                details={"line": index},
            )
        if record.get("unsupported_record") or record.get("role") not in ("user", "assistant"):
            failure(
                "invalid_canonical_input",
                "unsupported canonical record cannot be rendered losslessly",
                category="invalid_request",
                details={"line": index},
            )
        for chunk in record.get("content", []):
            if chunk.get("text") is None:
                failure(
                    "invalid_canonical_input",
                    f"content chunk type {chunk.get('type')} cannot be rendered losslessly without text",
                    category="invalid_request",
                    details={"line": index},
                )
    return data, records


def semantic_records(records):
    normalized = []
    for record in records:
        value = dict(record)
        value.pop("source", None)
        normalized.append(value)
    return normalized


def render_native(records, storage_type):
    lines = []
    if storage_type == "codex_session":
        session_id = records[0]["session_id"] if records else ""
        lines.append({"type": "session_meta", "payload": {"id": session_id}})
    for record in records:
        if record.get("unsupported_record"):
            failure("unsupported_canonical_record", "unsupported canonical records cannot be rendered")
        if storage_type == "claude_code":
            lines.append({
                "type": record["role"],
                "uuid": record["turn_id"],
                "sessionId": record["session_id"],
                "timestamp": record["timestamp"],
                "message": {
                    "role": record["role"],
                    "content": record["content"],
                },
            })
            continue
        if storage_type == "codex_session":
            content_type = "output_text" if record["role"] == "assistant" else "input_text"
            content = []
            for chunk in record["content"]:
                kind = content_type if chunk.get("type") == "text" else chunk.get("type")
                content.append({"type": kind, "text": chunk.get("text") or ""})
            lines.append({
                "type": "response_item",
                "id": record["turn_id"],
                "timestamp": record["timestamp"],
                "payload": {
                    "id": record["turn_id"],
                    "type": "message",
                    "role": record["role"],
                    "content": content,
                },
            })
            continue
        unsupported_storage(f"unsupported storage format: {storage_type or 'other'}")
    return b"".join(json_bytes(line) + b"\n" for line in lines)


def write_atomic(path, data):
    staged = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    staged.write_bytes(data)
    os.replace(staged, path)


def provider_replace_state_path(path, operation_id):
    return path.with_name(f".{path.name}.{operation_id}.provider-replace.json")


def replace_evidence(
    params,
    path,
    records,
    preimage_hash,
    post_data,
    input_hash,
    recovery_id,
    operation_state,
):
    post_hash = hashlib.sha256(post_data).hexdigest()
    operation_id = params.get("operation_id")
    session_id = params.get("session_id")
    provider_name = params.get(
        "provider_name", fixture_config.get("account_name", "historical-test-fixture")
    )
    source_id = str(path.resolve())
    last_turn = records[-1] if records else None
    plan = {
        "schema_version": 2,
        "operation": "session.replace",
        "replace_protocol": params.get("replace_protocol"),
        "operation_id": operation_id,
        "recovery_id": recovery_id,
        "session_id": session_id,
        "provider_name": provider_name,
        "canonical_format": "oulipoly.canonical_transcript/v1",
        "input_sha256": input_hash,
        "postimage_sha256": post_hash,
        "preimage_sha256_observed": preimage_hash,
        "turn_count": len(records),
        "db_apply": "replace_session_turns_from_canonical_v1",
        "source_id": source_id,
        "last_turn_id": last_turn["turn_id"] if last_turn else "",
        "last_used_at": last_turn["timestamp"] if last_turn else "",
    }
    return {
        "changed": True,
        "operation_id": operation_id,
        "recovery_id": recovery_id,
        "operation_state": operation_state,
        "preimage_sha256_observed": preimage_hash,
        "postimage_sha256": post_hash,
        "canonical_postimage": {
            "format_id": "oulipoly.canonical_transcript/v1",
            "sha256": post_hash,
            "turn_count": len(records),
            "source_id": source_id,
            "data_base64": base64.b64encode(post_data).decode("ascii"),
        },
        "artifacts": [],
        "host_state_plan": plan,
    }


def recover_session_replace(params):
    operation_id = params.get("operation_id")
    recovery_id = params.get("recovery_id") or f"historical-test-recovery:{operation_id}"
    action = params.get("recovery_action")
    if action != "query":
        state = "atomic_committed" if action == "commit" else "rolled_back"
        success({
            "changed": True,
            "operation_id": operation_id,
            "recovery_id": recovery_id,
            "operation_state": state,
            "artifacts": [],
        })
        return

    session_id = params.get("session_id")
    provider_name = params.get(
        "provider_name", fixture_config.get("account_name", "historical-test-fixture")
    )
    path = pathlib.Path(locate_result(params)["path"])
    state_path = provider_replace_state_path(path, operation_id)
    try:
        state = json.loads(state_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        failure(
            "recovery_state_unavailable",
            f"provider recovery state is unavailable: {error}",
        )
    records = canonical_records(path, session_id, provider_name)
    post_data = canonical_bytes(records)
    post_hash = hashlib.sha256(post_data).hexdigest()
    if post_hash != state.get("postimage_sha256"):
        failure(
            "recovery_ambiguous_hash",
            "provider transcript no longer matches the committed replacement postimage",
            category="conflict",
            details={
                "expected": state.get("postimage_sha256"),
                "actual": post_hash,
            },
        )
    success(replace_evidence(
        params,
        path,
        records,
        state["preimage_sha256"],
        post_data,
        post_hash,
        recovery_id,
        "atomic_committed",
    ))


def session_replace():
    params = request.get("params", {})
    if params.get("operation_mode") == "recover":
        recover_session_replace(params)
        return
    session_id = params.get("session_id")
    provider_name = params.get(
        "provider_name", fixture_config.get("account_name", "historical-test-fixture")
    )
    path = pathlib.Path(locate_result(params)["path"])
    current_records = canonical_records(path, session_id, provider_name)
    current_data = canonical_bytes(current_records)
    observed_hash = hashlib.sha256(current_data).hexdigest()
    expected_hash = params.get("preimage_sha256_expected")
    if expected_hash is not None and expected_hash != observed_hash:
        failure(
            "preimage_sha256_mismatch",
            "provider transcript preimage does not match the expected hash",
            category="conflict",
            details={"expected": expected_hash, "actual": observed_hash},
        )
    input_data, records = canonical_input(params)
    if semantic_records(current_records) == semantic_records(records):
        success({
            "changed": False,
            "preimage_sha256_observed": observed_hash,
            "postimage_sha256": observed_hash,
            "artifacts": [],
        })
        return
    storage_type = storage_format(storage_config())
    write_atomic(path, render_native(records, storage_type))
    post_records = canonical_records(path, session_id, provider_name)
    post_data = canonical_bytes(post_records)
    post_hash = hashlib.sha256(post_data).hexdigest()
    operation_id = params.get("operation_id")
    recovery_id = f"historical-test-recovery:{operation_id}"
    write_atomic(
        provider_replace_state_path(path, operation_id),
        json_bytes({
            "preimage_sha256": observed_hash,
            "postimage_sha256": post_hash,
        }),
    )
    if os.environ.get("OULIPOLY_IMPORT_REPLACE_TEST_HOOK") == "fail-postimage-verification":
        print("forced fixture postimage verification failure", flush=True)
        raise SystemExit(1)
    success(replace_evidence(
        params,
        path,
        post_records,
        observed_hash,
        post_data,
        hashlib.sha256(input_data).hexdigest(),
        recovery_id,
        "atomic_committed",
    ))


def terminal_classify():
    params = request.get("params", {})
    stdout = base64.b64decode(params.get("stdout_base64", ""))
    stderr = base64.b64decode(params.get("stderr_base64", ""))
    status = params.get("status", {})
    status_kind = status.get("kind")
    contention_evidence = opencode_storage_contention_evidence(stdout, stderr)
    if contention_evidence:
        terminal_kind = "provider_storage_contention"
        evidence = contention_evidence
    elif status_kind == "exited":
        terminal_kind = "clean_exit" if status.get("code") == 0 else "nonzero_exit"
        evidence = "historical fixture process status"
    else:
        terminal_kind = {
            "signal_terminated": "signal_exit",
            "spawn_error": "spawn_error",
            "prolonged_silence": "prolonged_silence",
            "cancelled": "cancelled",
        }.get(status_kind, "unknown")
        evidence = "historical fixture process status"
    success({
        "terminal_signal": {
            "kind": terminal_kind,
            "evidence": evidence,
            "observed_at_unix_ms": params.get("observed_at_unix_ms", 0),
        },
    })


def opencode_storage_contention_evidence(stdout, stderr):
    for data in (stdout, stderr):
        lines = [line.strip() for line in data.decode(
            "utf-8", errors="replace"
        ).splitlines() if line.strip()]
        if not lines:
            continue
        try:
            value = json.loads(lines[-1])
        except json.JSONDecodeError:
            value = None
        if isinstance(value, dict) and value.get("type") == "error":
            error = value.get("error", {})
            detail = error.get("data", {}) if isinstance(error, dict) else {}
            message = detail.get("message") or error.get("message", "")
            name = error.get("name") or detail.get("name") or "unknown"
            if reports_storage_contention(message):
                return f"provider error: opencode {name}: {message}"
    text = stderr.decode("utf-8", errors="replace")
    for line in text.splitlines():
        if reports_storage_contention(line):
            return line[:2048]
    return None


def reports_storage_contention(message):
    lower = message.lower()
    return any(token in lower for token in (
        "failed to execute statement",
        "failed query",
        "database is locked",
        "database is busy",
        "sqlite_busy",
    ))


required_profile_codes = {
    "launch": "l",
    "policy.evaluate": "p",
    "quota.source": "q",
    "quota.probe": "q",
    "quota.refresh_auth": "q",
    "session.locate_transcript": "s",
    "session.enumerate": "e",
    "session.read_turns": "s",
    "session.capture": "s",
    "session.export": "s",
    "session.replace": "s",
    "terminal.classify": "t",
}

handlers = {
    "launch": launch,
    "policy.evaluate": policy_evaluate,
    "quota.source": quota_source,
    "quota.probe": quota_probe,
    "quota.refresh_auth": quota_refresh_auth,
    "session.locate_transcript": session_locate_transcript,
    "session.enumerate": session_enumerate,
    "session.read_turns": session_read_turns,
    "session.capture": session_capture,
    "session.export": session_export,
    "session.replace": session_replace,
    "terminal.classify": terminal_classify,
}

if operation == "describe":
    result = {
        "provider_id": "historical-test-fixture",
        "display_name": "Historical Test Fixture",
        "contract_versions": [contract],
        "preferred_contract": contract,
        "capabilities": capabilities,
    }
    success(result)
    raise SystemExit(0)

required = required_profile_codes.get(operation)
if required is None or operation not in handlers or required not in enabled:
    unsupported()
handlers[operation]()
