#!/usr/bin/python3
"""Private AGE217 caller fixture: produce custody before rejecting live authority.

Uses the same v1 binary-prefix/standalone-channel protocol as the runtime AGE217
publication regression. No native provider, credential lookup or fallback.
"""
import json
import os
import pathlib
import subprocess
import sys
import sqlite3

root = pathlib.Path(AGE217_EXPLICIT_FIXTURE_ROOT)
request = json.load(sys.stdin)
contract = "oulipoly.provider/v1"

def response(result):
    print(json.dumps(dict(contract=contract, request_id=request["request_id"], ok=True, result=result)), flush=True)

def event(seq, kind, **fields):
    print(json.dumps(dict(contract=contract, request_id=request["request_id"], seq=seq,
                         time_unix_ms=1000 + seq, kind=kind, **fields)), flush=True)

operation = sys.argv[1]
if operation == "describe":
    response(dict(provider_id="caller-fixture", display_name="Private caller fixture",
                  contract_versions=[contract], preferred_contract=contract,
                  capabilities=dict(launch=True, launch_output_v1=True, policy=True,
                                    session=False, quota=False, terminal=False, rotation=False,
                                    discovery=False, settings=False, setup=False, setup_brain=False,
                                    migration=False)))
elif operation == "policy.evaluate":
    response(dict(accepted=True, stdin=None, prompt=None, diagnostics=[], markers=[]))
elif operation == "launch":
    (root / "launch.json").write_text(json.dumps(request))
    env = request["params"]["env"]
    invocation = json.loads(env["OULIPOLY_PARENT_INVOCATION"])["id"]
    helper_env = dict(os.environ, AGE217_PRODUCER_UUID=invocation,
                      AGE217_FIXTURE_ROOT=str(root),
                      AGE217_RETURN_CHANNEL=env["OULIPOLY_RETURN_CHANNEL"])
    helper = (root / "helper-path").read_text()
    completed = subprocess.run([helper, "produce_reference", "--exact", "--nocapture"],
                               env=helper_env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    (root / "helper.stdout").write_bytes(completed.stdout)
    (root / "helper.stderr").write_bytes(completed.stderr)
    assert completed.returncode == 0, completed.stderr
    event(1, "stdout", data_base64="AAH/")
    event(2, "stderr", data_base64="ZXJy//4=")
    # Intentionally not the caller's expected session. Host validation must reject
    # it, keep result.session_id absent, and stop before completion/readiness.
    session = "22222222-2222-4222-8222-222222222222"
    if (root / "publication-storage-fault").exists():
        session = request["params"]["session"]["known_provider_session_id"]
        with sqlite3.connect(root / "data/state.db") as db:
            db.execute("CREATE TRIGGER private_publication_fault BEFORE UPDATE OF provider_session_id ON invocations BEGIN SELECT RAISE(ABORT, 'PRIVATE_STORAGE_SECRET'); END")
    event(3, "marker", name="oulipoly.provider_session",
          value=dict(provider_session_id=session))
else:
    raise AssertionError("unexpected fixture operation: " + operation)
