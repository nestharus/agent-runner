#!/usr/bin/env bash
set -euo pipefail

contract_tests_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
eval_dir="$(cd "$contract_tests_dir/.." && pwd)"
repo_root="$(cd "$eval_dir/../.." && pwd)"

run_contract_test() {
  local test_id="$1"
  EVAL_DIR="$eval_dir" REPO_ROOT="$repo_root" TEST_ID="$test_id" python3 - <<'PY'
from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path
from typing import Any

try:
    import yaml
except Exception as exc:  # pragma: no cover - depends on host image
    print(f"PyYAML unavailable: {exc}")
    sys.exit(1)


EVAL_DIR = Path(os.environ["EVAL_DIR"])
REPO_ROOT = Path(os.environ["REPO_ROOT"])
FIXTURE_ROOT = EVAL_DIR / "fixtures"
MANIFEST = FIXTURE_ROOT / "MANIFEST.yaml"
EVAL_MD = EVAL_DIR / "eval.md"

SEVEN_DTO_KINDS = {
    "CleanExit",
    "NonzeroExit",
    "SignalExit",
    "SpawnError",
    "QuotaExhaustedInband",
    "ProlongedSilence",
    "Unknown",
}

SEVEN_MARKER_LABELS = {
    "clean_exit",
    "nonzero_exit",
    "signal_exit",
    "spawn_error",
    "quota_exhausted_inband",
    "prolonged_silence",
    "unknown",
}

MARKER_BY_DTO = {
    "CleanExit": "clean_exit",
    "NonzeroExit": "nonzero_exit",
    "SignalExit": "signal_exit",
    "SpawnError": "spawn_error",
    "QuotaExhaustedInband": "quota_exhausted_inband",
    "ProlongedSilence": "prolonged_silence",
    "Unknown": "unknown",
}

EXPECTED_STATUS_ROWS = {
    "claude": {
        "claude-signal": "SignalExit",
        "claude-spawn-error": "SpawnError",
        "claude-prolonged-silence": "ProlongedSilence",
        "claude-clean-exit": "CleanExit",
        "claude-nonzero": "NonzeroExit",
        "claude-unknown": "Unknown",
    },
    "codex": {
        "codex-signal": "SignalExit",
        "codex-spawn-error": "SpawnError",
        "codex-prolonged-silence": "ProlongedSilence",
        "codex-clean-exit": "CleanExit",
        "codex-nonzero": "NonzeroExit",
        "codex-unknown": "Unknown",
    },
    "openai_compat": {
        "openai-compat-signal": "SignalExit",
        "openai-compat-spawn-error": "SpawnError",
        "openai-compat-prolonged-silence": "ProlongedSilence",
        "openai-compat-clean-exit": "CleanExit",
        "openai-compat-nonzero": "NonzeroExit",
        "openai-compat-unknown": "Unknown",
    },
}

EXPECTED_QUOTA_ROWS = {
    "claude": ["claude-quota-acr186", "claude-quota-generic"],
    "codex": ["codex-quota"],
    "openai_compat": ["openai-compat-quota-gemini", "openai-compat-quota-opencode"],
}

EXPECTED_NETWORK_ROWS = {
    "claude": "claude-network-boundary",
    "codex": "codex-network-boundary",
    "openai_compat": "openai-compat-network-boundary",
}

EXPECTED_DISPATCH_ROWS = [
    ("claude", "crates/oulipoly-runtime/src/executor/providers/claude.rs", "seven kinds"),
    ("claude2", "crates/oulipoly-runtime/src/executor/providers/claude.rs", "seven kinds"),
    ("codex", "crates/oulipoly-runtime/src/executor/providers/codex.rs", "seven kinds"),
    ("gemini", "crates/oulipoly-runtime/src/executor/providers/openai_compat.rs", "seven kinds"),
    ("opencode", "crates/oulipoly-runtime/src/executor/providers/openai_compat.rs", "seven kinds"),
    (
        "other OpenAI-compatible wrappers",
        "crates/oulipoly-runtime/src/executor/providers/openai_compat.rs",
        "seven kinds",
    ),
]


class ContractFailure(Exception):
    pass


def fail(message: str) -> None:
    raise ContractFailure(message)


def load_manifest(require_rows: bool = True) -> dict[str, Any]:
    if not MANIFEST.exists():
        fail(f"missing manifest: {MANIFEST.relative_to(REPO_ROOT)}")
    try:
        data = yaml.safe_load(MANIFEST.read_text(encoding="utf-8"))
    except Exception as exc:
        fail(f"manifest YAML does not parse: {exc}")
    if not isinstance(data, dict):
        fail("manifest root must be a mapping")
    if data.get("schema_id") != "agent-runner-provider-termination-fixture-manifest-v1":
        fail("manifest schema_id is missing or incorrect")
    if data.get("schema_owner") != "evals/agent-runner-provider-termination/eval.md":
        fail("manifest schema_owner is missing or incorrect")
    surface = data.get("adapter_surface")
    if not isinstance(surface, dict):
        fail("manifest adapter_surface must be a mapping")
    expected_surface = {
        "raw_fixture_files": "provider bytes only",
        "sentinel_metadata_files": "minimal structured metadata only",
        "expected_rows": "machine-readable contract for W5",
    }
    for key, value in expected_surface.items():
        if surface.get(key) != value:
            fail(f"manifest adapter_surface.{key} must be {value!r}")
    rows = data.get("rows")
    if require_rows and (not isinstance(rows, list) or not rows):
        fail("manifest rows is empty; Step 6c must populate fixture row values")
    if rows is not None and not isinstance(rows, list):
        fail("manifest rows must be a list when populated")
    return data


def manifest_rows() -> list[dict[str, Any]]:
    rows = load_manifest(require_rows=True).get("rows")
    assert isinstance(rows, list)
    for index, row in enumerate(rows):
        if not isinstance(row, dict):
            fail(f"manifest row {index} must be a mapping")
        validate_row_shape(row)
    return rows


def rows_by_id() -> dict[str, dict[str, Any]]:
    rows = manifest_rows()
    by_id: dict[str, dict[str, Any]] = {}
    for row in rows:
        row_id = row["id"]
        if row_id in by_id:
            fail(f"duplicate manifest row id: {row_id}")
        by_id[row_id] = row
    return by_id


def validate_row_shape(row: dict[str, Any]) -> None:
    required = [
        "id",
        "provider_name",
        "provider_family",
        "recognizer_module_path",
        "fixture_bytes_path",
        "fixture_bytes_role",
        "sentinel_metadata_path",
        "terminal_status",
        "observed_at",
        "expected_terminal_signal_kind",
        "expected_marker_kind_label",
        "evidence_excerpt_policy",
        "provenance",
    ]
    for key in required:
        if key not in row:
            fail(f"row missing required field {key}: {row.get('id', '<unknown>')}")
    if not isinstance(row["id"], str) or not row["id"]:
        fail("row id must be a non-empty string")
    if row["provider_family"] not in {"claude", "codex", "openai_compat"}:
        fail(f"row {row['id']} has invalid provider_family")
    if row["fixture_bytes_role"] not in {"stdout", "stderr", "combined", "none"}:
        fail(f"row {row['id']} has invalid fixture_bytes_role")
    if row["fixture_bytes_path"] is not None and not isinstance(row["fixture_bytes_path"], str):
        fail(f"row {row['id']} fixture_bytes_path must be string or null")
    if row["sentinel_metadata_path"] is not None and not isinstance(row["sentinel_metadata_path"], str):
        fail(f"row {row['id']} sentinel_metadata_path must be string or null")
    if row["expected_terminal_signal_kind"] not in SEVEN_DTO_KINDS:
        fail(f"row {row['id']} has invalid expected_terminal_signal_kind")
    expected_marker = MARKER_BY_DTO[row["expected_terminal_signal_kind"]]
    if row["expected_marker_kind_label"] != expected_marker:
        fail(f"row {row['id']} marker label must be {expected_marker}")
    status = row["terminal_status"]
    if not isinstance(status, dict):
        fail(f"row {row['id']} terminal_status must be a mapping")
    for key in ["kind", "code", "signal", "reason"]:
        if key not in status:
            fail(f"row {row['id']} terminal_status missing {key}")
    if status["kind"] not in {"exited", "signal_terminated", "spawn_error", "prolonged_silence", "unknown"}:
        fail(f"row {row['id']} has invalid terminal_status.kind")
    policy = row["evidence_excerpt_policy"]
    if not isinstance(policy, dict):
        fail(f"row {row['id']} evidence_excerpt_policy must be a mapping")
    if policy.get("max_chars") != 160 or policy.get("opaque") is not True:
        fail(f"row {row['id']} evidence policy must be max_chars=160 and opaque=true")
    if policy.get("parsed_fields") != "TerminalSignalEvidence-only":
        fail(f"row {row['id']} parsed_fields must be TerminalSignalEvidence-only")
    provenance = row["provenance"]
    if not isinstance(provenance, dict):
        fail(f"row {row['id']} provenance must be a mapping")
    if not isinstance(provenance.get("source"), str):
        fail(f"row {row['id']} provenance.source must be a string")
    if not isinstance(provenance.get("privacy_reviewed"), bool):
        fail(f"row {row['id']} provenance.privacy_reviewed must be boolean")
    if not isinstance(provenance.get("notes"), str):
        fail(f"row {row['id']} provenance.notes must be a string")


def fixture_path(value: str, row_id: str, field: str) -> Path:
    path = Path(value)
    if path.is_absolute():
        fail(f"row {row_id} {field} must be relative to fixtures/")
    if ".." in path.parts:
        fail(f"row {row_id} {field} must not traverse out of fixtures/")
    return FIXTURE_ROOT / path


def resolve_fixture_refs(row: dict[str, Any], require_bytes: bool = False) -> list[str]:
    resolved: list[str] = []
    bytes_value = row["fixture_bytes_path"]
    if require_bytes and bytes_value is None:
        fail(f"row {row['id']} must name fixture_bytes_path")
    if bytes_value is not None:
        path = fixture_path(bytes_value, row["id"], "fixture_bytes_path")
        if not path.is_file():
            fail(f"row {row['id']} fixture bytes file is missing: {bytes_value}")
        path.read_bytes()
        resolved.append(bytes_value)
    metadata_value = row["sentinel_metadata_path"]
    if metadata_value is not None:
        path = fixture_path(metadata_value, row["id"], "sentinel_metadata_path")
        if not path.is_file():
            fail(f"row {row['id']} sentinel metadata file is missing: {metadata_value}")
        try:
            if path.suffix == ".json":
                metadata = json.loads(path.read_text(encoding="utf-8"))
            else:
                metadata = yaml.safe_load(path.read_text(encoding="utf-8"))
        except Exception as exc:
            fail(f"row {row['id']} sentinel metadata does not parse: {exc}")
        if not isinstance(metadata, dict):
            fail(f"row {row['id']} sentinel metadata root must be a mapping")
        if metadata.get("id") != row["id"]:
            fail(f"row {row['id']} sentinel metadata id does not match")
        resolved.append(metadata_value)
    return resolved


def assert_status_matches_expected(row: dict[str, Any]) -> None:
    status = row["terminal_status"]
    expected = row["expected_terminal_signal_kind"]
    if expected == "CleanExit":
        if status["kind"] != "exited" or status["code"] != 0:
            fail(f"row {row['id']} CleanExit must use exited code 0")
    elif expected == "NonzeroExit":
        if status["kind"] != "exited" or not isinstance(status["code"], int) or status["code"] == 0:
            fail(f"row {row['id']} NonzeroExit must use exited nonzero code")
    elif expected == "SignalExit":
        if status["kind"] != "signal_terminated" or not isinstance(status["signal"], int):
            fail(f"row {row['id']} SignalExit must use signal_terminated with signal")
    elif expected == "SpawnError":
        if status["kind"] != "spawn_error" or not status["reason"]:
            fail(f"row {row['id']} SpawnError must use spawn_error with reason")
    elif expected == "ProlongedSilence":
        if status["kind"] != "prolonged_silence" or not status["reason"]:
            fail(f"row {row['id']} ProlongedSilence must use prolonged_silence with reason")
    elif expected == "Unknown":
        if status["kind"] != "unknown":
            fail(f"row {row['id']} Unknown must use terminal_status.kind unknown")


def require_rows(row_ids: list[str]) -> list[dict[str, Any]]:
    by_id = rows_by_id()
    missing = [row_id for row_id in row_ids if row_id not in by_id]
    if missing:
        fail(f"missing manifest row ids: {', '.join(missing)}")
    return [by_id[row_id] for row_id in row_ids]


def test_quota(provider_family: str) -> None:
    rows = require_rows(EXPECTED_QUOTA_ROWS[provider_family])
    resolved_count = 0
    for row in rows:
        if row["provider_family"] != provider_family:
            fail(f"row {row['id']} must use provider_family {provider_family}")
        if row["expected_terminal_signal_kind"] != "QuotaExhaustedInband":
            fail(f"row {row['id']} must expect QuotaExhaustedInband")
        resolved_count += len(resolve_fixture_refs(row, require_bytes=True))
        if row["id"] == "claude-quota-acr186":
            provenance = row["provenance"]
            source_text = f"{provenance.get('source', '')} {provenance.get('notes', '')}"
            if "ACR-186" not in source_text and "acr-186" not in source_text.lower():
                fail("claude-quota-acr186 provenance must cite ACR-186")
            if provenance.get("privacy_reviewed") is not True:
                fail("claude-quota-acr186 provenance must set privacy_reviewed true")
    print(f"{provider_family} quota rows={len(rows)} resolved_refs={resolved_count}")


def test_status_matrix(provider_family: str) -> None:
    expected = EXPECTED_STATUS_ROWS[provider_family]
    rows = require_rows(list(expected))
    seen: set[str] = set()
    for row in rows:
        if row["provider_family"] != provider_family:
            fail(f"row {row['id']} must use provider_family {provider_family}")
        expected_kind = expected[row["id"]]
        if row["expected_terminal_signal_kind"] != expected_kind:
            fail(f"row {row['id']} must expect {expected_kind}")
        assert_status_matches_expected(row)
        resolve_fixture_refs(row)
        seen.add(expected_kind)
    if seen != {"SignalExit", "SpawnError", "ProlongedSilence", "CleanExit", "NonzeroExit", "Unknown"}:
        fail(f"{provider_family} status matrix did not cover six non-quota kinds")
    print(f"{provider_family} status matrix covers {', '.join(sorted(seen))}")


def eval_text() -> str:
    if not EVAL_MD.exists():
        fail(f"missing eval.md: {EVAL_MD.relative_to(REPO_ROOT)}")
    return EVAL_MD.read_text(encoding="utf-8")


def section_text(heading: str) -> str:
    text = eval_text()
    pattern = re.compile(
        rf"^#{{2,4}}\s+{re.escape(heading)}\s*$([\s\S]*?)(?=^#{{2,4}}\s+|\Z)",
        re.MULTILINE,
    )
    match = pattern.search(text)
    if not match:
        fail(f"eval.md missing section: {heading}")
    return match.group(1)


def fenced_yaml_in_section(heading: str) -> dict[str, Any]:
    body = section_text(heading)
    blocks = re.findall(r"```(?:yaml|yml)\s*\n([\s\S]*?)\n```", body)
    if not blocks:
        fail(f"{heading} must contain a fenced YAML block")
    errors = []
    for block in blocks:
        try:
            value = yaml.safe_load(block)
        except Exception as exc:
            errors.append(str(exc))
            continue
        if isinstance(value, dict):
            return value
    fail(f"{heading} YAML block did not parse as mapping: {'; '.join(errors)}")


def normalize_cell(cell: str) -> str:
    cell = re.sub(r"<br\s*/?>", " ", cell, flags=re.IGNORECASE)
    cell = re.sub(r"`", "", cell)
    cell = re.sub(r"\s+", " ", cell)
    return cell.strip()


def markdown_table_rows(section: str) -> list[list[str]]:
    rows: list[list[str]] = []
    for line in section.splitlines():
        stripped = line.strip()
        if not stripped.startswith("|") or not stripped.endswith("|"):
            continue
        cells = [normalize_cell(cell) for cell in stripped.strip("|").split("|")]
        if not cells or all(re.fullmatch(r":?-{3,}:?", cell.replace(" ", "")) for cell in cells):
            continue
        rows.append(cells)
    if len(rows) < 2:
        fail("dispatch table must include header and rows")
    return rows


def test_dispatch_table() -> None:
    rows = markdown_table_rows(section_text("Provider-family dispatch table"))
    header = [cell.lower().replace(" ", "_").replace("-", "_") for cell in rows[0]]
    expected_header_names = ["provider_name", "recognizer_module_path", "terminal_signal_kind_set"]
    if header != expected_header_names:
        fail(f"dispatch table header must be {' | '.join(expected_header_names)}")
    data_rows = rows[1:]
    if len(data_rows) != len(EXPECTED_DISPATCH_ROWS):
        fail(f"dispatch table must have exactly {len(EXPECTED_DISPATCH_ROWS)} data rows")
    normalized: list[tuple[str, str, str]] = []
    for cells in data_rows:
        if len(cells) != 3:
            fail("dispatch table rows must have exactly 3 cells")
        provider, path, kind_set = cells
        path = path.replace("::Recognizer", "")
        normalized.append((provider, path, kind_set))
        if kind_set != "seven kinds":
            fail(f"dispatch row {provider} kind set must equal 'seven kinds'")
        reference_path = Path(path)
        current_path = REPO_ROOT / reference_path
        age139_path = Path("/home/nes/projects/agent-runner/worktrees/age-139-terminal-signal-core") / reference_path
        if not current_path.exists() and not age139_path.exists():
            fail(f"dispatch row {provider} recognizer path does not resolve: {path}")
    if normalized != EXPECTED_DISPATCH_ROWS:
        fail("dispatch table rows do not exactly match the Step 6a contract")
    print("dispatch table has exactly 6 expected rows with seven kinds")


def test_network_boundary() -> None:
    rows = require_rows(list(EXPECTED_NETWORK_ROWS.values()))
    for row in rows:
        if row["expected_terminal_signal_kind"] not in {"Unknown", "NonzeroExit"}:
            fail(f"network row {row['id']} must expect Unknown or NonzeroExit")
        if row["expected_terminal_signal_kind"] == "NetworkError":
            fail(f"row {row['id']} must not expect NetworkError")
        resolve_fixture_refs(row)
    text = eval_text().lower()
    if "networkerror" in text:
        fail("eval.md must not introduce NetworkError as a TerminalSignalKind")
    if not re.search(r"network_error[\s\S]{0,120}adjacent diagnostics", text):
        fail("eval.md must state network_error is adjacent diagnostics")
    if not re.search(r"network_error[\s\S]{0,160}not (?:a |an )?terminal-signal kind", text):
        fail("eval.md must state network_error is not a terminal-signal kind")
    print("network rows use Unknown/NonzeroExit and eval prose excludes NetworkError")


def assert_schema_kind_labels(schema: dict[str, Any]) -> None:
    kind = str(schema.get("kind", ""))
    missing = [label for label in sorted(SEVEN_MARKER_LABELS) if label not in kind]
    if missing:
        fail(f"marker payload kind omits labels: {', '.join(missing)}")


def test_marker_payload_schema() -> None:
    text = eval_text()
    if "OULIPOLY_TERMINAL_SIGNAL <json-payload>" not in text:
        fail("eval.md must declare OULIPOLY_TERMINAL_SIGNAL <json-payload>")
    schema = fenced_yaml_in_section("Marker payload schema")
    if schema.get("schema_id") != "agent-runner-terminal-signal-marker-v1":
        fail("marker payload schema_id is incorrect")
    assert_schema_kind_labels(schema)
    evidence = schema.get("evidence")
    if not isinstance(evidence, dict):
        fail("marker schema evidence must be a mapping")
    if evidence.get("excerpt_max_chars") != 160:
        fail("marker schema evidence.excerpt_max_chars must be 160")
    if evidence.get("opaque") is not True:
        fail("marker schema evidence.opaque must be true")
    print("marker schema parses with seven labels and excerpt_max_chars=160")


def test_fixture_roundtrip() -> None:
    rows = manifest_rows()
    bytes_count = 0
    metadata_count = 0
    for row in rows:
        resolved = resolve_fixture_refs(row)
        if row["fixture_bytes_path"] is not None:
            bytes_count += 1
        if row["sentinel_metadata_path"] is not None:
            metadata_count += 1
        if not resolved and row["fixture_bytes_role"] != "none":
            fail(f"row {row['id']} has no resolvable fixture refs but fixture_bytes_role is not none")
    print(f"roundtrip checked rows={len(rows)} bytes_files={bytes_count} metadata_files={metadata_count}")


def test_schema_ids() -> None:
    load_manifest(require_rows=False)
    marker_schema = fenced_yaml_in_section("Marker payload schema")
    w5_schema = fenced_yaml_in_section("W5 reader interface schema")
    if marker_schema.get("schema_id") != "agent-runner-terminal-signal-marker-v1":
        fail("marker payload schema_id is incorrect")
    if w5_schema.get("schema_id") != "agent-runner-provider-termination-w5-reader-v1":
        fail("W5 reader schema_id is incorrect")
    print("manifest, marker, and W5 schema ids match contract")


def test_privacy_bounds() -> None:
    rows = manifest_rows()
    acr_rows = []
    for row in rows:
        provenance = row["provenance"]
        source_text = f"{provenance.get('source', '')} {provenance.get('notes', '')}"
        if row["provider_family"] == "claude" and "acr-186" in source_text.lower():
            acr_rows.append(row)
    if not acr_rows:
        fail("no Claude provenance row cites ACR-186")
    for row in acr_rows:
        if row["provenance"].get("privacy_reviewed") is not True:
            fail(f"row {row['id']} cites ACR-186 but privacy_reviewed is not true")
        max_chars = row["evidence_excerpt_policy"]["max_chars"]
        bytes_value = row["fixture_bytes_path"]
        if bytes_value is not None:
            text = fixture_path(bytes_value, row["id"], "fixture_bytes_path").read_text(
                encoding="utf-8", errors="replace"
            )
            if len(text) > max_chars:
                fail(f"row {row['id']} fixture excerpt length {len(text)} exceeds {max_chars}")
    print(f"ACR-186 Claude rows privacy-reviewed and <=160 chars: {len(acr_rows)}")


def parse_yaml_list_after_key(section: str, key: str) -> list[str]:
    match = re.search(rf"^\s*{re.escape(key)}:\s*$([\s\S]*?)(?=^\S|\Z)", section, re.MULTILINE)
    if not match:
        fail(f"missing {key}: list")
    items = []
    for line in match.group(1).splitlines():
        item_match = re.match(r"\s*-\s*(.+?)\s*$", line)
        if item_match:
            items.append(item_match.group(1).strip().strip("`"))
    return items


def test_coupling_declarations() -> None:
    adapter = section_text("Adapter declarations")
    intrinsic = section_text("Intrinsic-surface declarations")
    expected_translates = [
        "age-139-terminal-signal-dto-contract",
        "age-139-provider-recognizer-contract",
        "age-139-provider-vocabulary",
        "oulipoly-terminal-signal-marker-contract",
        "age-143-w5-reader-interface-contract",
    ]
    translated = parse_yaml_list_after_key(adapter, "Translates")
    if translated != expected_translates:
        fail("Adapter declarations Translates list does not exactly match contract")
    if adapter.count("role: adapter") != 1:
        fail("Adapter declarations must contain exactly one role: adapter")
    if intrinsic.count("Domain: provider-termination-fixture-corpus") != 1:
        fail("Intrinsic-surface declarations must contain exactly one provider-termination domain")
    expected_owns = [
        "MANIFEST.yaml",
        "fixture_bytes_path",
        "sentinel_metadata_path",
        "manifest-row-schema",
        "bounded-evidence-excerpt-policy",
        "fixture-provenance",
        "per-fixture-expected-terminal-signal-kind",
        "per-fixture-expected-marker-kind-label",
    ]
    owns = parse_yaml_list_after_key(intrinsic, "Owns")
    if owns != expected_owns:
        fail("Intrinsic-surface Owns list does not exactly match contract")
    print("coupling declarations carry 5 Translates entries and 8 Owns entries")


TESTS = {
    "t01": lambda: test_quota("claude"),
    "t02": lambda: test_status_matrix("claude"),
    "t03": lambda: test_quota("codex"),
    "t04": lambda: test_status_matrix("codex"),
    "t05": lambda: test_quota("openai_compat"),
    "t06": lambda: test_status_matrix("openai_compat"),
    "t07": test_dispatch_table,
    "t08": test_network_boundary,
    "t09": test_marker_payload_schema,
    "t10": test_fixture_roundtrip,
    "t11": test_schema_ids,
    "t12": test_privacy_bounds,
    "t13": test_coupling_declarations,
}

try:
    test_id = os.environ["TEST_ID"]
    TESTS[test_id]()
except ContractFailure as exc:
    print(str(exc))
    sys.exit(1)
except KeyError as exc:
    print(f"unknown contract test id: {exc}")
    sys.exit(2)
except Exception as exc:
    print(f"unexpected verifier error: {type(exc).__name__}: {exc}")
    sys.exit(1)
PY
}
