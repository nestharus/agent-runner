#!/usr/bin/env python3
"""Private protocol fixture: actual prompt acceptance, output and Messenger return.

No State/sidecar receipt is fabricated. Fault injection obstructs only this
invocation's output destination after producing a genuine Store/channel return.
"""
import base64
import hashlib
import json
import os
import pathlib
import subprocess
import sys

CONTRACT = "oulipoly.provider/v1"
SESSION = "e6b7a4c2-97bf-4d4a-8ab3-136dc0c121c1"
request = json.load(sys.stdin)
params = request.get("params", {})
root = pathlib.Path(os.environ["ARTIFACT_FIXTURE_ROOT"])


def envelope(result):
    return dict(contract=CONTRACT, request_id=request["request_id"], ok=True, result=result)


def event(seq, kind, **fields):
    value = dict(contract=CONTRACT, request_id=request["request_id"], seq=seq,
                 time_unix_ms=1000 + seq, kind=kind, **fields)
    with (root / "events.jsonl").open("a") as stream:
        stream.write(json.dumps(value) + "\n")
    print(json.dumps(value), flush=True)


def launch():
    with (root / "launches.jsonl").open("a") as stream:
        stream.write(json.dumps(request) + "\n")
    known = params.get("session", {}).get("known_provider_session_id")
    if known:
        assert known == SESSION
        child_env = os.environ.copy()
        child_env.update(params["env"])
        result = subprocess.run([os.environ["ARTIFACT_FIXTURE_TEST"],
                                 "messenger_return_child", "--exact", "--ignored", "--nocapture"],
                                env=child_env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        (root / "child.json").write_text(json.dumps(dict(rc=result.returncode,
                   stdout=result.stdout.hex(), stderr=result.stderr.hex())))
        assert result.returncode == 0
        identity = json.loads(params["env"]["OULIPOLY_PARENT_INVOCATION"])
        if os.environ.get("ARTIFACT_FIXTURE_OBSTRUCT") == "1":
            destination = pathlib.Path(params["env"]["OULIPOLY_DATA_DIR"]) / "invocations" / "output" / (identity["id"] + ".stdout")
            destination.mkdir(parents=True)
            (root / "obstruction").write_text(str(destination))
    event(1, "marker", name="oulipoly.provider_session", value=dict(provider_session_id=SESSION))
    text = "completed fixture response\n"
    event(2, "stdout", data_base64=base64.b64encode(text.encode()).decode())
    seq = 3
    if known:
        prompt = params["model"]["inputs"]["prompt"]
        acceptance = params["prompt_acceptance"]
        assert acceptance["protocol"] == "oulipoly.prompt_acceptance/v1"
        assert acceptance["prompt_sha256"] == hashlib.sha256(prompt.encode()).hexdigest()
        value = dict(protocol=acceptance["protocol"], provider_session_id=SESSION,
                     prompt_sha256=acceptance["prompt_sha256"],
                     delivery_nonce=acceptance["delivery_nonce"], source="fixture",
                     message_id="accepted-input")
        event(seq, "marker", name="oulipoly.prompt_accepted/v1", value=value)
        seq += 1
    if os.environ.get("ARTIFACT_FIXTURE_NO_ASSISTANT") != "1":
        event(seq, "marker", name="oulipoly.produced_assistant_response", value=True)
        seq += 1
    event(seq, "marker", name="oulipoly.launch_output_complete/v1", value=dict(
        protocol="oulipoly.launch_output/v1",
        stdout=dict(bytes=len(text.encode()), sha256=hashlib.sha256(text.encode()).hexdigest()),
        stderr=dict(bytes=0, sha256=hashlib.sha256(b"").hexdigest()), data_event_count=1))
    event(seq + 1, "exit", status=dict(kind="exited", code=0),
          terminal_signal=dict(kind="clean_exit", evidence="fixture finished", observed_at_unix_ms=1000 + seq + 1),
          session=dict(provider_session_id=SESSION, state={}))


command = sys.argv[1]
if command == "describe":
    print(json.dumps(envelope(dict(provider_id="artifact-fixture", display_name="Artifact fixture",
        contract_versions=[CONTRACT], preferred_contract=CONTRACT,
        capabilities=dict(launch=True, launch_output_v1=True, prompt_acceptance_v1=True,
                          policy=True, quota=False, session=True, session_turn_pages_v1=True,
                          terminal=False, rotation=False, discovery=False, settings=False,
                          setup_brain=False, setup=False, migration=False)))))
elif command == "policy.evaluate":
    print(json.dumps(envelope(dict(accepted=True, env={}, stdin=None, prompt=None, diagnostics=[], markers=[]))))
elif command == "launch":
    launch()
elif command == "session.capture":
    print(json.dumps(envelope(dict(provider_session_id=SESSION, state={}, artifacts=[]))))
elif command == "session.read_turns":
    # No fabricated observation fallback: confirmation must come from the exact
    # affirmative launch marker. Empty bounded source pages support anchoring.
    print(json.dumps(envelope(dict(read_protocol="oulipoly.session_turn_pages/v1",
        provider_instance_id=request["provider_instance_id"], settings_id=params["settings_id"],
        session_id=SESSION, turn_projection=params["turn_projection"], snapshot_id="empty",
        page_index=0, page_start_sequence=0, turns=[], page_turn_count=0,
        source_bytes_examined=0, scan_progress=False, snapshot_complete=True,
        next_page_token=None, resume_token="empty:0", source_final=False, warnings=[]))))
else:
    raise AssertionError(command)
