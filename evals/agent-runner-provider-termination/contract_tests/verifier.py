"""verifier.py — provider-termination eval contract verifier.

## Adapter declarations

```yaml
adapter_declarations:
  - component: evals/agent-runner-provider-termination/contract_tests/verifier.py
    role: adapter
    Translates:
      - age-139-terminal-signal-dto-contract
      - age-139-provider-vocabulary
      - oulipoly-terminal-signal-marker-contract
      - age-143-w5-reader-interface-contract
      - provider-termination-fixture-manifest-schema
```

(5 contracts under the N=5 threshold.)
"""

from __future__ import annotations

import json
import os
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

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
RUNTIME_MARKER_PREFIX = "OULIPOLY_TERMINAL_SIGNAL="

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

EXPECTED_NON_QUOTA_KINDS = {
    "SignalExit",
    "SpawnError",
    "ProlongedSilence",
    "CleanExit",
    "NonzeroExit",
    "Unknown",
}


class ContractFailure(Exception):
    pass


@dataclass(frozen=True)
class FixtureRef:
    field: str
    value: str
    kind: str


def fail(message: str) -> None:
    raise ContractFailure(message)


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def read_text_lossy(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def read_bytes(path: Path) -> bytes:
    return path.read_bytes()


def path_display(path: Path) -> str:
    return str(path.relative_to(REPO_ROOT))


def is_absolute_or_traversal(path: Path) -> bool:
    return path.is_absolute() or ".." in path.parts


def is_separator_row(cells: list[str]) -> bool:
    return all(re.fullmatch(r":?-{3,}:?", cell.replace(" ", "")) for cell in cells)


def is_table_row_line(line: str) -> bool:
    stripped = line.strip()
    return stripped.startswith("|") and stripped.endswith("|")


def row_provenance_text(row: dict[str, Any]) -> str:
    provenance = row["provenance"]
    return f"{provenance.get('source', '')} {provenance.get('notes', '')}"


def is_claude_provider_family(provider_family: str) -> bool:
    return provider_family == "claude"


def is_acr186_provenance_text(text: str) -> bool:
    return "acr-186" in text.lower()


def is_acr186_claude_row(row: dict[str, Any]) -> bool:
    return is_claude_provider_family(row["provider_family"]) and is_acr186_provenance_text(
        row_provenance_text(row)
    )


def validate_path_exists(path: Path, label: str) -> None:
    if not path.exists():
        fail(f"missing {label}: {path_display(path)}")


def validate_regular_file(path: Path, message: str) -> None:
    if not path.is_file():
        fail(message)


def validate_manifest_schema(data: Any, require_rows: bool) -> None:
    validate_manifest_root(data)
    validate_manifest_header(data)
    validate_manifest_adapter_surface(data)
    validate_manifest_rows_slot(data.get("rows"), require_rows)


def validate_manifest_root(data: Any) -> None:
    if not isinstance(data, dict):
        fail("manifest root must be a mapping")


def validate_manifest_header(data: dict[str, Any]) -> None:
    if data.get("schema_id") != "agent-runner-provider-termination-fixture-manifest-v1":
        fail("manifest schema_id is missing or incorrect")
    if data.get("schema_owner") != "evals/agent-runner-provider-termination/eval.md":
        fail("manifest schema_owner is missing or incorrect")


def validate_manifest_adapter_surface(data: dict[str, Any]) -> None:
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


def validate_manifest_rows_slot(rows: Any, require_rows: bool) -> None:
    if require_rows and (not isinstance(rows, list) or not rows):
        fail("manifest rows is empty; Step 6c must populate fixture row values")
    if rows is not None and not isinstance(rows, list):
        fail("manifest rows must be a list when populated")


def validate_manifest_rows(rows: Any) -> None:
    if not isinstance(rows, list):
        fail("manifest rows must be a list when populated")
    for index, row in enumerate(rows):
        validate_manifest_row_mapping(row, index)
        validate_row_shape(row)


def validate_manifest_row_mapping(row: Any, index: int) -> None:
    if not isinstance(row, dict):
        fail(f"manifest row {index} must be a mapping")


def validate_unique_row_ids(rows: list[dict[str, Any]]) -> None:
    seen: set[str] = set()
    for row in rows:
        row_id = row["id"]
        if row_id in seen:
            fail(f"duplicate manifest row id: {row_id}")
        seen.add(row_id)


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
    validate_row_identity(row)
    validate_row_fixture_fields(row)
    validate_row_expected_kind_fields(row)
    validate_row_status(row)
    validate_row_evidence_policy(row)
    validate_row_provenance(row)


def validate_row_identity(row: dict[str, Any]) -> None:
    if not isinstance(row["id"], str) or not row["id"]:
        fail("row id must be a non-empty string")
    if row["provider_family"] not in {"claude", "codex", "openai_compat"}:
        fail(f"row {row['id']} has invalid provider_family")


def validate_row_fixture_fields(row: dict[str, Any]) -> None:
    if row["fixture_bytes_role"] not in {"stdout", "stderr", "combined", "none"}:
        fail(f"row {row['id']} has invalid fixture_bytes_role")
    if row["fixture_bytes_path"] is not None and not isinstance(row["fixture_bytes_path"], str):
        fail(f"row {row['id']} fixture_bytes_path must be string or null")
    if row["sentinel_metadata_path"] is not None and not isinstance(row["sentinel_metadata_path"], str):
        fail(f"row {row['id']} sentinel_metadata_path must be string or null")


def validate_row_expected_kind_fields(row: dict[str, Any]) -> None:
    if row["expected_terminal_signal_kind"] not in SEVEN_DTO_KINDS:
        fail(f"row {row['id']} has invalid expected_terminal_signal_kind")
    expected_marker = MARKER_BY_DTO[row["expected_terminal_signal_kind"]]
    if row["expected_marker_kind_label"] != expected_marker:
        fail(f"row {row['id']} marker label must be {expected_marker}")


def validate_row_status(row: dict[str, Any]) -> None:
    status = row["terminal_status"]
    if not isinstance(status, dict):
        fail(f"row {row['id']} terminal_status must be a mapping")
    for key in ["kind", "code", "signal", "reason"]:
        if key not in status:
            fail(f"row {row['id']} terminal_status missing {key}")
    if status["kind"] not in {"exited", "signal_terminated", "spawn_error", "prolonged_silence", "unknown"}:
        fail(f"row {row['id']} has invalid terminal_status.kind")


def validate_row_evidence_policy(row: dict[str, Any]) -> None:
    policy = row["evidence_excerpt_policy"]
    if not isinstance(policy, dict):
        fail(f"row {row['id']} evidence_excerpt_policy must be a mapping")
    if policy.get("max_chars") != 160 or policy.get("opaque") is not True:
        fail(f"row {row['id']} evidence policy must be max_chars=160 and opaque=true")
    if policy.get("parsed_fields") != "TerminalSignalEvidence-only":
        fail(f"row {row['id']} parsed_fields must be TerminalSignalEvidence-only")


def validate_row_provenance(row: dict[str, Any]) -> None:
    provenance = row["provenance"]
    if not isinstance(provenance, dict):
        fail(f"row {row['id']} provenance must be a mapping")
    if not isinstance(provenance.get("source"), str):
        fail(f"row {row['id']} provenance.source must be a string")
    if not isinstance(provenance.get("privacy_reviewed"), bool):
        fail(f"row {row['id']} provenance.privacy_reviewed must be boolean")
    if not isinstance(provenance.get("notes"), str):
        fail(f"row {row['id']} provenance.notes must be a string")


def validate_relative_fixture_path(path: Path, row_id: str, field: str) -> None:
    if is_absolute_or_traversal(path):
        fail(f"row {row_id} {field} must be relative to fixtures/ and not traverse out")


def validate_required_fixture_bytes(row: dict[str, Any], require_bytes: bool) -> None:
    if require_bytes and row["fixture_bytes_path"] is None:
        fail(f"row {row['id']} must name fixture_bytes_path")


def validate_fixture_path(path: Path, message: str) -> None:
    validate_regular_file(path, message)


def validate_metadata(metadata: Any, row: dict[str, Any]) -> None:
    if not isinstance(metadata, dict):
        fail(f"row {row['id']} sentinel metadata root must be a mapping")
    if metadata.get("id") != row["id"]:
        fail(f"row {row['id']} sentinel metadata id does not match")


def validate_status_matches_expected(row: dict[str, Any]) -> None:
    status = row["terminal_status"]
    expected = row["expected_terminal_signal_kind"]
    if expected == "CleanExit" and (status["kind"] != "exited" or status["code"] != 0):
        fail(f"row {row['id']} CleanExit must use exited code 0")
    if expected == "NonzeroExit" and (
        status["kind"] != "exited" or not isinstance(status["code"], int) or status["code"] == 0
    ):
        fail(f"row {row['id']} NonzeroExit must use exited nonzero code")
    if expected == "SignalExit" and (status["kind"] != "signal_terminated" or not isinstance(status["signal"], int)):
        fail(f"row {row['id']} SignalExit must use signal_terminated with signal")
    if expected == "SpawnError" and (status["kind"] != "spawn_error" or not status["reason"]):
        fail(f"row {row['id']} SpawnError must use spawn_error with reason")
    if expected == "ProlongedSilence" and (status["kind"] != "prolonged_silence" or not status["reason"]):
        fail(f"row {row['id']} ProlongedSilence must use prolonged_silence with reason")
    if expected == "Unknown" and status["kind"] != "unknown":
        fail(f"row {row['id']} Unknown must use terminal_status.kind unknown")


def validate_no_missing_row_ids(missing: list[str]) -> None:
    if missing:
        fail(f"missing manifest row ids: {', '.join(missing)}")


def validate_quota_rows(rows: list[dict[str, Any]], provider_family: str) -> None:
    for row in rows:
        if row["provider_family"] != provider_family:
            fail(f"row {row['id']} must use provider_family {provider_family}")
        if row["expected_terminal_signal_kind"] != "QuotaExhaustedInband":
            fail(f"row {row['id']} must expect QuotaExhaustedInband")


def validate_acr186_provenance_text(text: str) -> None:
    if not is_acr186_provenance_text(text):
        fail("claude-quota-acr186 provenance must cite ACR-186")


def validate_acr186_privacy_reviewed(privacy_reviewed: Any) -> None:
    if privacy_reviewed is not True:
        fail("claude-quota-acr186 provenance must set privacy_reviewed true")


def validate_acr186_quota_rows(rows: list[dict[str, Any]]) -> None:
    for row in rows:
        provenance = row["provenance"]
        validate_acr186_provenance_text(row_provenance_text(row))
        validate_acr186_privacy_reviewed(provenance.get("privacy_reviewed"))


def validate_status_rows(
    rows: list[dict[str, Any]], provider_family: str, expected: dict[str, str]
) -> None:
    for row in rows:
        if row["provider_family"] != provider_family:
            fail(f"row {row['id']} must use provider_family {provider_family}")
        expected_kind = expected[row["id"]]
        if row["expected_terminal_signal_kind"] != expected_kind:
            fail(f"row {row['id']} must expect {expected_kind}")
        validate_status_matches_expected(row)


def validate_status_coverage(seen: set[str], provider_family: str) -> None:
    if seen != EXPECTED_NON_QUOTA_KINDS:
        fail(f"{provider_family} status matrix did not cover six non-quota kinds")


def validate_eval_section_present(body: str | None, heading: str) -> None:
    if body is None:
        fail(f"eval.md missing section: {heading}")


def validate_yaml_blocks_present(blocks: list[str], heading: str) -> None:
    if not blocks:
        fail(f"{heading} must contain a fenced YAML block")


def validate_mapping_yaml_candidate(candidates: list[dict[str, Any]], errors: list[str], heading: str) -> None:
    if not candidates:
        fail(f"{heading} YAML block did not parse as mapping: {'; '.join(errors)}")


def validate_table_minimum(rows: list[list[str]]) -> None:
    if len(rows) < 2:
        fail("dispatch table must include header and rows")


def validate_dispatch_header(header: list[str]) -> None:
    expected_header_names = ["provider_name", "recognizer_module_path", "terminal_signal_kind_set"]
    if header != expected_header_names:
        fail(f"dispatch table header must be {' | '.join(expected_header_names)}")


def validate_dispatch_row_count(data_rows: list[list[str]]) -> None:
    if len(data_rows) != len(EXPECTED_DISPATCH_ROWS):
        fail(f"dispatch table must have exactly {len(EXPECTED_DISPATCH_ROWS)} data rows")


def validate_dispatch_row_shapes(data_rows: list[list[str]]) -> None:
    for cells in data_rows:
        if len(cells) != 3:
            fail("dispatch table rows must have exactly 3 cells")


def validate_dispatch_kind_sets(rows: list[tuple[str, str, str]]) -> None:
    for provider, _, kind_set in rows:
        if kind_set != "seven kinds":
            fail(f"dispatch row {provider} kind set must equal 'seven kinds'")


def validate_dispatch_expected_rows(rows: list[tuple[str, str, str]]) -> None:
    if rows != EXPECTED_DISPATCH_ROWS:
        fail("dispatch table rows do not exactly match the Step 6a contract")


def validate_network_rows(rows: list[dict[str, Any]]) -> None:
    for row in rows:
        if row["expected_terminal_signal_kind"] not in {"Unknown", "NonzeroExit"}:
            fail(f"network row {row['id']} must expect Unknown or NonzeroExit")
        if row["expected_terminal_signal_kind"] == "NetworkError":
            fail(f"row {row['id']} must not expect NetworkError")


def validate_network_prose(claims: dict[str, bool]) -> None:
    if claims["mentions_network_error_enum"]:
        fail("eval.md must not introduce NetworkError as a TerminalSignalKind")
    if not claims["states_adjacent_diagnostics"]:
        fail("eval.md must state network_error is adjacent diagnostics")
    if not claims["states_not_terminal_signal_kind"]:
        fail("eval.md must state network_error is not a terminal-signal kind")


def validate_marker_literal(text: str) -> None:
    if "OULIPOLY_TERMINAL_SIGNAL <json-payload>" in text:
        fail("eval.md must not declare the obsolete space-separated terminal-signal marker")
    if "OULIPOLY_TERMINAL_SIGNAL=<json>" not in text:
        fail("eval.md must declare OULIPOLY_TERMINAL_SIGNAL=<json>")


def consume_marker_line(line: str) -> dict[str, Any]:
    validate_runtime_marker_prefix(line)
    payload = parse_runtime_marker_payload(marker_payload_text(line))
    validate_runtime_marker_payload(payload)
    return payload


def validate_runtime_marker_prefix(line: str) -> None:
    if not line.startswith(RUNTIME_MARKER_PREFIX):
        fail("runtime marker line must use OULIPOLY_TERMINAL_SIGNAL=<json>")


def marker_payload_text(line: str) -> str:
    return line[len(RUNTIME_MARKER_PREFIX) :]


def parse_runtime_marker_payload(payload_text: str) -> dict[str, Any]:
    try:
        payload = json.loads(payload_text)
    except json.JSONDecodeError as exc:
        fail(f"runtime marker payload must be JSON: {exc}")
    return payload


def validate_runtime_marker_payload(payload: Any) -> None:
    if not isinstance(payload, dict):
        fail("runtime marker payload must be a mapping")
    expected_keys = {"kind", "evidence", "invocation_id", "session_id"}
    if set(payload) != expected_keys:
        fail("runtime marker payload must contain exactly kind, evidence, invocation_id, session_id")
    if payload["kind"] not in SEVEN_DTO_KINDS:
        fail("runtime marker kind must use TerminalSignalKind serde labels")
    if not isinstance(payload["evidence"], dict):
        fail("runtime marker evidence must be a mapping")
    validate_uuid_like(payload["invocation_id"], "runtime marker invocation_id")
    if payload["session_id"] is not None:
        validate_uuid_like(payload["session_id"], "runtime marker session_id")


def validate_uuid_like(value: Any, label: str) -> None:
    if not isinstance(value, str):
        fail(f"{label} must be a string")
    if not re.fullmatch(
        r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
        value,
    ):
        fail(f"{label} must be UUID-shaped")


def marker_schema_kind(schema: dict[str, Any]) -> str:
    return str(schema.get("kind", ""))


def filter_missing_marker_labels(kind: str) -> list[str]:
    return [label for label in sorted(SEVEN_MARKER_LABELS) if label not in kind]


def validate_no_missing_marker_labels(missing: list[str]) -> None:
    if missing:
        fail(f"marker payload kind omits labels: {', '.join(missing)}")


def validate_schema_kind_labels(schema: dict[str, Any]) -> None:
    validate_no_missing_marker_labels(filter_missing_marker_labels(marker_schema_kind(schema)))


def validate_marker_schema(schema: dict[str, Any]) -> None:
    if schema.get("schema_id") != "agent-runner-terminal-signal-marker-v1":
        fail("marker payload schema_id is incorrect")
    validate_schema_kind_labels(schema)
    evidence = schema.get("evidence")
    if not isinstance(evidence, dict):
        fail("marker schema evidence must be a mapping")
    if evidence.get("excerpt_max_chars") != 160:
        fail("marker schema evidence.excerpt_max_chars must be 160")
    if evidence.get("opaque") is not True:
        fail("marker schema evidence.opaque must be true")


def validate_roundtrip_refs(rows: list[dict[str, Any]], refs_by_row: dict[str, list[str]]) -> None:
    for row in rows:
        if not refs_by_row[row["id"]] and row["fixture_bytes_role"] != "none":
            fail(f"row {row['id']} has no resolvable fixture refs but fixture_bytes_role is not none")


def validate_schema_ids(marker_schema: dict[str, Any], w5_schema: dict[str, Any]) -> None:
    if marker_schema.get("schema_id") != "agent-runner-terminal-signal-marker-v1":
        fail("marker payload schema_id is incorrect")
    if w5_schema.get("schema_id") != "agent-runner-provider-termination-w5-reader-v1":
        fail("W5 reader schema_id is incorrect")


def validate_acr186_rows_present(rows: list[dict[str, Any]]) -> None:
    if not rows:
        fail("no Claude provenance row cites ACR-186")


def validate_acr186_privacy_rows(rows: list[dict[str, Any]]) -> None:
    for row in rows:
        if row["provenance"].get("privacy_reviewed") is not True:
            fail(f"row {row['id']} cites ACR-186 but privacy_reviewed is not true")


def validate_fixture_excerpt_length(row: dict[str, Any], text: str) -> None:
    max_chars = row["evidence_excerpt_policy"]["max_chars"]
    if len(text) > max_chars:
        fail(f"row {row['id']} fixture excerpt length {len(text)} exceeds {max_chars}")


def validate_acr186_excerpt_bounds(rows: list[dict[str, Any]], texts_by_id: dict[str, str]) -> None:
    for row in rows:
        if row["id"] not in texts_by_id:
            continue
        validate_fixture_excerpt_length(row, texts_by_id[row["id"]])


def validate_yaml_list_block(block: str | None, key: str) -> None:
    if block is None:
        fail(f"missing {key}: list")


def validate_adapter_declarations(adapter: str, translated: list[str]) -> None:
    expected_translates = [
        "age-139-terminal-signal-dto-contract",
        "age-139-provider-recognizer-contract",
        "age-139-provider-vocabulary",
        "oulipoly-terminal-signal-marker-contract",
        "age-143-w5-reader-interface-contract",
    ]
    if translated != expected_translates:
        fail("Adapter declarations Translates list does not exactly match contract")
    if adapter.count("role: adapter") != 1:
        fail("Adapter declarations must contain exactly one role: adapter")


def validate_intrinsic_declarations(intrinsic: str, owns: list[str]) -> None:
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
    if intrinsic.count("Domain: provider-termination-fixture-corpus") != 1:
        fail("Intrinsic-surface declarations must contain exactly one provider-termination domain")
    if owns != expected_owns:
        fail("Intrinsic-surface Owns list does not exactly match contract")


def parse_yaml_text(text: str) -> Any:
    try:
        return yaml.safe_load(text)
    except Exception as exc:
        fail(f"YAML does not parse: {exc}")


def parse_json_text(text: str) -> Any:
    try:
        return json.loads(text)
    except Exception as exc:
        fail(f"JSON does not parse: {exc}")


def parse_relative_path(value: str) -> Path:
    return Path(value)


def parse_metadata_text(text: str, suffix: str) -> Any:
    if suffix == ".json":
        return parse_json_text(text)
    return parse_yaml_text(text)


def parse_section_body(text: str, heading: str) -> str | None:
    pattern = re.compile(
        rf"^#{{2,4}}\s+{re.escape(heading)}\s*$([\s\S]*?)(?=^#{{2,4}}\s+|\Z)",
        re.MULTILINE,
    )
    match = pattern.search(text)
    if not match:
        return None
    return match.group(1)


def parse_fenced_yaml_blocks(body: str) -> list[str]:
    return re.findall(r"```(?:yaml|yml)\s*\n([\s\S]*?)\n```", body)


def parse_yaml_block_value(block: str) -> Any:
    return yaml.safe_load(block)


def parse_yaml_block_values(blocks: list[str]) -> tuple[list[Any], list[Exception]]:
    values: list[Any] = []
    errors: list[Exception] = []
    for block in blocks:
        try:
            values.append(parse_yaml_block_value(block))
        except Exception as exc:
            errors.append(exc)
    return values, errors


def normalize_cell(cell: str) -> str:
    cell = re.sub(r"<br\s*/?>", " ", cell, flags=re.IGNORECASE)
    cell = re.sub(r"`", "", cell)
    cell = re.sub(r"\s+", " ", cell)
    return cell.strip()


def parse_table_line(line: str) -> list[str]:
    return [normalize_cell(cell) for cell in line.strip().strip("|").split("|")]


def parse_markdown_table_candidates(lines: list[str]) -> list[list[str]]:
    return [parse_table_line(line) for line in lines]


def parse_network_boundary_claims(text: str) -> dict[str, bool]:
    lowered = text.lower()
    return {
        "mentions_network_error_enum": "networkerror" in lowered,
        "states_adjacent_diagnostics": bool(re.search(r"network_error[\s\S]{0,120}adjacent diagnostics", lowered)),
        "states_not_terminal_signal_kind": bool(
            re.search(r"network_error[\s\S]{0,160}not (?:a |an )?terminal-signal kind", lowered)
        ),
    }


def parse_yaml_list_block_after_key(section: str, key: str) -> str | None:
    match = re.search(rf"^\s*{re.escape(key)}:\s*$([\s\S]*?)(?=^\S|\Z)", section, re.MULTILINE)
    if not match:
        return None
    return match.group(1)


def manifest_rows_value(data: dict[str, Any]) -> Any:
    return data.get("rows")


def first_mapping_candidate(candidates: list[dict[str, Any]]) -> dict[str, Any]:
    return candidates[0]


def table_data_rows(rows: list[list[str]]) -> list[list[str]]:
    return rows[1:]


def row_fixture_ref_values(row: dict[str, Any]) -> dict[str, str | None]:
    return {
        "fixture_bytes_path": row["fixture_bytes_path"],
        "sentinel_metadata_path": row["sentinel_metadata_path"],
    }


def filter_present_ref_values(values: dict[str, str | None]) -> dict[str, str]:
    return {key: value for key, value in values.items() if value is not None}


def filter_non_separator_rows(rows: list[list[str]]) -> list[list[str]]:
    return [cells for cells in rows if not is_separator_row(cells)]


def filter_mapping_values(values: list[Any]) -> list[dict[str, Any]]:
    return [value for value in values if isinstance(value, dict)]


def filter_missing_row_ids(row_ids: list[str], by_id: dict[str, dict[str, Any]]) -> list[str]:
    return [row_id for row_id in row_ids if row_id not in by_id]


def filter_acr186_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [row for row in rows if is_acr186_claude_row(row)]


def filter_acr186_quota_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [row for row in rows if row["id"] == "claude-quota-acr186"]


def filter_markdown_table_lines(section: str) -> list[str]:
    return [line for line in section.splitlines() if is_table_row_line(line)]


def filter_rows_with_fixture_bytes_path(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [row for row in rows if row["fixture_bytes_path"] is not None]


def filter_rows_with_sentinel_metadata_path(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [row for row in rows if row["sentinel_metadata_path"] is not None]


def filter_yaml_list_item_lines(block: str) -> list[str]:
    return [line for line in block.splitlines() if re.match(r"\s*-\s*(.+?)\s*$", line)]


def map_rows_by_id(rows: list[dict[str, Any]]) -> dict[str, dict[str, Any]]:
    return {row["id"]: row for row in rows}


def map_row_ids_to_rows(row_ids: list[str], by_id: dict[str, dict[str, Any]]) -> list[dict[str, Any]]:
    return [by_id[row_id] for row_id in row_ids]


def map_fixture_path(relative_path: Path) -> Path:
    return FIXTURE_ROOT / relative_path


def resolve_fixture_path(ref: FixtureRef, fixture_root: Path) -> Path:
    return fixture_root / Path(ref.value)


def map_ref_values_to_specs(values: dict[str, str]) -> list[FixtureRef]:
    kinds = {"fixture_bytes_path": "bytes", "sentinel_metadata_path": "metadata"}
    return [FixtureRef(field=field, value=value, kind=kinds[field]) for field, value in values.items()]


def classify_fixture_kind(ref: FixtureRef) -> str:
    kinds = {"fixture_bytes_path": "bytes", "sentinel_metadata_path": "metadata"}
    expected = kinds.get(ref.field)
    if expected is None:
        fail(f"unknown fixture ref field: {ref.field}")
    if ref.kind != expected:
        fail(f"fixture ref {ref.field} must use kind {expected}")
    return expected


def is_fixture_bytes_ref(kind: str) -> bool:
    return kind == "bytes"


def is_fixture_metadata_ref(kind: str) -> bool:
    return kind == "metadata"


def map_status_kinds(rows: list[dict[str, Any]], expected: dict[str, str]) -> set[str]:
    return {expected[row["id"]] for row in rows}


def map_dispatch_header(cells: list[str]) -> list[str]:
    return [cell.lower().replace(" ", "_").replace("-", "_") for cell in cells]


def map_dispatch_path(path: str) -> str:
    return path.replace("::Recognizer", "")


def map_dispatch_rows(data_rows: list[list[str]]) -> list[tuple[str, str, str]]:
    return [(cells[0], map_dispatch_path(cells[1]), cells[2]) for cells in data_rows]


def map_fixture_ref_counts(
    rows_with_bytes: list[dict[str, Any]], rows_with_metadata: list[dict[str, Any]]
) -> tuple[int, int]:
    return len(rows_with_bytes), len(rows_with_metadata)


def map_fixture_text_paths(rows: list[dict[str, Any]]) -> dict[str, Path]:
    return {
        row["id"]: fixture_path(row["fixture_bytes_path"], row["id"], "fixture_bytes_path")
        for row in rows
    }


def map_yaml_list_item(line: str) -> str:
    item_match = re.match(r"\s*-\s*(.+?)\s*$", line)
    if not item_match:
        return ""
    return item_match.group(1).strip().strip("`")


def map_yaml_list_items(lines: list[str]) -> list[str]:
    return [map_yaml_list_item(line) for line in lines]


def format_quota_result(provider_family: str, row_count: int, resolved_count: int) -> str:
    return f"{provider_family} quota rows={row_count} resolved_refs={resolved_count}"


def format_status_result(provider_family: str, seen: set[str]) -> str:
    return f"{provider_family} status matrix covers {', '.join(sorted(seen))}"


def format_dispatch_result() -> str:
    return "dispatch table has exactly 6 expected rows with seven kinds"


def format_network_result() -> str:
    return "network rows use Unknown/NonzeroExit and eval prose excludes NetworkError"


def format_marker_result() -> str:
    return "runtime marker literal consumes 4-key payload; eval internal schema remains enriched"


def format_roundtrip_result(row_count: int, bytes_count: int, metadata_count: int) -> str:
    return f"roundtrip checked rows={row_count} bytes_files={bytes_count} metadata_files={metadata_count}"


def format_schema_ids_result() -> str:
    return "manifest, marker, and W5 schema ids match contract"


def format_privacy_result(row_count: int) -> str:
    return f"ACR-186 Claude rows privacy-reviewed and <=160 chars: {row_count}"


def format_missing_fixture_path(row: dict[str, Any], ref: FixtureRef) -> str:
    return f"row {row['id']} {ref.kind} file is missing: {ref.value}"


def format_fixture_ref_result(path: Path) -> str:
    return str(path.relative_to(FIXTURE_ROOT))


def format_yaml_parse_error(err: Exception, context: str) -> str:
    if not context:
        return str(err)
    return f"{context}: {err}"


def format_coupling_result() -> str:
    return "coupling declarations carry 5 Translates entries and 8 Owns entries"


def format_contract_failure(exc: ContractFailure) -> str:
    return str(exc)


def format_missing_test_id() -> str:
    return "missing contract test id"


def format_unexpected_error(exc: Exception) -> str:
    return f"unexpected verifier error: {type(exc).__name__}: {exc}"


def load_manifest(require_rows: bool = True) -> dict[str, Any]:
    validate_path_exists(MANIFEST, "manifest")
    data = parse_yaml_text(read_text(MANIFEST))
    validate_manifest_schema(data, require_rows)
    return data


def manifest_rows() -> list[dict[str, Any]]:
    rows = manifest_rows_value(load_manifest(require_rows=True))
    validate_manifest_rows(rows)
    return rows


def rows_by_id() -> dict[str, dict[str, Any]]:
    rows = manifest_rows()
    validate_unique_row_ids(rows)
    return map_rows_by_id(rows)


def fixture_path(value: str, row_id: str, field: str) -> Path:
    path = parse_relative_path(value)
    validate_relative_fixture_path(path, row_id, field)
    return map_fixture_path(path)


def fixture_refs(row: dict[str, Any]) -> list[FixtureRef]:
    values = row_fixture_ref_values(row)
    present_values = filter_present_ref_values(values)
    return map_ref_values_to_specs(present_values)


def verify_metadata_ref(row: dict[str, Any], path: Path) -> None:
    metadata = parse_metadata_text(read_text(path), path.suffix)
    validate_metadata(metadata, row)


def resolve_fixture_ref(row: dict[str, Any], ref: FixtureRef) -> str:
    kind = classify_fixture_kind(ref)
    relative_path = parse_relative_path(ref.value)
    validate_relative_fixture_path(relative_path, row["id"], ref.field)
    path = resolve_fixture_path(ref, FIXTURE_ROOT)
    validate_fixture_path(path, format_missing_fixture_path(row, ref))
    if is_fixture_bytes_ref(kind):
        read_bytes(path)
    if is_fixture_metadata_ref(kind):
        verify_metadata_ref(row, path)
    return format_fixture_ref_result(path)


def resolve_fixture_refs(row: dict[str, Any], require_bytes: bool = False) -> list[str]:
    validate_required_fixture_bytes(row, require_bytes)
    return [resolve_fixture_ref(row, ref) for ref in fixture_refs(row)]


def resolve_refs_for_rows(rows: list[dict[str, Any]], require_bytes: bool = False) -> list[str]:
    resolved: list[str] = []
    for row in rows:
        resolved.extend(resolve_fixture_refs(row, require_bytes=require_bytes))
    return resolved


def require_rows(row_ids: list[str]) -> list[dict[str, Any]]:
    by_id = rows_by_id()
    missing = filter_missing_row_ids(row_ids, by_id)
    validate_no_missing_row_ids(missing)
    return map_row_ids_to_rows(row_ids, by_id)


def eval_text() -> str:
    validate_path_exists(EVAL_MD, "eval.md")
    return read_text(EVAL_MD)


def section_text(heading: str) -> str:
    body = parse_section_body(eval_text(), heading)
    validate_eval_section_present(body, heading)
    assert body is not None
    return body


def fenced_yaml_in_section(heading: str) -> dict[str, Any]:
    body = section_text(heading)
    blocks = parse_fenced_yaml_blocks(body)
    validate_yaml_blocks_present(blocks, heading)
    values, parse_errors = parse_yaml_block_values(blocks)
    errors = [format_yaml_parse_error(error, "") for error in parse_errors]
    candidates = filter_mapping_values(values)
    validate_mapping_yaml_candidate(candidates, errors, heading)
    return first_mapping_candidate(candidates)


def markdown_table_rows(section: str) -> list[list[str]]:
    table_lines = filter_markdown_table_lines(section)
    candidates = parse_markdown_table_candidates(table_lines)
    rows = filter_non_separator_rows(candidates)
    validate_table_minimum(rows)
    return rows


def parse_yaml_list_after_key(section: str, key: str) -> list[str]:
    block = parse_yaml_list_block_after_key(section, key)
    validate_yaml_list_block(block, key)
    assert block is not None
    lines = filter_yaml_list_item_lines(block)
    return map_yaml_list_items(lines)


def resolve_refs_by_row(rows: list[dict[str, Any]]) -> dict[str, list[str]]:
    return {row["id"]: resolve_fixture_refs(row) for row in rows}


def fixture_texts_by_row(paths_by_id: dict[str, Path]) -> dict[str, str]:
    return {row_id: read_text_lossy(path) for row_id, path in paths_by_id.items()}


def test_quota(provider_family: str) -> None:
    rows = require_rows(EXPECTED_QUOTA_ROWS[provider_family])
    validate_quota_rows(rows, provider_family)
    validate_acr186_quota_rows(filter_acr186_quota_rows(rows))
    resolved = resolve_refs_for_rows(rows, require_bytes=True)
    print(format_quota_result(provider_family, len(rows), len(resolved)))


def test_status_matrix(provider_family: str) -> None:
    expected = EXPECTED_STATUS_ROWS[provider_family]
    rows = require_rows(list(expected))
    validate_status_rows(rows, provider_family, expected)
    resolve_refs_for_rows(rows)
    seen = map_status_kinds(rows, expected)
    validate_status_coverage(seen, provider_family)
    print(format_status_result(provider_family, seen))


def test_dispatch_table() -> None:
    rows = markdown_table_rows(section_text("Provider-family dispatch table"))
    header = map_dispatch_header(rows[0])
    data_rows = table_data_rows(rows)
    validate_dispatch_header(header)
    validate_dispatch_row_count(data_rows)
    validate_dispatch_row_shapes(data_rows)
    normalized = map_dispatch_rows(data_rows)
    validate_dispatch_kind_sets(normalized)
    validate_dispatch_expected_rows(normalized)
    print(format_dispatch_result())


def test_network_boundary() -> None:
    rows = require_rows(list(EXPECTED_NETWORK_ROWS.values()))
    validate_network_rows(rows)
    resolve_refs_for_rows(rows)
    claims = parse_network_boundary_claims(eval_text())
    validate_network_prose(claims)
    print(format_network_result())


def test_marker_payload_schema() -> None:
    text = eval_text()
    validate_marker_literal(text)
    consume_marker_line(
        'OULIPOLY_TERMINAL_SIGNAL={"kind":"SignalExit","evidence":{},'
        '"invocation_id":"11111111-1111-4111-8111-111111111111","session_id":null}'
    )
    schema = fenced_yaml_in_section("Marker payload schema")
    validate_marker_schema(schema)
    print(format_marker_result())


def test_fixture_roundtrip() -> None:
    rows = manifest_rows()
    refs_by_row = resolve_refs_by_row(rows)
    validate_roundtrip_refs(rows, refs_by_row)
    rows_with_bytes = filter_rows_with_fixture_bytes_path(rows)
    rows_with_metadata = filter_rows_with_sentinel_metadata_path(rows)
    bytes_count, metadata_count = map_fixture_ref_counts(rows_with_bytes, rows_with_metadata)
    print(format_roundtrip_result(len(rows), bytes_count, metadata_count))


def test_schema_ids() -> None:
    load_manifest(require_rows=False)
    marker_schema = fenced_yaml_in_section("Marker payload schema")
    w5_schema = fenced_yaml_in_section("W5 reader interface schema")
    validate_schema_ids(marker_schema, w5_schema)
    print(format_schema_ids_result())


def test_privacy_bounds() -> None:
    acr_rows = filter_acr186_rows(manifest_rows())
    validate_acr186_rows_present(acr_rows)
    validate_acr186_privacy_rows(acr_rows)
    paths_by_id = map_fixture_text_paths(filter_rows_with_fixture_bytes_path(acr_rows))
    texts_by_id = fixture_texts_by_row(paths_by_id)
    validate_acr186_excerpt_bounds(acr_rows, texts_by_id)
    print(format_privacy_result(len(acr_rows)))


def test_coupling_declarations() -> None:
    adapter = section_text("Adapter declarations")
    intrinsic = section_text("Intrinsic-surface declarations")
    translated = parse_yaml_list_after_key(adapter, "Translates")
    owns = parse_yaml_list_after_key(intrinsic, "Owns")
    validate_adapter_declarations(adapter, translated)
    validate_intrinsic_declarations(intrinsic, owns)
    print(format_coupling_result())


TESTS: dict[str, Callable[[], None]] = {
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


def selected_test(test_id: str) -> Callable[[], None] | None:
    return TESTS.get(test_id)


def validate_test_exists(test: Callable[[], None] | None, test_id: str) -> None:
    if test is None:
        fail(f"unknown contract test id: {test_id}")


def test_from_id(test_id: str) -> Callable[[], None]:
    test = selected_test(test_id)
    validate_test_exists(test, test_id)
    assert test is not None
    return test


def main(argv: list[str]) -> int:
    try:
        test_id = argv[1]
        test_from_id(test_id)()
        return 0
    except ContractFailure as exc:
        print(format_contract_failure(exc))
        return 1
    except IndexError:
        print(format_missing_test_id())
        return 2
    except Exception as exc:
        print(format_unexpected_error(exc))
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
