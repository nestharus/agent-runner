"""Private delayed-visibility control; never supplies receipt/ACK or edits DB.

Only the actual TerminalBounded inspection sees the selected user turn. Other
readers see an empty page. This models a source becoming visible at the terminal
opportunity without making periodic timing or explicit-resume ownership an oracle.
All owners read the same adjacent durable control, independent of inherited env.
"""
import json
import os
import pathlib
import sqlite3
import time

CONTROL = pathlib.Path(__file__).with_name("slow-control.json")


def configure():
    if CONTROL.exists():
        control = json.loads(CONTROL.read_text())
        os.environ["S11_WORK_DIR"] = control["work_dir"]
        os.environ["S11_OMIT_PROMPT_ACCEPTANCE_CAPABILITY"] = "1"
        os.environ["S11_NO_ASSISTANT_RESULT"] = "1"
        os.environ.pop("S11_READ_TURNS_DELAY_MS", None)


def terminal_target():
    pid = os.getppid()
    while pid > 1:
        proc = pathlib.Path("/proc") / str(pid)
        args = (proc / "cmdline").read_bytes().split(b"\0")
        if b"--internal-native-receipt-helper" in args:
            for arg in args:
                if arg.startswith(b"{"):
                    target = json.loads(arg)
                    if target.get("admission_purpose") == "TerminalBounded":
                        return target
            return None
        pid = int((proc / "stat").read_text().rsplit(")", 1)[1].split()[1])
    return None


def read_page(request, page_fn):
    page = page_fn(request)
    if not CONTROL.exists() or request.get("params", {}).get("turn_projection") != "user_observation":
        return page
    control = json.loads(CONTROL.read_text())
    target = terminal_target()
    if target is None:
        # Hidden suffix is not a consumed prefix. Keep the request checkpoint,
        # including tail reads, rather than advertising len(all appended prompts).
        token = request.get("params", {}).get("after_token") or "s11-anchor:0"
        page["result"].update(turns=[], page_turn_count=0, source_bytes_examined=0,
                              resume_token=token,
                              snapshot_id="s11-hidden:" + token)
        record(control, dict(kind="nonterminal_page", request=request,
                             result=page["result"]))
        return page
    before = receipt_snapshot(control, target)
    # Actual page latency, not a scheduling race or test-owner lifetime bound.
    start = time.monotonic_ns()
    time.sleep(2.5)
    elapsed_ms = (time.monotonic_ns() - start) // 1_000_000
    after = receipt_snapshot(control, target)
    turns = page["result"]["turns"]
    assert len(turns) == 1, page
    if control["wrong_digest"]:
        turns[0]["canonical_text_sha256"] = "0" * 64
    record(control, dict(kind="terminal_page", target=target, request=request, before=before,
                 after=after, elapsed_ms=elapsed_ms, turn=turns[0]["turn_id"],
                 digest=turns[0]["canonical_text_sha256"]))
    return page


def receipt_snapshot(control, target):
    with sqlite3.connect("file:" + control["sidecar"] + "?mode=ro", uri=True) as db:
        db.row_factory = sqlite3.Row
        return dict(db.execute(
            "SELECT delivery_invocation_uuid AS invocation, submission_started_at AS submitted, "
            "observation_confirmed_at AS confirmed, acknowledged_at AS ack "
            "FROM mailbox_delivery_attempts WHERE attempt_id=?", (target["attempt_id"],)
        ).fetchone())


def launch_exit(request):
    if CONTROL.exists():
        control = json.loads(CONTROL.read_text())
        identity = json.loads(request["params"]["env"]["OULIPOLY_PARENT_INVOCATION"])
        record(control, dict(kind="launch_exit", invocation=identity["id"],
                             provider_exit=0, assistant_result=False))


def record(control, event):
    path = pathlib.Path(control["work_dir"]) / "slow-events.jsonl"
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(fd, (json.dumps(event) + "\n").encode())
    finally:
        os.close(fd)


def admit_source(request, session):
    if CONTROL.exists():
        control = json.loads(CONTROL.read_text())
        if control.get("admit_source"):
            from s11_ingress import admit
            admit(request, session, control)
