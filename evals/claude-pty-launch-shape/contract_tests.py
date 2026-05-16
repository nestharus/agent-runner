#!/usr/bin/env python3
"""Contract tests for the AGE-113 Claude PTY launch-shape eval surface."""

from __future__ import annotations

import json
import re
import subprocess
import sys
import unittest
from pathlib import Path
from typing import Any


EVAL_DIR = Path(__file__).resolve().parent
REPO_ROOT = EVAL_DIR.parents[1]
FIXTURES = EVAL_DIR / "fixtures"
ARGV_FIXTURES = FIXTURES / "argv-shape"
EVAL_SPEC = EVAL_DIR / "eval.md"
EVAL_HARNESS = EVAL_DIR / "eval.sh"
ARGV_GUARD = EVAL_DIR / "assert-argv-shape.py"
CARRY_FORWARD = EVAL_DIR / "CARRY_FORWARD.md"


def run_command(args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=REPO_ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def path_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def lossy_path_text(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def json_values(text: str) -> list[Any]:
    values: list[Any] = []
    decoder = json.JSONDecoder()
    remaining = text.strip()
    while remaining:
        try:
            value, idx = decoder.raw_decode(remaining)
        except json.JSONDecodeError:
            lines = remaining.splitlines()
            parsed_line = False
            for line in lines:
                line = line.strip()
                if not line or not line.startswith(("{", "[")):
                    continue
                try:
                    values.append(json.loads(line))
                    parsed_line = True
                except json.JSONDecodeError:
                    continue
            if parsed_line:
                return values
            raise
        values.append(value)
        remaining = remaining[idx:].strip()
    return values


def first_json_object(values: list[Any], text: str) -> dict[str, Any]:
    for value in values:
        if isinstance(value, dict):
            return value
    raise AssertionError(f"expected JSON object in output, got: {text!r}")


def assert_minimum_finding(test: unittest.TestCase, finding: dict[str, Any]) -> None:
    for field in [
        "eval_id",
        "severity",
        "evidence_paths",
        "summary",
        "suggested_action",
        "confidence",
    ]:
        test.assertIn(field, finding)
    test.assertEqual(finding["eval_id"], "claude-pty-launch-shape")
    test.assertIsInstance(finding["evidence_paths"], list)
    test.assertTrue(finding["evidence_paths"])


def argv_fixture_path(fixture_name: str) -> Path:
    return ARGV_FIXTURES / fixture_name


def bad_fixture_probe(fixture_name: str) -> tuple[Path, subprocess.CompletedProcess[str], dict[str, Any]]:
    fixture = argv_fixture_path(fixture_name)
    result = run_command([sys.executable, str(ARGV_GUARD), "--fixture", str(fixture)])
    finding = first_json_object(json_values(result.stdout), result.stdout)
    return fixture, result, finding


def dry_run_probe(mode_cell: str) -> tuple[subprocess.CompletedProcess[str], dict[str, Any]]:
    result = run_command(["bash", str(EVAL_HARNESS), "--dry-run", "--json", "--mode", mode_cell])
    payload = first_json_object(json_values(result.stdout), result.stdout)
    return result, payload


def matrix_probe() -> tuple[subprocess.CompletedProcess[str], dict[str, Any]]:
    result = run_command(["bash", str(EVAL_HARNESS), "--dry-run", "--json", "--matrix"])
    payload = first_json_object(json_values(result.stdout), result.stdout)
    return result, payload


def parsed_json_text(text: str) -> Any:
    return json.loads(text)


def truth_table_by_cell(rows: list[dict[str, Any]]) -> dict[tuple[Any, Any], dict[str, Any]]:
    return {
        (row["mode"], row["column"]): {
            "mcp_server_initialized": row["mcp_server_initialized"],
            "tools_call_reached_server": row["tools_call_reached_server"],
            "tool_returned_to_claude": row["tool_returned_to_claude"],
        }
        for row in rows
    }


def accessor_get_eval_files() -> list[Path]:
    return list(EVAL_DIR.rglob("*"))


def filter_non_pycache(files: list[Path]) -> list[Path]:
    return [
        path
        for path in files
        if path.is_file() and "__pycache__" not in path.parts and path.suffix != ".pyc"
    ]


def eval_source_files() -> list[Path]:
    return filter_non_pycache(accessor_get_eval_files())


def paths_with_marker(paths: list[Path], marker: str) -> list[Path]:
    return [path for path in paths if marker in lossy_path_text(path)]


def relative_eval_paths(paths: list[Path]) -> list[str]:
    return [str(path.relative_to(EVAL_DIR)) for path in paths]


def carry_forward_rows(text: str) -> list[dict[str, str]]:
    rows = []
    for line in text.splitlines():
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) != 3 or cells[0] in {"Inherited path", "---"}:
            continue
        rows.append({"inherited": cells[0], "successor": cells[1], "verdict": cells[2]})
    return rows


def carry_forward_by_inherited(rows: list[dict[str, str]]) -> dict[str, dict[str, str]]:
    return {row["inherited"]: row for row in rows}


def carry_forward_manifest() -> dict[str, dict[str, str]]:
    return carry_forward_by_inherited(carry_forward_rows(path_text(CARRY_FORWARD)))


def uncommented_shell_lines(script: str) -> list[str]:
    return [line for line in script.splitlines() if line.strip() and not line.lstrip().startswith("#")]


def shell_line_without_redirects(line: str) -> str:
    return (
        line.replace("&&", "")
        .replace(">&", "")
        .replace("&>", "")
        .replace("2>&1", "")
        .replace("1>&2", "")
    )


def trap_shell_lines(lines: list[str]) -> list[str]:
    return [line for line in lines if re.search(r"\btrap\b", line)]


def background_shell_lines(lines: list[str]) -> list[str]:
    return [line for line in lines if "&" in shell_line_without_redirects(line)]


class ClaudePtyLaunchShapeContract(unittest.TestCase):
    def test_t1_eval_spec_names_behavior_contract(self) -> None:
        self.assertTrue(EVAL_SPEC.exists(), f"missing {EVAL_SPEC}")
        text = EVAL_SPEC.read_text(encoding="utf-8")
        lowered = text.lower()
        required_lower_fragments = [
            "eval-id: claude-pty-launch-shape",
            "claude proxy",
            "pty",
            "--tools mcp__",
            "unwanted behavior",
            "positive evidence",
            "non-fire",
            "required trace fields",
            "suggested action",
            "write",
        ]
        for fragment in required_lower_fragments:
            self.assertIn(fragment, lowered)
        self.assertIn("--allowedTools", text)
        self.assertIn("AGE-105", text, "T1: missing AGE-105 cross-link")
        self.assertIn("AGE-107", text, "T1: missing AGE-107 cross-link")
        self.assertIn("AGE-108", text, "T1: missing AGE-108 cross-link")
        for field in [
            "eval_id",
            "severity",
            "evidence_paths",
            "summary",
            "suggested_action",
            "confidence",
        ]:
            self.assertIn(field, text)

    def assert_bad_fixture_reports_high(self, fixture_name: str) -> dict[str, Any]:
        fixture, result, finding = bad_fixture_probe(fixture_name)
        self.assertTrue(fixture.exists(), f"missing {fixture}")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        assert_minimum_finding(self, finding)
        self.assertEqual(finding["severity"], "HIGH")
        self.assertIn("--tools", finding["summary"])
        self.assertIn("mcp__", finding["summary"])
        evidence = "\n".join(str(path) for path in finding["evidence_paths"])
        self.assertIn(fixture_name, evidence)
        action = str(finding["suggested_action"])
        self.assertTrue("--allowedTools" in action or "no tool filter" in action)
        return finding

    def test_t2_source_guard_rejects_typed_tools_mcp_shape(self) -> None:
        self.assert_bad_fixture_reports_high("bad-typed.json")

    def test_t3_source_guard_rejects_raw_interactive_args_shape(self) -> None:
        finding = self.assert_bad_fixture_reports_high("bad-raw-interactive-args.json")
        self.assertIn("interactive_args", json.dumps(finding))

    def test_t4_source_guard_accepts_allowedtools_family_and_no_filter(self) -> None:
        for fixture_name in [
            "good-allowedtools-camel.json",
            "good-allowedtools-kebab.json",
            "good-no-filter.json",
        ]:
            with self.subTest(fixture=fixture_name):
                fixture = ARGV_FIXTURES / fixture_name
                self.assertTrue(fixture.exists(), f"missing {fixture}")
                result = run_command([sys.executable, str(ARGV_GUARD), "--fixture", str(fixture)])
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                combined = result.stdout + "\n" + result.stderr
                self.assertNotIn('"severity": "HIGH"', combined)
                self.assertNotIn('"severity":"HIGH"', combined)

    def dry_run_mode(self, mode_cell: str) -> tuple[subprocess.CompletedProcess[str], dict[str, Any]]:
        return dry_run_probe(mode_cell)

    def assert_dry_run_payload(
        self, result: subprocess.CompletedProcess[str], payload: dict[str, Any], mode_cell: str
    ) -> None:
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(payload.get("eval_id"), "claude-pty-launch-shape")
        self.assertEqual(payload.get("mode_cell"), mode_cell)
        self.assertIn("expected", payload)

    def test_t5_no_filter_pty_positive_contract(self) -> None:
        result, payload = self.dry_run_mode("M3-C3")
        self.assert_dry_run_payload(result, payload, "M3-C3")
        expected = payload["expected"]
        self.assertTrue(expected["mcp_server_initialized"])
        self.assertTrue(expected["tools_call_reached_server"])
        self.assertTrue(expected["tool_returned_to_claude"])

    def test_t6_allowedtools_pty_positive_contract(self) -> None:
        result, payload = self.dry_run_mode("M3-C2")
        self.assert_dry_run_payload(result, payload, "M3-C2")
        expected = payload["expected"]
        self.assertTrue(expected["mcp_server_initialized"])
        self.assertTrue(expected["tools_call_reached_server"])
        self.assertTrue(expected["tool_returned_to_claude"])
        self.assertIn("--allowedTools", json.dumps(payload))

    def test_t7_tools_mcp_pty_negative_contract(self) -> None:
        result, payload = self.dry_run_mode("M3-C1")
        self.assert_dry_run_payload(result, payload, "M3-C1")
        expected = payload["expected"]
        self.assertTrue(expected["mcp_server_initialized"])
        self.assertFalse(expected["tools_call_reached_server"])
        self.assertFalse(expected["tool_returned_to_claude"])
        self.assertIn("--tools", json.dumps(payload))

    def test_t8_full_truth_table_baseline_contract(self) -> None:
        baseline_path = FIXTURES / "truth-table-baseline.json"
        baseline = parsed_json_text(path_text(baseline_path))
        result, payload = matrix_probe()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(payload.get("claude_code_baseline"), baseline["claude_code_baseline"])
        self.assertEqual(truth_table_by_cell(payload["matrix"]), truth_table_by_cell(baseline["matrix"]))

    def test_t9_no_pending_markers_and_inherited_paths_mapped(self) -> None:
        marker = "prototype-" + "pending:"
        self.assertEqual(relative_eval_paths(paths_with_marker(eval_source_files(), marker)), [])
        self.assertTrue(CARRY_FORWARD.exists(), f"missing {CARRY_FORWARD}")
        mapped = carry_forward_manifest()
        self.assertEqual(
            mapped["prototype-tests/age-104-pty-mcp-gap/p2-proof-tests.sh"]["successor"],
            "evals/claude-pty-launch-shape/contract_tests.py + fixtures/truth-table-baseline.json",
        )
        self.assertEqual(
            mapped["prototype-tests/age-104-pty-mcp-gap/p2-truth-table-harness/run-all.sh"]["successor"],
            "evals/claude-pty-launch-shape/fixtures/truth-table-baseline.json",
        )
        self.assertEqual(
            mapped["prototype-tests/age-104-pty-mcp-gap/p2-truth-table-harness/server.py"]["successor"],
            "evals/claude-pty-launch-shape/fixtures/server.py",
        )
        self.assertEqual({row["verdict"] for row in mapped.values()}, {"superseded"})

    def test_t10_eval_harness_cleanup_contract(self) -> None:
        self.assertTrue(EVAL_HARNESS.exists(), f"missing {EVAL_HARNESS}")
        script = path_text(EVAL_HARNESS)
        uncommented_lines = uncommented_shell_lines(script)
        uncommented = "\n".join(uncommented_lines)
        trap_lines = trap_shell_lines(uncommented_lines)
        self.assertTrue(trap_lines, "eval.sh must install cleanup traps")
        joined_traps = "\n".join(trap_lines)
        for signal in ["EXIT", "INT", "TERM"]:
            self.assertIn(signal, joined_traps)
        self.assertIn("cleanup", uncommented)
        self.assertNotIn("^WROTE:", script)
        self.assertNotIn("WROTE:", script)
        self.assertNotRegex(script, r"\bTIMEOUT_S\b|wall-timeout|sleep\s+30|timeout\s+180")

        background_lines = background_shell_lines(uncommented_lines)
        if background_lines:
            self.assertRegex(uncommented, r"(pids|child_pids|bg_pids)")
            self.assertRegex(uncommented, r"\bkill\b")
            self.assertRegex(uncommented, r"\bwait\b")


if __name__ == "__main__":
    unittest.main(verbosity=2)
