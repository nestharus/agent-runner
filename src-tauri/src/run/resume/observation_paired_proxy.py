#!/usr/bin/python3
"""Offline test transport only; never dispatch native launch/model/session work."""
import json
import pathlib
import subprocess
import shutil
import sys

root = pathlib.Path(__FIXTURE_ROOT__)
binary = __PROVIDER_BINARY__
assert len(sys.argv) == 2 and sys.argv[1] in ("describe", "session.read_turns")
request = json.load(sys.stdin)
request["host"]["env"]["HOME"] = str(root / "home")
request["host"]["data_root"] = str(root / "data")
request["host"]["config_root"] = str(root / "config")
mode = (root / "mode").read_text()
if sys.argv[1] == "session.read_turns":
    if mode == "canonical_request":
        request["params"]["turn_projection"] = "canonical_ingest"
        request["params"].pop("expected_delivery_nonce", None)
    if mode == "wrong_nonce":
        request["params"]["expected_delivery_nonce"] = "b" * 64
    if mode == "wrong_account":
        other = root / "other-home/.codex/sessions"
        other.mkdir(parents=True, exist_ok=True)
        native = next((root / "home/.codex/sessions").rglob("*.jsonl"))
        shutil.copyfile(native, other / native.name)
        request["host"]["env"]["HOME"] = str(root / "other-home")
    if mode == "wrong_snapshot":
        request["params"]["snapshot_id"] = "wrong-opaque-snapshot"
output = subprocess.run(
    [binary, sys.argv[1]], input=json.dumps(request).encode(), capture_output=True,
    env={"HOME": str(root / "home"), "XDG_CONFIG_HOME": str(root / "config"),
         "XDG_DATA_HOME": str(root / "data"), "PATH": "/usr/bin:/bin", "TMPDIR": str(root)},
    timeout=5, check=False,
)
response = json.loads(output.stdout)
if sys.argv[1] == "session.read_turns" and response.get("ok"):
    result = response["result"]
    warnings = result["warnings"]
    # Neighbor mutations start with a real provider response, not a fabricated page.
    declaration = next(w for w in warnings if w.startswith("codex_observation_io_v1:")) if mode != "canonical_request" else ""
    other = [w for w in warnings if w != declaration]
    fields = dict(item.split("=") for item in declaration.split(":", 1)[1].split(";")) if declaration else {}
    if mode == "missing":
        result["warnings"] = other
    elif mode == "duplicate":
        warnings.append(declaration)
    elif mode in ("negative", "overflow", "sum_overflow", "forward_over", "reconstruction_limit", "malformed"):
        if mode == "negative": fields["reconstruction"] = "-1"
        if mode == "overflow": fields["reconstruction"] = "18446744073709551616"
        if mode == "sum_overflow": fields.update(forward="18446744073709551615", metadata="1")
        if mode == "forward_over": fields["forward"] = "513"
        if mode == "reconstruction_limit": fields["reconstruction"] = "8388608"
        if mode == "malformed": fields.pop("metadata")
        result["warnings"] = other + ["codex_observation_io_v1:" + ";".join(f"{k}={v}" for k,v in fields.items())]
    elif mode == "wrong_total": result["source_bytes_examined"] += 1
    elif mode == "wrong_projection": result["turn_projection"] = "canonical_ingest"
    elif mode == "wrong_session": result["session_id"] = "22222222-2222-4222-8222-222222222222"
    elif mode in ("capacity_error", "transient_error"):
        response = {"contract": response["contract"], "request_id": response["request_id"], "ok": False,
                    "error": {"category": "failed", "code": "session_turn_staging_capacity_exceeded" if mode == "capacity_error" else "offline_transient_error", "message": "synthetic offline observation failure", "retryable": False}}
    # Ordinary warnings must remain accepted alongside exactly one declaration.
    if mode == "normal": warnings.append("offline unrelated warning")
print(json.dumps(response, separators=(",", ":")))
