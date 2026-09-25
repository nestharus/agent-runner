#!/usr/bin/python3
"""Private adapter for the broker-launched interactive fixture's native store."""
import hashlib
import json
import os
import pathlib
import sys

request = json.load(sys.stdin)
method = sys.argv[1]
contract = "oulipoly.provider/v1"

def reply(result):
    print(json.dumps({"contract": contract, "request_id": request["request_id"],
                      "ok": True, "result": result}, separators=(",", ":")))

if method == "describe":
    reply({"provider_id": "age319-resident-native-fixture",
           "display_name": "AGE-319 resident fixture",
           "contract_versions": [contract], "preferred_contract": contract,
           "capabilities": {"launch": False, "policy": False, "quota": False,
                            "session": True, "session_turn_pages_v1":
                                not bool(os.environ.get("AGE319_PRIVATE_NATIVE_ADAPTER_UNSUPPORTED_V1")),
                            "terminal": False, "rotation": False,
                            "discovery": False, "settings": False,
                            "setup_brain": False, "setup": False, "migration": False}})
elif method == "session.read_turns":
    path = pathlib.Path(os.environ["AGE319_PRIVATE_NATIVE_STORE"])
    source = path.read_bytes()
    native = json.loads(source)
    params = request["params"]
    assert native["format"] == "age319-interactive-native-session/v1"
    assert params["session_id"] == native["session_id"]
    assert params["settings_id"] == "age319-resident-settings"
    assert params["turn_projection"] == "user_observation"
    tail = params["start_mode"] == "tail"
    assert tail or params["start_mode"] == "beginning"
    token_prefix = "age319-native:" + native["store_nonce"] + ":"
    after = params.get("after_token")
    assert tail or (isinstance(after, str) and after.startswith(token_prefix))
    offset = len(native["turns"]) if tail else int(after[len(token_prefix):])
    assert 0 <= offset <= len(native["turns"])
    turns = []
    for sequence, stored in enumerate(native["turns"][offset:]):
        body = stored["body"]
        chunks = [{"type": "text", "text": body}]
        encoded = json.dumps(chunks, separators=(",", ":"), ensure_ascii=False).encode()
        turns.append({"session_id": native["session_id"],
                      "turn_id": stored["turn_id"],
                      "snapshot_sequence": sequence,
                      "timestamp": "2026-09-25T00:00:00Z", "role": "user",
                      "parent_turn_id": None, "is_sidechain": False,
                      "is_compaction_boundary": False, "body_state": "inline",
                      "body": chunks, "body_bytes": len(encoded),
                      "body_sha256": hashlib.sha256(encoded).hexdigest(),
                      "canonical_text_sha256": hashlib.sha256(body.strip().encode()).hexdigest()})
    reply({"read_protocol": "oulipoly.session_turn_pages/v1",
           "provider_instance_id": request["provider_instance_id"],
           "settings_id": params["settings_id"],
           "session_id": native["session_id"],
           "turn_projection": params["turn_projection"],
           "snapshot_id": hashlib.sha256(source).hexdigest(),
           "page_index": 0, "page_start_sequence": 0, "turns": turns,
           "page_turn_count": len(turns), "source_bytes_examined": len(source),
           "scan_progress": False, "snapshot_complete": True,
           "next_page_token": None, "resume_token": token_prefix + str(len(native["turns"])),
           "source_final": False, "warnings": []})
else:
    print(json.dumps({"contract": contract, "request_id": request["request_id"],
                      "ok": False,
                      "error": {"category": "unsupported", "code": "unsupported_method",
                                "message": method, "retryable": False}}))
