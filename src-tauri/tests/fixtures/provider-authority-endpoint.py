#!/usr/bin/env python3
import base64
import hashlib
import json
import os
import subprocess
import sys


try:
    request = json.load(sys.stdin)
except json.JSONDecodeError:
    request = {}
contract = request.get("contract", "oulipoly.provider/v1")
request_id = request.get("request_id", "historical-test-fixture")
operation = sys.argv[1] if len(sys.argv) > 1 else ""

profile = "__OULIPOLY_FIXTURE_PROFILE__"
enabled = set() if profile.startswith("__") else set(profile)
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
if "l" in enabled and host_env.get("OULIPOLY_HOST_PROMPT_ACCEPTANCE_V1") == "1":
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


def launch_session_id(params):
    known = params.get("session", {}).get("known_provider_session_id")
    if known:
        return known
    return "historical-test-session"


def launch():
    params = request.get("params", {})
    argv = params.get("argv", [])
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
    if stdout:
        event(seq, "stdout", data_base64=base64.b64encode(stdout).decode("ascii"))
        seq += 1
        data_event_count += 1
    if stderr:
        event(seq, "stderr", data_base64=base64.b64encode(stderr).decode("ascii"))
        seq += 1
        data_event_count += 1

    acceptance = params.get("prompt_acceptance")
    session_id = launch_session_id(params)
    if acceptance is not None and spawn_succeeded:
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

    event(
        seq,
        "exit",
        status=status,
        terminal_signal={
            "kind": terminal_kind,
            "evidence": "historical fixture launch adapter",
            "observed_at_unix_ms": 1000 + seq,
        },
        session={"provider_session_id": session_id},
    )


def policy_evaluate():
    success({
        "accepted": True,
        "diagnostics": [],
        "markers": [],
    })


def quota_source():
    success({
        "has_source": True,
        "source_id": "historical-test-fixture",
        "freshness": "probe_required",
    })


def quota_probe():
    success({
        "available": True,
        "checked_at_unix_ms": 0,
        "windows": [],
        "detail": "historical fixture quota is available",
    })


def quota_refresh_auth():
    success({
        "refreshed": True,
        "available": True,
        "checked_at_unix_ms": 0,
        "detail": "historical fixture authentication refreshed",
    })


def session_id_from(params):
    live_report = params.get("live_report") or {}
    if live_report.get("provider_session_id"):
        return live_report["provider_session_id"]
    if params.get("session_id"):
        return params["session_id"]
    for marker in params.get("markers", []):
        if marker.get("name") == "provider_session_id" and marker.get("value"):
            return marker["value"]
    return "historical-test-session"


def session_locate_transcript():
    success({"located": False})


def session_enumerate():
    success({
        "sessions": [],
        "complete": True,
        "warnings": [],
    })


def session_read_turns():
    params = request.get("params", {})
    session_id = session_id_from(params)
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
        "provider_session_id": session_id_from(params),
        "state": {"kind": "historical-test-fixture"},
        "artifacts": [],
    })


def session_export():
    transcript = b'{"turns":[]}\n'
    success({
        "canonical_format": "oulipoly.canonical_transcript/v1",
        "data_base64": base64.b64encode(transcript).decode("ascii"),
        "turn_count": 0,
        "sha256": hashlib.sha256(transcript).hexdigest(),
    })


def session_replace():
    success({
        "changed": False,
        "artifacts": [],
    })


def terminal_classify():
    params = request.get("params", {})
    status = params.get("status", {})
    status_kind = status.get("kind")
    if status_kind == "exited":
        terminal_kind = "clean_exit" if status.get("code") == 0 else "nonzero_exit"
    else:
        terminal_kind = {
            "signal_terminated": "signal_exit",
            "spawn_error": "spawn_error",
            "prolonged_silence": "prolonged_silence",
            "cancelled": "cancelled",
        }.get(status_kind, "unknown")
    success({
        "terminal_signal": {
            "kind": terminal_kind,
            "evidence": "historical fixture process status",
            "observed_at_unix_ms": params.get("observed_at_unix_ms", 0),
        },
    })


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
