#!/usr/bin/python3
"""Offline test transport only; never dispatch native launch/model/session work."""
import json
import pathlib
import subprocess
import shutil
import sys

root = pathlib.Path(__FIXTURE_ROOT__)
binary = __PROVIDER_BINARY__
# Bootstrap calls are read-only provider metadata operations, separate from
# the deliberately narrower page proxy. All provider executions remain offline
# under this fixture's empty environment and owned HOME/data/config roots.
def invoke(operation, request):
    return subprocess.run(
        [binary, operation], input=json.dumps(request).encode(), capture_output=True,
        env={"HOME": str(root / "home"), "XDG_CONFIG_HOME": str(root / "config"),
             "XDG_DATA_HOME": str(root / "data"), "PATH": "/usr/bin:/bin", "TMPDIR": str(root)},
        timeout=5, check=False,
    )

if sys.argv[1:] == ["--prepare-fixture"]:
    request = {"contract": "oulipoly.provider/v1", "request_id": "offline-profile",
               "host": {"app": "offline-fixture", "data_root": str(root / "data"),
                        "config_root": str(root / "config"), "env": {"HOME": str(root / "home")}},
               "params": {}}
    def metadata(operation):
        result = invoke(operation, request)
        assert result.returncode == 0, result.stderr
        response = json.loads(result.stdout)
        assert response["ok"], response
        return response["result"]
    family = metadata("describe")["provider_id"]
    account = metadata("discovery.accounts")["accounts"][0]["id"]
    # Fixture layout contract: native dot-account sessions and family-scoped
    # retained state. Reject path traversal before any fixture filesystem write.
    for name in (family, account):
        assert name and name not in (".", "..") and "/" not in name and "\\" not in name
    (root / "paired-profile.json").write_text(json.dumps({
        "family": family, "settings_id": account,
        "native_sessions": str(pathlib.Path("." + account) / "sessions"),
    }))
    sys.exit(0)

profile = json.loads((root / "paired-profile.json").read_text())
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
    if mode == "observation_request":
        request["params"]["turn_projection"] = "user_observation"
        request["params"]["expected_delivery_nonce"] = "a" * 64
    if mode == "wrong_nonce":
        request["params"]["expected_delivery_nonce"] = "b" * 64
    if mode == "wrong_account":
        other = root / "other-home" / profile["native_sessions"]
        other.mkdir(parents=True, exist_ok=True)
        native = next((root / "home" / profile["native_sessions"]).rglob("*.jsonl"))
        shutil.copyfile(native, other / native.name)
        request["host"]["env"]["HOME"] = str(root / "other-home")
    if mode == "wrong_snapshot":
        request["params"]["snapshot_id"] = "wrong-opaque-snapshot"
output = invoke(sys.argv[1], request)
response = json.loads(output.stdout)
if sys.argv[1] == "session.read_turns" and response.get("ok"):
    result = response["result"]
    warnings = result["warnings"]
    # Neighbor mutations start with a real provider response, not a fabricated page.
    # The actual request projection identifies observation, never the warning
    # or a proxy mutation-mode name. Canonical real-provider pages declare none.
    observation = request["params"]["turn_projection"] == "user_observation"
    declaration = next(w for w in warnings if w.startswith("codex_observation_io_v1:")) if observation else ""
    other = [w for w in warnings if w != declaration]
    fields = dict(item.split("=") for item in declaration.split(":", 1)[1].split(";")) if declaration else {}
    if mode == "missing":
        result["warnings"] = other
    elif mode == "duplicate":
        warnings.append(declaration)
    elif mode in ("negative", "fractional", "exponent", "signed", "overflow", "sum_overflow", "total_overflow", "forward_over", "reconstruction_limit", "malformed"):
        if mode == "negative": fields["reconstruction"] = "-1"
        if mode == "fractional": fields["reconstruction"] = "1.5"
        if mode == "exponent": fields["reconstruction"] = "1e0"
        if mode == "signed": fields["reconstruction"] = "+1"
        if mode == "total_overflow": fields.update(forward="18446744073709551615", metadata="0", reconstruction="1")
        if mode == "overflow": fields["reconstruction"] = "18446744073709551616"
        if mode == "sum_overflow": fields.update(forward="18446744073709551615", metadata="1")
        if mode == "forward_over": fields["forward"] = str(request["params"]["max_source_bytes"] + 1)
        if mode == "reconstruction_limit": fields["reconstruction"] = "8388608"
        if mode == "malformed": fields.pop("metadata")
        result["warnings"] = other + ["codex_observation_io_v1:" + ";".join(f"{k}={v}" for k,v in fields.items())]
    elif mode == "total_ceiling": result["source_bytes_examined"] = 16777216
    elif mode == "noninteger_total": result["source_bytes_examined"] = 1.5
    elif mode == "negative_total": result["source_bytes_examined"] = -1
    elif mode == "wrong_instance": result["provider_instance_id"] = "wrong-instance"
    elif mode == "wrong_settings": result["settings_id"] = "wrong-settings"
    elif mode == "wrong_total": result["source_bytes_examined"] += 1
    elif mode == "wrong_projection": result["turn_projection"] = "canonical_ingest"
    elif mode == "wrong_session": result["session_id"] = "22222222-2222-4222-8222-222222222222"
    elif mode in ("capacity_error", "transient_error"):
        response = {"contract": response["contract"], "request_id": response["request_id"], "ok": False,
                    "error": {"category": "failed", "code": "session_turn_staging_capacity_exceeded" if mode == "capacity_error" else "offline_transient_error", "message": "synthetic offline observation failure", "retryable": False}}
    # Ordinary warnings must remain accepted alongside exactly one declaration.
    if mode == "normal": warnings.append("offline unrelated warning")
encoded = json.dumps(response, separators=(",", ":"))
if sys.argv[1] == "session.read_turns":
    (root / "last-response.json").write_text(encoded + "\n")
print(encoded)
