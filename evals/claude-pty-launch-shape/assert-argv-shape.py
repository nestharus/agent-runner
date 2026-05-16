#!/usr/bin/env python3
"""Detect Claude PTY argv shapes that suppress MCP replacement tool calls."""

from __future__ import annotations

import argparse
import json
import shlex
import sys
from pathlib import Path
from typing import Any, Iterable


EVAL_ID = "claude-pty-launch-shape"
BAD_FLAG = "--tools"
ALLOWED_TOOL_FLAGS = {"--allowedTools", "--allowed-tools"}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", required=True, help="argv-shape fixture JSON to inspect")
    return parser.parse_args()


def decode_fixture_json(path: Path) -> Any:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def require_fixture_object(path: Path, value: Any) -> dict[str, Any]:
    if isinstance(value, dict):
        return value
    raise ValueError(f"{path} must contain a JSON object")


def command_tokens(provider: dict[str, Any]) -> list[str]:
    command = provider.get("command")
    if isinstance(command, str) and command.strip():
        return shlex.split(command)
    return ["claude"]


def render_typed_policy(provider: dict[str, Any]) -> list[str]:
    restrictions = provider.get("tool_restrictions")
    if not isinstance(restrictions, dict) or restrictions.get("kind") != "claude":
        return []

    claude = restrictions.get("claude")
    if not isinstance(claude, dict):
        return []

    rendered: list[str] = []
    disallowed_tools = list_string_values(claude.get("disallowed_tools"))
    allowed_tools = list_string_values(claude.get("allowed_tools"))
    if disallowed_tools:
        rendered.extend(["--disallowed-tools", ",".join(disallowed_tools)])
    if allowed_tools:
        rendered.extend(["--allowed-tools", ",".join(allowed_tools)])
    if claude.get("disable_slash_commands") is True:
        rendered.append("--disable-slash-commands")
    return rendered


def list_string_values(value: Any) -> list[str]:
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, str)]


def normalized_argv(value: Any) -> list[str] | None:
    if isinstance(value, list) and all(isinstance(item, str) for item in value):
        return list(value)
    return None


def argv_case(source: str, argv: list[str]) -> dict[str, Any]:
    return {"source": source, "argv": argv}


def rendered_argv_inputs(fixture: dict[str, Any]) -> list[tuple[str, list[str]]]:
    rendered = normalized_argv(fixture.get("rendered_argv"))
    if rendered is None:
        return []
    return [("rendered_argv", rendered)]


def provider_fixture(fixture: dict[str, Any]) -> dict[str, Any] | None:
    provider = fixture.get("provider")
    if isinstance(provider, dict):
        return provider
    return None


def provider_argv_inputs(provider: dict[str, Any] | None) -> list[tuple[str, list[str]]]:
    if provider is None:
        return []

    command = command_tokens(provider)
    interactive_args = normalized_argv(provider.get("interactive_args")) or []
    typed_policy = render_typed_policy(provider)
    inputs: list[tuple[str, list[str]]] = []
    if interactive_args:
        inputs.append(("interactive_args", command + interactive_args))
    if typed_policy:
        inputs.append(("typed_claude_tool_restrictions", command + interactive_args + typed_policy))
    return inputs


def equivalent_case_name(case: dict[str, Any]) -> str:
    name = case.get("name")
    if isinstance(name, str):
        return name
    return "equivalent"


def valid_equivalent_cases(fixture: dict[str, Any]) -> list[dict[str, Any]]:
    equivalent_cases = fixture.get("equivalent_cases")
    if not isinstance(equivalent_cases, list):
        return []
    return [
        case
        for case in equivalent_cases
        if isinstance(case, dict) and normalized_argv(case.get("rendered_argv")) is not None
    ]


def equivalent_case_to_input(case: dict[str, Any]) -> tuple[str, list[str]]:
    return (
        f"equivalent_cases.{equivalent_case_name(case)}",
        list(case["rendered_argv"]),
    )


def equivalent_case_inputs(fixture: dict[str, Any]) -> list[tuple[str, list[str]]]:
    return [equivalent_case_to_input(case) for case in valid_equivalent_cases(fixture)]


def argv_case_records(inputs: Iterable[tuple[str, list[str]]]) -> list[dict[str, Any]]:
    return [argv_case(source, argv) for source, argv in inputs]


