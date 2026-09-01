#!/usr/bin/env python3
import json
import sys


request = json.load(sys.stdin)
contract = request.get("contract", "oulipoly.provider/v1")
request_id = request.get("request_id", "historical-test-fixture")

if len(sys.argv) > 1 and sys.argv[1] == "describe":
    result = {
        "provider_id": "historical-test-fixture",
        "display_name": "Historical Test Fixture",
        "contract_versions": [contract],
        "preferred_contract": contract,
        "capabilities": {
            "launch": False,
            "policy": False,
            "quota": False,
            "session": True,
            "session_enumerate": False,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        },
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
        "message": "historical fixture endpoint only supports describe",
    },
}))
raise SystemExit(1)
