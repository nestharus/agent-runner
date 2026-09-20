"""Actual owner acquires exact bytes; local acceptance must not settle async mail."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

binary, handle, prefix, runner, session = sys.argv[1:]


def capture(suffix, argv):
    result = subprocess.run(argv, capture_output=True)
    Path(prefix + suffix).write_bytes(result.stdout)
    Path(prefix + suffix + ".stderr").write_bytes(result.stderr)
    result.check_returncode()
    return json.loads(result.stdout)


acquired = capture("-snapshot.json", [binary, "snapshot", handle])
snapshot = acquired["snapshot"]
body = bytes.fromhex(acquired["output"])
assert body == b"nested-root-complete", acquired
assert acquired["status"].startswith("DONE rc=0"), acquired
assert snapshot["version"] == 1 and snapshot["handle"] == handle, acquired
assert snapshot["encoding"] == "hex" and snapshot["bytes"] == len(body), acquired
assert snapshot["sha256"] == hashlib.sha256(body).hexdigest(), acquired
receipt = capture("-receipt.json", [binary, "accept-output", handle, "--snapshot", json.dumps(snapshot)])
assert receipt["version"] == 1 and receipt["handle"] == handle, receipt
assert receipt["snapshot"] == snapshot and receipt["local_receipt"] == "durable", receipt
assert isinstance(receipt["receipt_updated"], bool), receipt
assert receipt["remote_ack"] == receipt["physical_drain"] == "unconfirmed", receipt
# Initial provider still owns this turn. No resumed native receipt exists yet.
listing = capture("-pending-after-receipt.json", [runner, "mailbox", "list", "--session-id", session, "--json"])
assert listing["session_id"] == session and listing["all"] is False, listing
rows = listing["rows"]
assert any(row["handle"] == handle and row["delivered_at"] is None for row in rows), rows
