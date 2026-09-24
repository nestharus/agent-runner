#!/usr/bin/python3
"""Offline receipt-page provider for the detached scan contract test."""
import json
import pathlib
import sys
import time
import traceback

# The client executes its pinned /proc/self/fd image, so __file__ is not the
# copied test pathname. Rust replaces this literal only in the private copy.
root = pathlib.Path(__FIXTURE_ROOT__)
def report_exception(kind, error, trace):
    (root / "provider-error").write_text("".join(traceback.format_exception(kind, error, trace)))
    sys.__excepthook__(kind, error, trace)
sys.excepthook = report_exception
request = json.load(sys.stdin)
operation = sys.argv[1]
with (root / "provider-calls").open("a") as calls:
    calls.write(operation + "\n")
if (root / "nonzero").exists() and operation == "session.read_turns":
    sys.exit(37)
if (root / "hold").exists() and operation == "session.read_turns":
    time.sleep(10)

contract = request["contract"]
def emit(value):
    print(json.dumps({"contract": contract, "request_id": request["request_id"],
                      "ok": True, "result": value}))

if operation == "describe":
    emit({"provider_id": "receipt-scan-fixture", "display_name": "Receipt Scan Fixture",
          "contract_versions": [contract], "preferred_contract": contract,
          "capabilities": {"launch": False, "policy": False, "quota": False,
                           "session": True, "session_turn_pages_v1": True,
                           "terminal": False, "rotation": False, "discovery": False,
                           "settings": False, "setup_brain": False, "setup": False,
                           "migration": False}})
elif operation == "session.read_turns":
    params = request["params"]
    session = params["session_id"]
    hashes = json.loads((root / "turn-hashes.json").read_text())
    selected = [] if params.get("start_mode") == "tail" else hashes
    turns = [{"session_id": session, "turn_id": "turn-" + str(i + 1),
              "snapshot_sequence": i, "timestamp": "2026-09-24T00:00:00Z",
              "role": "user", "parent_turn_id": None, "is_sidechain": False,
              "is_compaction_boundary": False, "body_state": "omitted_oversize",
              "body": None, "body_bytes": 1, "body_sha256": None,
              "canonical_text_sha256": digest}
             for i, digest in enumerate(selected)]
    emit({"read_protocol": "oulipoly.session_turn_pages/v1",
          "provider_instance_id": request["provider_instance_id"],
          "settings_id": params["settings_id"], "session_id": session,
          "turn_projection": params["turn_projection"], "snapshot_id": "scan-snapshot",
          "page_index": 0, "page_start_sequence": 0, "turns": turns,
          "page_turn_count": len(turns), "source_bytes_examined": 0,
          "scan_progress": False, "snapshot_complete": True,
          "next_page_token": None, "resume_token": "scan-after",
          "source_final": False, "warnings": []})
else:
    sys.exit(38)