def argv_case_inputs(fixture: dict[str, Any]) -> list[tuple[str, list[str]]]:
    return [
        *rendered_argv_inputs(fixture),
        *provider_argv_inputs(provider_fixture(fixture)),
        *equivalent_case_inputs(fixture),
    ]


def argv_cases(fixture: dict[str, Any]) -> list[dict[str, Any]]:
    return argv_case_records(argv_case_inputs(fixture))


def split_flag_value(token: str) -> tuple[str, str] | None:
    if not token.startswith("--") or "=" not in token:
        return None
    flag, value = token.split("=", 1)
    return flag, value


def values_have_mcp_tool(values: Iterable[str]) -> bool:
    for value in values:
        for item in value.split(","):
            if item.strip().startswith("mcp__"):
                return True
    return False


def flag_value_pairs(argv: list[str]) -> list[dict[str, str]]:
    pairs: list[dict[str, str]] = []
    index = 0
    while index < len(argv):
        token = argv[index]
        split = split_flag_value(token)
        if split is not None:
            flag, value = split
            pairs.append({"flag": flag, "value": value, "form": "joined-equals"})
            index += 1
            continue

        if token == BAD_FLAG and index + 1 < len(argv):
            pairs.append({"flag": token, "value": argv[index + 1], "form": "split-token"})
            index += 2
            continue

        if token in ALLOWED_TOOL_FLAGS:
            index += 2
            continue
        index += 1
    return pairs


def invalid_tool_pairs(pairs: Iterable[dict[str, str]]) -> list[dict[str, str]]:
    return [pair for pair in pairs if pair["flag"] == BAD_FLAG and values_have_mcp_tool([pair["value"]])]


def violation_record(pair: dict[str, str]) -> dict[str, Any]:
    return {"flag": pair["flag"], "value": pair["value"], "form": pair["form"]}


def violation_records(pairs: Iterable[dict[str, str]]) -> list[dict[str, Any]]:
    return [violation_record(pair) for pair in pairs]


def bad_tools_mcp_pairs(argv: list[str]) -> list[dict[str, Any]]:
    return violation_records(invalid_tool_pairs(flag_value_pairs(argv)))


def enrich_violation(case: dict[str, Any], violation: dict[str, Any]) -> dict[str, Any]:
    return {
        "source": case["source"],
        "argv": case["argv"],
        **violation,
    }


def case_violation_records(case: dict[str, Any]) -> list[dict[str, Any]]:
    return [enrich_violation(case, violation) for violation in bad_tools_mcp_pairs(case["argv"])]


def fixture_violation_cases(fixture: dict[str, Any]) -> list[dict[str, Any]]:
    violation_cases: list[dict[str, Any]] = []
    for case in argv_cases(fixture):
        violation_cases.extend(case_violation_records(case))
    return violation_cases


def finding(path: Path, fixture: dict[str, Any], violation_cases: list[dict[str, Any]]) -> dict[str, Any]:
    sources = sorted({str(case["source"]) for case in violation_cases})
    return {
        "eval_id": EVAL_ID,
        "severity": "HIGH",
        "evidence_paths": [str(path.resolve())],
        "summary": "Claude proxy PTY argv contains --tools paired with mcp__ replacement tools.",
        "suggested_action": "Use --allowedTools mcp__... for the PTY allowlist, use the accepted --allowed-tools family only where supported, or use no tool filter.",
        "confidence": 0.98,
        "case_id": fixture.get("case_id"),
        "argv_source": fixture.get("argv_source"),
        "detected_sources": sources,
        "violations": violation_cases,
    }


def emit_finding(path: Path, fixture: dict[str, Any], violation_cases: list[dict[str, Any]]) -> None:
    print(json.dumps(finding(path, fixture, violation_cases), sort_keys=True))


def main() -> int:
    args = parse_args()
    fixture_path = Path(args.fixture)
    fixture = require_fixture_object(fixture_path, decode_fixture_json(fixture_path))
    violation_cases = fixture_violation_cases(fixture)

    if violation_cases:
        emit_finding(fixture_path, fixture, violation_cases)
        return 1
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(
            json.dumps(
                {
                    "eval_id": EVAL_ID,
                    "severity": "HIGH",
                    "evidence_paths": [],
                    "summary": f"argv-shape guard failed before evaluating fixture: {exc}",
                    "suggested_action": "Fix the eval fixture or guard invocation so the Claude argv can be inspected.",
                    "confidence": 0.5,
                },
                sort_keys=True,
            )
        )
        raise SystemExit(2)
