"""Real private producer admission, using only the launch's supplied authority.

The original shell work completes under its real v2 registration. No artificial
owner keepalive, post-seed gate, or raw mailbox insertion owns its delivery.
"""
import json
import os
import pathlib
import sqlite3
import subprocess
import time


def admit(request, session, control):
    env = dict(os.environ)
    env.update(request["params"]["env"])
    owner = json.loads(env["OULIPOLY_PARENT_INVOCATION"])["id"]
    # Observe actual marker ingestion before dispatch; do not author session State.
    deadline = time.monotonic() + 10
    while True:
        with sqlite3.connect("file:" + control["sidecar"] + "?mode=ro", uri=True) as db:
            row = db.execute("SELECT session_id FROM runtime_generation WHERE spawn_invocation_uuid=?",
                             (owner,)).fetchone()
        if row and row[0] == session:
            break
        assert time.monotonic() < deadline, "provider session marker was not ingested"
        time.sleep(.02)
    root = pathlib.Path(control["work_dir"])
    env.update(AGENT_BASH_OWNER_SESSION_ID=session,
               AGENT_BASH_OWNER_INVOCATION_UUID=owner,
               AGENT_BASH_AGENT_RUNNER_BIN=control["runner"],
               XDG_STATE_HOME=str(root / "spool"),
               S11_SOURCE_LAUNCHES=str(root / "source-launches"))
    command = [control["agent_bash"], "run", "--delivery", "async",
               "--completion-scope", "tree", "--", "/bin/sh", "-c",
               'printf "launch\\n" >> "$S11_SOURCE_LAUNCHES"; '
               'printf s11-admitted-source-output']
    result = subprocess.run(command, env=env, capture_output=True)
    (root / "ingress-result.json").write_text(json.dumps(dict(
        argv=command, owner=owner, session=session, rc=result.returncode,
        stdout_hex=result.stdout.hex(), stderr_hex=result.stderr.hex())))
    assert result.returncode == 0, result.stderr.decode(errors="replace")
    registrations = list((root / "spool/agent-bash").glob("*/source-registration-v2.json"))
    assert len(registrations) == 1, registrations
    registration = json.loads(registrations[0].read_text())
    assert registration["owner_invocation_uuid"] == owner
    assert registration["owner_session_id"] == session
    (root / "ingress-registration.json").write_text(json.dumps(registration))
