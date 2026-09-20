"""Synthetic provider with explicit automatic/descendant barriers; no real CLI."""
import base64
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).parent
SESSION = "ses_age360_manual_arbitration"
CONTRACT = "oulipoly.provider/v1"
request = json.loads(sys.stdin.read())
method = sys.argv[1]
params = request.get("params", {})


def reply(result):
    print(json.dumps(dict(contract=CONTRACT, request_id=request["request_id"], ok=True, result=result)), flush=True)


def event(seq, kind, **values):
    print(json.dumps(dict(contract=CONTRACT, request_id=request["request_id"], seq=seq,
                         time_unix_ms=1000 + seq, kind=kind, **values)), flush=True)


def gate(name):
    end = time.monotonic() + 45
    while not ROOT.joinpath(name).exists():
        assert time.monotonic() < end, "fixture barrier expired: " + name
        time.sleep(.02)


if method == "describe":
    reply(dict(provider_id="manual-arbitration-fixture", display_name="manual arbitration",
               contract_versions=[CONTRACT], preferred_contract=CONTRACT,
               capabilities=dict(launch=True, launch_output_v1=True, policy=True,
                                 quota=False, session=True, session_turn_pages_v1=True,
                                 prompt_acceptance_v1=True, terminal=False, rotation=False,
                                 discovery=False, settings=False, setup_brain=False, setup=False, migration=False)))
elif method == "policy.evaluate":
    transformed = "POLICY-TRANSFORMED-MANUAL" if ROOT.joinpath("policy-transform-manual").exists() and "MANUAL-EXACT-INPUT" in params["model"]["inputs"]["prompt"] else None
    reply(dict(accepted=True, env={}, stdin=None, prompt=transformed, diagnostics=[], markers=[]))
elif method == "session.capture":
    reply(dict(provider_session_id=SESSION, state={}, artifacts=[]))
elif method == "session.read_turns":
    reply(dict(read_protocol="oulipoly.session_turn_pages/v1", provider_instance_id=request["provider_instance_id"],
               settings_id=params["settings_id"], session_id=SESSION, turn_projection=params["turn_projection"],
               snapshot_id="manual:0", page_index=0, page_start_sequence=0, turns=[], page_turn_count=0,
               source_bytes_examined=0, scan_progress=False, snapshot_complete=True, next_page_token=None,
               resume_token="manual:0", source_final=False, warnings=[]))
elif method == "launch":
    prompt = params["model"]["inputs"]["prompt"]
    known = params.get("session", {}).get("known_provider_session_id")
    invocation = json.loads(params["env"]["OULIPOLY_PARENT_INVOCATION"])["id"]
    with ROOT.joinpath("launches.jsonl").open("a") as stream:
        stream.write(json.dumps(dict(prompt=prompt, invocation=invocation, pid=os.getpid(), known=known, prompt_acceptance=params.get("prompt_acceptance"))) + "\n")
    automatic = known and "AUTO-CUSTODY-INPUT" in prompt
    if not known:
        ROOT.joinpath("initial-ready").touch()
        gate("release-initial")
    elif automatic:
        ROOT.joinpath("automatic-ready").touch()
        gate("release-automatic")
        # The actual recipient ACKs its exact selected input through the public
        # command. Parent tests never forge acknowledgement or drain receipts.
        env = dict(params["env"])
        runner = env["AGE360_MANUAL_RUNNER"]
        listed = subprocess.run([runner, "mailbox", "list", "--session-id", SESSION, "--all", "--json"],
                                env=env, capture_output=True, check=True, timeout=10)
        rows = json.loads(listed.stdout)["rows"]
        assert len(rows) == 1 and rows[0]["submission_token"] == "automatic-token", rows
        seq = str(rows[0]["seq"])
        ack = subprocess.run([runner, "mailbox", "ack", "--session-id", SESSION,
                              "--from-seq", seq, "--to-seq", seq, "--json"],
                             env=env, capture_output=True, check=True, timeout=10)
        ROOT.joinpath("automatic-exact-ack.json").write_bytes(ack.stdout)
        child = subprocess.Popen(["/bin/sh", "-c", 'while [ ! -f "$1" ]; do sleep .02; done', "fixture", str(ROOT / "release-descendant")],
                                 start_new_session=True, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        ROOT.joinpath("descendant.pid").write_text(str(child.pid))
    seq = 1
    event(seq, "marker", name="oulipoly.provider_session", value=dict(provider_session_id=SESSION)); seq += 1
    if known:
        acceptance = dict(protocol="oulipoly.prompt_acceptance/v1", provider_session_id=SESSION,
                          prompt_sha256=hashlib.sha256(prompt.encode()).hexdigest(), source="manual.fixture", message_id=invocation)
        nonce = params.get("prompt_acceptance", {}).get("delivery_nonce")
        if nonce:
            acceptance["delivery_nonce"] = nonce
        event(seq, "marker", name="oulipoly.prompt_accepted/v1", value=acceptance); seq += 1
    payload = ("automatic-result\n" if automatic else "manual-result\n" if known else "initial-result\n").encode()
    event(seq, "stdout", data_base64=base64.b64encode(payload).decode()); seq += 1
    event(seq, "marker", name="oulipoly.produced_assistant_response", value=True); seq += 1
    event(seq, "marker", name="oulipoly.launch_output_complete/v1", value=dict(protocol="oulipoly.launch_output/v1",
          stdout=dict(bytes=len(payload), sha256=hashlib.sha256(payload).hexdigest()),
          stderr=dict(bytes=0, sha256=hashlib.sha256(b"").hexdigest()), data_event_count=1)); seq += 1
    event(seq, "exit", status=dict(kind="exited", code=0), terminal_signal=dict(kind="clean_exit", evidence="fixture", observed_at_unix_ms=1000 + seq), session=dict(provider_session_id=SESSION, state={}))
else:
    raise AssertionError(method)
