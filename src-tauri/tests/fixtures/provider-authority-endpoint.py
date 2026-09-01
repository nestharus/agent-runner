#!/usr/bin/env python3
import json
import sys


try:
    request = json.load(sys.stdin)
except json.JSONDecodeError:
    request = {}
contract = request.get("contract", "oulipoly.provider/v1")
request_id = request.get("request_id", "historical-test-fixture")

profile = "__OULIPOLY_FIXTURE_PROFILE__"
enabled = set() if profile.startswith("__") else set(profile)
capabilities = {
    "launch": "l" in enabled,
    "prompt_acceptance_v1": "l" in enabled,
    "launch_output_v1": "l" in enabled,
    "policy": "p" in enabled,
    "quota": "q" in enabled,
    "session": "s" in enabled,
    "session_turn_pages_v1": "s" in enabled,
    "session_enumerate": "e" in enabled,
    "terminal": "t" in enabled,
    "rotation": False,
    "discovery": False,
    "settings": False,
    "setup_brain": False,
    "setup": False,
    "migration": False,
}

if len(sys.argv) > 1 and sys.argv[1] == "describe":
    result = {
        "provider_id": "historical-test-fixture",
        "display_name": "Historical Test Fixture",
        "contract_versions": [contract],
        "preferred_contract": contract,
        "capabilities": capabilities,
    }
    print(json.dumps({
        "contract": contract,
        "request_id": request_id,
        "ok": True,
        "result": result,
    }))
    raise SystemExit(0)

print(json.dumps({
    "contract": contract,
    "request_id": request_id,
    "ok": False,
    "error": {
        "kind": "unsupported_operation",
        "message": "historical fixture endpoint only implements describe",
    },
}))
raise SystemExit(1)
