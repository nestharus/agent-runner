"""Durable, notification-recipient-only fixture controls.

No State admission/receipt/custody writes. The sole SQL mutation is the selected
sidecar projection fault, installed before invoking the real source producer.
Observation hiding retains the unconsumed checkpoint; recovery exposes original
provider prompt bytes, never an invented receipt.
"""
import json
import os
import pathlib
import sqlite3
import time

CONTROL = pathlib.Path(__file__).with_name("recipient-control.json")
ACTIVE_REQUEST = None


def control():
    return json.loads(CONTROL.read_text()) if CONTROL.exists() else None


def record(kind, **values):
    c = control()
    if c is None:
        return
    event = dict(kind=kind, monotonic_ns=time.monotonic_ns(), **values)
    path = pathlib.Path(c["work_dir"]) / "recipient-events.jsonl"
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(fd, (json.dumps(event) + "\n").encode())
    finally:
        os.close(fd)


def configure(request, subcommand):
    global ACTIVE_REQUEST
    c = control()
    if c is None:
        return
    os.environ["S11_WORK_DIR"] = c["work_dir"]
    if subcommand != "launch":
        return
    ACTIVE_REQUEST = request
    nonce = request.get("params", {}).get("prompt_acceptance", {}).get("delivery_nonce")
    selected = c["env"] if nonce else {}
    os.environ.update(selected)
    record("launch", request=request, selected=selected, nonce=nonce,
           projection_fault=projection_present(c))


def projection_present(c):
    with sqlite3.connect("file:" + c["sidecar"] + "?mode=ro", uri=True) as db:
        return bool(db.execute("SELECT 1 FROM sqlite_master WHERE type='trigger' "
                               "AND name='s11_recipient_projection_fault'").fetchone())


def emitted(event):
    if ACTIVE_REQUEST is not None:
        record("emitted", request_id=ACTIVE_REQUEST["request_id"], event=event)


def admit_source(request, session):
    c = control()
    if c is None:
        return
    if c["projection"]:
        with sqlite3.connect(c["sidecar"]) as db:
            assert db.execute("SELECT COUNT(*) FROM mailbox_delivery_attempts").fetchone()[0] == 0
            db.executescript("""CREATE TRIGGER s11_recipient_projection_fault
                BEFORE UPDATE OF delivered_at ON mailbox
                WHEN NEW.delivered_at IS NOT NULL
                BEGIN SELECT RAISE(ABORT, 'forced recipient mailbox projection failure'); END;""")
        record("projection_installed", owner=json.loads(
            request["params"]["env"]["OULIPOLY_PARENT_INVOCATION"])["id"])
    record("before_ingress")
    from s11_ingress import admit
    admit(request, session, c)
    record("after_ingress")


def read_page(request, page):
    c = control()
    if c is None or request.get("params", {}).get("turn_projection") != "user_observation":
        return page
    hidden = c["negative"] and not pathlib.Path(c["work_dir"], "expose-observation").exists()
    if hidden:
        token = request["params"].get("after_token") or "s11-anchor:0"
        page["result"].update(turns=[], page_turn_count=0, source_bytes_examined=0,
                              resume_token=token, snapshot_id="s11-hidden:" + token)
    record("page", hidden=hidden, request=request, result=page["result"])
    return page
