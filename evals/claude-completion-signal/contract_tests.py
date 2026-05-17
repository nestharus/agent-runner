#!/usr/bin/env python3
"""Contract tests for the AGE-105 Claude completion-signal eval."""

from __future__ import annotations

import importlib.util
import json
import re
import unittest
from pathlib import Path
from types import ModuleType
from typing import Any, Iterable


EVAL_ROOT = Path(__file__).resolve().parent
SCENARIO_ROOT = EVAL_ROOT / "fixtures" / "scenarios"
BASELINE_PATH = EVAL_ROOT / "fixtures" / "completion-baseline.json"
EVALUATOR_FILENAME = "evaluate-predicate.py"
EVALUATOR_PATH = EVAL_ROOT / EVALUATOR_FILENAME
EVAL_HARNESS_PATH = EVAL_ROOT / "eval.sh"
RUN_TESTS_PATH = EVAL_ROOT / "run-tests.sh"

SCENARIOS = [
    "clean_foreground",
    "foreground_tool",
    "one_background",
    "five_background",
    "scheduled_wakeup",
]

EVENT_NAMES = {
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "SubagentStop",
    "ScheduleWakeup",
    "SessionStart",
    "SessionEnd",
    "Notification",
}

FINALIZATION_REASONS = {
    "completion_predicate_satisfied",
    "checkpoint_pending_work",
}

FORBIDDEN_TIMEOUT_TERMS = [
    "tracing_timeout",
    "stale_running",
    "idle_deadline",
    "idle_timeout",
    "wall_timeout",
    "sleep_window",
]

TIMELINE_REQUIRED_FIELDS = {
    "seq",
    "event",
    "active_tool_after",
    "pending_background_agent_after",
    "pending_scheduled_wakeup_after",
    "predicate_after",
}

COUNTER_FIELDS = {
    "active_tool": "active_tool_after",
    "pending_background_agent": "pending_background_agent_after",
    "pending_scheduled_wakeup": "pending_scheduled_wakeup_after",
}

EXPECTED_FIXTURE_TRAJECTORIES = {
    "clean_foreground": {
        "active_tool": [0, 0],
        "pending_background_agent": [0, 0],
        "pending_scheduled_wakeup": [0, 0],
    },
    "foreground_tool": {
        "active_tool": [0, 1, 0],
        "pending_background_agent": [0, 0],
        "pending_scheduled_wakeup": [0, 0],
    },
    "one_background": {
        "active_tool": [0, 0],
        "pending_background_agent": [0, 1, 0],
        "pending_scheduled_wakeup": [0, 0],
    },
    "five_background": {
        "active_tool": [0, 0],
        "pending_background_agent": [0, 5, 4, 3, 2, 1, 0],
        "pending_scheduled_wakeup": [0, 0],
    },
    "scheduled_wakeup": {
        "active_tool": [0, 0],
        "pending_background_agent": [0, 0],
        "pending_scheduled_wakeup": [0, 1, 0],
    },
}

EXPECTED_FIRST_STOP_SNAPSHOTS = {
    "clean_foreground": {
        "active_tool_at_stop": 0,
        "pending_background_agent_at_stop": 0,
        "pending_scheduled_wakeup_at_stop": 0,
        "predicate": True,
    },
    "foreground_tool": {
        "active_tool_at_stop": 0,
        "pending_background_agent_at_stop": 0,
        "pending_scheduled_wakeup_at_stop": 0,
        "predicate": True,
    },
    "one_background": {
        "active_tool_at_stop": 0,
        "pending_background_agent_at_stop": 1,
        "pending_scheduled_wakeup_at_stop": 0,
        "predicate": False,
    },
    "five_background": {
        "active_tool_at_stop": 0,
        "pending_background_agent_at_stop": 5,
        "pending_scheduled_wakeup_at_stop": 0,
        "predicate": False,
    },
    "scheduled_wakeup": {
        "active_tool_at_stop": 0,
        "pending_background_agent_at_stop": 0,
        "pending_scheduled_wakeup_at_stop": 1,
        "predicate": False,
    },
}

EXACT_STOP_COUNTS = {
    "clean_foreground": 1,
    "foreground_tool": 1,
}

MINIMUM_STOP_COUNTS = {
    "one_background": 2,
    "five_background": 6,
    "scheduled_wakeup": 2,
}

SOURCE_PATHS = [
    EVALUATOR_PATH,
    EVAL_HARNESS_PATH,
    RUN_TESTS_PATH,
]
PROVENANCE_PATH = EVAL_ROOT / "provenance.json"
PROVENANCE_MANIFEST_PATH = (
    EVAL_ROOT.parents[1] / "evals" / "_provenance" / "age-89-dossier-manifest.json"
)


# role: mapper
def map_scenario_path(scenario_id: str) -> str:
    return str(SCENARIO_ROOT / f"{scenario_id}.json")


# role: accessor
def read_scenario_bytes(path: str) -> bytes:
    return Path(path).read_bytes()


# role: parser
def decode_scenario_json(raw: bytes) -> Any:
    return json.loads(raw.decode("utf-8"))


# role: validator
def validate_scenario_shape(payload: Any) -> dict[str, Any]:
    if not isinstance(payload, dict):
        raise AssertionError("scenario fixture must be a JSON object")
    for key in [
        "scenario_id",
        "description",
        "events",
        "expected_counter_trajectory",
        "expected_stop_snapshots",
        "expected_finalization",
    ]:
        if key not in payload:
            raise AssertionError(f"scenario fixture missing required key {key!r}")
    if not isinstance(payload["scenario_id"], str):
        raise AssertionError("scenario_id must be a string")
    if not isinstance(payload["description"], str):
        raise AssertionError("description must be a string")
    if not isinstance(payload["events"], list):
        raise AssertionError("events must be a list")
    if not isinstance(payload["expected_counter_trajectory"], dict):
        raise AssertionError("expected_counter_trajectory must be an object")
    if not isinstance(payload["expected_stop_snapshots"], list):
        raise AssertionError("expected_stop_snapshots must be a list")
    if not isinstance(payload["expected_finalization"], dict):
        raise AssertionError("expected_finalization must be an object")
    return payload


# role: orchestration
def load_scenario(path: str) -> dict[str, Any]:
    return validate_scenario_shape(decode_scenario_json(read_scenario_bytes(path)))


# role: accessor
def read_baseline_bytes(path: str) -> bytes:
    return Path(path).read_bytes()


# role: parser
def decode_baseline_json(raw: bytes) -> Any:
    return json.loads(raw.decode("utf-8"))


# role: validator
def validate_baseline_shape(payload: Any) -> dict[str, Any]:
    if not isinstance(payload, dict):
        raise AssertionError("completion baseline must be a JSON object")
    return payload


# role: orchestration
def load_baseline(path: str) -> dict[str, Any]:
    return validate_baseline_shape(decode_baseline_json(read_baseline_bytes(path)))


# role: accessor
def resolve_evaluator_path(eval_root: str) -> str:
    return str(Path(eval_root).resolve() / EVALUATOR_FILENAME)


# role: orchestration
def import_module_from_path(name: str, path: str) -> ModuleType:
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise ImportError(f"could not import evaluator from {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# role: validator
def validate_evaluator_module(module: ModuleType) -> ModuleType:
    if not hasattr(module, "evaluate"):
        raise AttributeError("evaluate-predicate.py must expose evaluate(scenario)")
    return module


# role: orchestration
def load_evaluator_module(eval_root: str) -> ModuleType:
    return validate_evaluator_module(
        import_module_from_path("evaluate_predicate", resolve_evaluator_path(eval_root))
    )


# role: orchestration
def evaluate_scenario_raw(module: ModuleType, scenario_dict: dict[str, Any]) -> Any:
    return module.evaluate(scenario_dict)


# role: validator
def validate_evaluation_result(result: Any) -> dict[str, Any]:
    if not isinstance(result, dict):
        raise AssertionError("evaluate(scenario) must return a JSON-like object")
    return result


# role: orchestration
def evaluate_scenario(eval_root: str, scenario_id: str) -> dict[str, Any]:
    module = load_evaluator_module(eval_root)
    scenario = load_scenario(map_scenario_path(scenario_id))
    return validate_evaluation_result(evaluate_scenario_raw(module, scenario))


# role: validator
def validate_timeline_shape(result: dict[str, Any]) -> list[dict[str, Any]]:
    timeline = result.get("timeline")
    if not isinstance(timeline, list):
        raise AssertionError("evaluator result must include a timeline list")
    rows: list[dict[str, Any]] = []
    for row in timeline:
        if not isinstance(row, dict):
            raise AssertionError("every timeline row must be a JSON object")
        rows.append(row)
    return rows


# role: filter
def select_stop_rows(timeline: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [row for row in timeline if row["event"] == "Stop"]


# role: accessor
def get_first_row(rows: list[dict[str, Any]]) -> dict[str, Any]:
    return rows[0]


# role: accessor
def get_row_at_position(rows: list[dict[str, Any]], idx: int) -> dict[str, Any]:
    return rows[idx]


# role: orchestration
def first_stop_row(timeline: list[dict[str, Any]]) -> dict[str, Any]:
    return get_first_row(select_stop_rows(timeline))


# role: accessor
def extract_counter_value(row: dict[str, Any], counter_key: str) -> int:
    return row[counter_key]


# role: mapper
def extract_counter_trajectory(
    timeline: list[dict[str, Any]], counter_key: str
) -> list[int]:
    return [row[counter_key] for row in timeline]


# role: filter
def compact_trajectory(values: list[int]) -> list[int]:
    compacted: list[int] = []
    for value in values:
        if not compacted or compacted[-1] != value:
            compacted.append(value)
    return compacted


# role: orchestration
def stop_counter_trajectory(
    timeline: list[dict[str, Any]], counter_key: str
) -> list[int]:
    values = [extract_counter_value(get_first_row(timeline), counter_key)]
    values.extend(extract_counter_trajectory(select_stop_rows(timeline), counter_key))
    return compact_trajectory(values)


# role: orchestration
def all_counter_trajectory(timeline: list[dict[str, Any]], counter_key: str) -> list[int]:
    return compact_trajectory(extract_counter_trajectory(timeline, counter_key))


# role: accessor
def extract_finalization(result: dict[str, Any]) -> Any:
    return result.get("finalization")


# role: validator
def validate_finalization_shape(result: dict[str, Any]) -> dict[str, Any]:
    block = extract_finalization(result)
    if not isinstance(block, dict):
        raise AssertionError("evaluator result must include finalization object")
    return block


# role: accessor
def extract_fixture_trajectory(
    scenario: dict[str, Any], counter_name: str
) -> list[int]:
    return scenario["expected_counter_trajectory"][counter_name]


# role: accessor
def extract_first_snapshot(scenario: dict[str, Any]) -> dict[str, Any]:
    return scenario["expected_stop_snapshots"][0]


# role: filter
def select_rows_by_seq(rows: list[dict[str, Any]], seq: int) -> list[dict[str, Any]]:
    return [row for row in rows if row["seq"] == seq]


# role: orchestration
def stop_row_for_snapshot(
    timeline: list[dict[str, Any]], snapshot: dict[str, Any]
) -> dict[str, Any]:
    return get_first_row(select_rows_by_seq(select_stop_rows(timeline), snapshot["after_seq"]))


# role: mapper
def synthetic_background_pending_scenario() -> dict[str, Any]:
    return {
        "scenario_id": "synthetic_background_pending",
        "description": "Synthetic Drift A guard: Stop fires while background work is pending.",
        "events": [
            {
                "seq": 0,
                "event": "SessionStart",
                "timestamp": "2026-05-15T18:00:00Z",
            },
            {
                "seq": 1,
                "event": "PreToolUse",
                "tool": "Agent",
                "tool_kind": "agent",
                "subagent_id": "worker-1",
                "timestamp": "2026-05-15T18:00:01Z",
            },
            {
                "seq": 2,
                "event": "Stop",
                "timestamp": "2026-05-15T18:00:02Z",
            },
        ],
        "expected_counter_trajectory": {
            "active_tool": [0, 0],
            "pending_background_agent": [0, 1],
            "pending_scheduled_wakeup": [0, 0],
        },
        "expected_stop_snapshots": [
            {
                "after_seq": 2,
                "stop_for": "foreground",
                "active_tool_at_stop": 0,
                "pending_background_agent_at_stop": 1,
                "pending_scheduled_wakeup_at_stop": 0,
                "predicate": False,
            }
        ],
        "expected_finalization": {
            "reason": "checkpoint_pending_work",
            "stop_count": 1,
        },
    }


# role: accessor
def read_source_text(path: Path) -> str:
    return path.read_text(encoding="utf-8")


# role: parser
def split_source_lines(text: str) -> list[str]:
    return text.splitlines()


# role: filter
def select_forbidden_timeout_terms_present(scan_text: str, terms: list[str]) -> list[str]:
    return [term for term in terms if term in scan_text]


# role: formatter
def format_forbidden_timeout_diagnostic(source_name: str, term: str) -> str:
    return f"{source_name}: forbidden term {term}"


# role: mapper
def normalize_source_text_for_scan(text: str) -> str:
    return text.lower()


# role: filter
def match_sleep_success_pattern(source_text: str) -> re.Match[str] | None:
    return re.search(r"time\.sleep\(.*\)\s*#.*success", source_text, flags=re.IGNORECASE)


# role: formatter
def format_sleep_success_diagnostic(source_name: str, match: re.Match[str]) -> str:
    return f"{source_name}: time.sleep success pattern"


# role: filter
def select_time_monotonic_lines(
    enumerated_lines: Iterable[tuple[int, str]],
) -> list[tuple[int, str]]:
    return [(index, line) for index, line in enumerated_lines if "time.monotonic" in line]


# role: mapper
def project_line_indexes(enumerated_lines: Iterable[tuple[int, str]]) -> list[int]:
    return [index for index, _line in enumerated_lines]


# role: filter
def select_line_context_window(lines: list[str], index: int) -> list[str]:
    return lines[max(0, index - 5) : index + 6]


# role: formatter
def format_line_context_text(selected_lines: list[str]) -> str:
    return "\n".join(selected_lines)


# role: filter
def match_success_term_in_context(context_text: str) -> re.Match[str] | None:
    return re.search(
        r"(completion_predicate_satisfied|finalization|reason|success)",
        context_text,
        re.I,
    )


# role: formatter
def format_success_term_diagnostic(source_name: str, match: re.Match[str]) -> str:
    return f"{source_name}: time.monotonic success-adjacent context"


# role: orchestration
def scan_source_for_timeout_contract(path: Path) -> list[str]:
    source_name = str(path.relative_to(EVAL_ROOT))
    text = read_source_text(path)
    forbidden_terms = select_forbidden_timeout_terms_present(
        normalize_source_text_for_scan(text),
        FORBIDDEN_TIMEOUT_TERMS,
    )
    matches = [
        format_forbidden_timeout_diagnostic(source_name, term)
        for term in forbidden_terms
    ]
    sleep_match = match_sleep_success_pattern(text)
    if sleep_match is not None:
        matches.append(format_sleep_success_diagnostic(source_name, sleep_match))
    lines = split_source_lines(text)
    time_monotonic_lines = select_time_monotonic_lines(enumerate(lines))
    for index in project_line_indexes(time_monotonic_lines):
        context_window = select_line_context_window(lines, index)
        context_text = format_line_context_text(context_window)
        success_match = match_success_term_in_context(context_text)
        if success_match is not None:
            matches.append(format_success_term_diagnostic(source_name, success_match))
    return matches


# role: orchestration
def collect_anti_timeout_source_matches(paths: list[Path]) -> list[str]:
    matches: list[str] = []
    for path in paths:
        matches.extend(scan_source_for_timeout_contract(path))
    return matches


class ClaudeCompletionSignalContract(unittest.TestCase):
    # role: orchestration
    @classmethod
    def setUpClass(cls) -> None:
        cls.evaluator = load_evaluator_module(str(EVAL_ROOT))
        cls.baseline = load_baseline(str(BASELINE_PATH))
        cls.scenarios = {
            scenario_id: load_scenario(map_scenario_path(scenario_id))
            for scenario_id in SCENARIOS
        }
        cls.results = {
            scenario_id: validate_evaluation_result(
                evaluate_scenario_raw(cls.evaluator, scenario)
            )
            for scenario_id, scenario in cls.scenarios.items()
        }
        cls.timelines = {
            scenario_id: validate_timeline_shape(result)
            for scenario_id, result in cls.results.items()
        }
        cls.stop_rows = {
            scenario_id: select_stop_rows(timeline)
            for scenario_id, timeline in cls.timelines.items()
        }
        cls.first_stops = {
            scenario_id: get_first_row(stops)
            for scenario_id, stops in cls.stop_rows.items()
        }
        cls.finalizations = {
            scenario_id: validate_finalization_shape(result)
            for scenario_id, result in cls.results.items()
        }
        cls.stop_trajectories = {
            scenario_id: {
                counter_name: stop_counter_trajectory(
                    cls.timelines[scenario_id], counter_field
                )
                for counter_name, counter_field in COUNTER_FIELDS.items()
            }
            for scenario_id in SCENARIOS
        }
        cls.all_trajectories = {
            scenario_id: {
                counter_name: all_counter_trajectory(
                    cls.timelines[scenario_id], counter_field
                )
                for counter_name, counter_field in COUNTER_FIELDS.items()
            }
            for scenario_id in SCENARIOS
        }
        cls.first_fixture_snapshots = {
            scenario_id: extract_first_snapshot(scenario)
            for scenario_id, scenario in cls.scenarios.items()
        }
        cls.fixture_trajectories = {
            scenario_id: {
                counter_name: extract_fixture_trajectory(scenario, counter_name)
                for counter_name in COUNTER_FIELDS
            }
            for scenario_id, scenario in cls.scenarios.items()
        }
        cls.first_snapshot_rows = {
            scenario_id: stop_row_for_snapshot(
                cls.timelines[scenario_id],
                cls.first_fixture_snapshots[scenario_id],
            )
            for scenario_id in SCENARIOS
        }
        cls.synthetic_scenario = synthetic_background_pending_scenario()
        cls.synthetic_result = validate_evaluation_result(
            evaluate_scenario_raw(cls.evaluator, cls.synthetic_scenario)
        )
        cls.synthetic_timeline = validate_timeline_shape(cls.synthetic_result)
        cls.synthetic_first_stop = first_stop_row(cls.synthetic_timeline)
        cls.source_scan_matches = collect_anti_timeout_source_matches(SOURCE_PATHS)

    def test_clean_foreground_evaluator_matches_baseline(self) -> None:
        # role: validator
        expected = self.baseline["clean_foreground"]
        actual = self.finalizations["clean_foreground"]
        self.assertEqual(actual.get("stop_count"), expected.get("stop_count"))
        self.assertEqual(actual.get("final_active_tool"), expected.get("final_active_tool"))
        self.assertEqual(
            actual.get("final_pending_background_agent"),
            expected.get("final_pending_background_agent"),
        )
        self.assertEqual(
            actual.get("final_pending_scheduled_wakeup"),
            expected.get("final_pending_scheduled_wakeup"),
        )
        self.assertEqual(actual.get("reason"), expected.get("final_reason"))

    def test_clean_foreground_initial_stop_predicate_false_when_pending(self) -> None:
        # role: validator
        first_stop = self.first_stops["clean_foreground"]
        expected = EXPECTED_FIRST_STOP_SNAPSHOTS["clean_foreground"]
        self.assertEqual(first_stop.get("active_tool_after"), expected["active_tool_at_stop"])
        self.assertEqual(
            first_stop.get("pending_background_agent_after"),
            expected["pending_background_agent_at_stop"],
        )
        self.assertEqual(
            first_stop.get("pending_scheduled_wakeup_after"),
            expected["pending_scheduled_wakeup_at_stop"],
        )
        self.assertIs(first_stop.get("predicate_after"), expected["predicate"])

    def test_clean_foreground_finalization_reason_is_predicate_satisfied(self) -> None:
        # role: validator
        self.assertEqual(
            self.finalizations["clean_foreground"].get("reason"),
            "completion_predicate_satisfied",
        )

    def test_clean_foreground_finalization_reason_not_timeout(self) -> None:
        # role: validator
        reason = str(self.finalizations["clean_foreground"].get("reason", "")).lower()
        self.assertNotIn(reason, FORBIDDEN_TIMEOUT_TERMS)
        for forbidden in FORBIDDEN_TIMEOUT_TERMS:
            self.assertNotIn(forbidden, reason)

    def test_clean_foreground_timeline_has_required_fields(self) -> None:
        # role: validator
        rows = self.timelines["clean_foreground"]
        self.assertGreater(len(rows), 0)
        for index, row in enumerate(rows):
            with self.subTest(scenario="clean_foreground", row=index):
                self.assertTrue(TIMELINE_REQUIRED_FIELDS.issubset(row))

    def test_clean_foreground_stop_count_matches_contract(self) -> None:
        # role: validator
        stop_count = self.finalizations["clean_foreground"].get("stop_count")
        self.assertEqual(self.results["clean_foreground"].get("stop_count"), stop_count)
        self.assertEqual(len(self.stop_rows["clean_foreground"]), stop_count)
        self.assertEqual(stop_count, EXACT_STOP_COUNTS["clean_foreground"])

    def test_clean_foreground_counter_trajectory_matches_contract(self) -> None:
        # role: validator
        for counter_name, expected in EXPECTED_FIXTURE_TRAJECTORIES["clean_foreground"].items():
            self.assertEqual(self.fixture_trajectories["clean_foreground"][counter_name], expected)
            self.assertEqual(self.all_trajectories["clean_foreground"][counter_name], [0])

    def test_clean_foreground_first_stop_snapshot_matches_contract(self) -> None:
        # role: validator
        snapshot = self.first_fixture_snapshots["clean_foreground"]
        row = self.first_snapshot_rows["clean_foreground"]
        expected = EXPECTED_FIRST_STOP_SNAPSHOTS["clean_foreground"]
        for field, value in expected.items():
            self.assertEqual(snapshot.get(field), value)
        self.assertEqual(row.get("active_tool_after"), expected["active_tool_at_stop"])
        self.assertEqual(
            row.get("pending_background_agent_after"),
            expected["pending_background_agent_at_stop"],
        )
        self.assertEqual(
            row.get("pending_scheduled_wakeup_after"),
            expected["pending_scheduled_wakeup_at_stop"],
        )
        self.assertIs(row.get("predicate_after"), expected["predicate"])

    def test_foreground_tool_evaluator_matches_baseline(self) -> None:
        # role: validator
        expected = self.baseline["foreground_tool"]
        actual = self.finalizations["foreground_tool"]
        self.assertEqual(actual.get("stop_count"), expected.get("stop_count"))
        self.assertEqual(actual.get("final_active_tool"), expected.get("final_active_tool"))
        self.assertEqual(
            actual.get("final_pending_background_agent"),
            expected.get("final_pending_background_agent"),
        )
        self.assertEqual(
            actual.get("final_pending_scheduled_wakeup"),
            expected.get("final_pending_scheduled_wakeup"),
        )
        self.assertEqual(actual.get("reason"), expected.get("final_reason"))

    def test_foreground_tool_initial_stop_predicate_false_when_pending(self) -> None:
        # role: validator
        first_stop = self.first_stops["foreground_tool"]
        expected = EXPECTED_FIRST_STOP_SNAPSHOTS["foreground_tool"]
        self.assertEqual(first_stop.get("active_tool_after"), expected["active_tool_at_stop"])
        self.assertEqual(
            first_stop.get("pending_background_agent_after"),
            expected["pending_background_agent_at_stop"],
        )
        self.assertEqual(
            first_stop.get("pending_scheduled_wakeup_after"),
            expected["pending_scheduled_wakeup_at_stop"],
        )
        self.assertIs(first_stop.get("predicate_after"), expected["predicate"])

    def test_foreground_tool_finalization_reason_is_predicate_satisfied(self) -> None:
        # role: validator
        self.assertEqual(
            self.finalizations["foreground_tool"].get("reason"),
            "completion_predicate_satisfied",
        )

    def test_foreground_tool_finalization_reason_not_timeout(self) -> None:
        # role: validator
        reason = str(self.finalizations["foreground_tool"].get("reason", "")).lower()
        self.assertNotIn(reason, FORBIDDEN_TIMEOUT_TERMS)
        for forbidden in FORBIDDEN_TIMEOUT_TERMS:
            self.assertNotIn(forbidden, reason)

    def test_foreground_tool_timeline_has_required_fields(self) -> None:
        # role: validator
        rows = self.timelines["foreground_tool"]
        self.assertGreater(len(rows), 0)
        for index, row in enumerate(rows):
            with self.subTest(scenario="foreground_tool", row=index):
                self.assertTrue(TIMELINE_REQUIRED_FIELDS.issubset(row))

    def test_foreground_tool_stop_count_matches_contract(self) -> None:
        # role: validator
        stop_count = self.finalizations["foreground_tool"].get("stop_count")
        self.assertEqual(self.results["foreground_tool"].get("stop_count"), stop_count)
        self.assertEqual(len(self.stop_rows["foreground_tool"]), stop_count)
        self.assertEqual(stop_count, EXACT_STOP_COUNTS["foreground_tool"])

    def test_foreground_tool_counter_trajectory_matches_contract(self) -> None:
        # role: validator
        for counter_name, expected in EXPECTED_FIXTURE_TRAJECTORIES["foreground_tool"].items():
            self.assertEqual(self.fixture_trajectories["foreground_tool"][counter_name], expected)
        self.assertEqual(self.all_trajectories["foreground_tool"]["active_tool"], [0, 1, 0])
        self.assertEqual(
            self.all_trajectories["foreground_tool"]["pending_background_agent"], [0]
        )
        self.assertEqual(
            self.all_trajectories["foreground_tool"]["pending_scheduled_wakeup"], [0]
        )
        self.assertEqual(self.first_stops["foreground_tool"].get("active_tool_after"), 0)
        self.assertIs(self.first_stops["foreground_tool"].get("predicate_after"), True)

    def test_foreground_tool_first_stop_snapshot_matches_contract(self) -> None:
        # role: validator
        snapshot = self.first_fixture_snapshots["foreground_tool"]
        row = self.first_snapshot_rows["foreground_tool"]
        expected = EXPECTED_FIRST_STOP_SNAPSHOTS["foreground_tool"]
        for field, value in expected.items():
            self.assertEqual(snapshot.get(field), value)
        self.assertEqual(row.get("active_tool_after"), expected["active_tool_at_stop"])
        self.assertEqual(
            row.get("pending_background_agent_after"),
            expected["pending_background_agent_at_stop"],
        )
        self.assertEqual(
            row.get("pending_scheduled_wakeup_after"),
            expected["pending_scheduled_wakeup_at_stop"],
        )
        self.assertIs(row.get("predicate_after"), expected["predicate"])

    def test_one_background_evaluator_matches_baseline(self) -> None:
        # role: validator
        expected = self.baseline["one_background"]
        actual = self.finalizations["one_background"]
        self.assertEqual(actual.get("stop_count"), expected.get("stop_count"))
        self.assertEqual(actual.get("final_active_tool"), expected.get("final_active_tool"))
        self.assertEqual(
            actual.get("final_pending_background_agent"),
            expected.get("final_pending_background_agent"),
        )
        self.assertEqual(
            actual.get("final_pending_scheduled_wakeup"),
            expected.get("final_pending_scheduled_wakeup"),
        )
        self.assertEqual(actual.get("reason"), expected.get("final_reason"))

    def test_one_background_initial_stop_predicate_false_when_pending(self) -> None:
        # role: validator
        first_stop = self.first_stops["one_background"]
        expected = EXPECTED_FIRST_STOP_SNAPSHOTS["one_background"]
        self.assertEqual(first_stop.get("active_tool_after"), expected["active_tool_at_stop"])
        self.assertEqual(
            first_stop.get("pending_background_agent_after"),
            expected["pending_background_agent_at_stop"],
        )
        self.assertEqual(
            first_stop.get("pending_scheduled_wakeup_after"),
            expected["pending_scheduled_wakeup_at_stop"],
        )
        self.assertIs(first_stop.get("predicate_after"), expected["predicate"])

    def test_one_background_finalization_reason_is_predicate_satisfied(self) -> None:
        # role: validator
        self.assertEqual(
            self.finalizations["one_background"].get("reason"),
            "completion_predicate_satisfied",
        )

    def test_one_background_finalization_reason_not_timeout(self) -> None:
        # role: validator
        reason = str(self.finalizations["one_background"].get("reason", "")).lower()
        self.assertNotIn(reason, FORBIDDEN_TIMEOUT_TERMS)
        for forbidden in FORBIDDEN_TIMEOUT_TERMS:
            self.assertNotIn(forbidden, reason)

    def test_one_background_timeline_has_required_fields(self) -> None:
        # role: validator
        rows = self.timelines["one_background"]
        self.assertGreater(len(rows), 0)
        for index, row in enumerate(rows):
            with self.subTest(scenario="one_background", row=index):
                self.assertTrue(TIMELINE_REQUIRED_FIELDS.issubset(row))

    def test_one_background_stop_count_matches_contract(self) -> None:
        # role: validator
        stop_count = self.finalizations["one_background"].get("stop_count")
        self.assertEqual(self.results["one_background"].get("stop_count"), stop_count)
        self.assertEqual(len(self.stop_rows["one_background"]), stop_count)
        self.assertGreaterEqual(stop_count, MINIMUM_STOP_COUNTS["one_background"])

    def test_one_background_counter_trajectory_matches_contract(self) -> None:
        # role: validator
        for counter_name, expected in EXPECTED_FIXTURE_TRAJECTORIES["one_background"].items():
            self.assertEqual(self.fixture_trajectories["one_background"][counter_name], expected)
        self.assertEqual(
            self.stop_trajectories["one_background"]["pending_background_agent"], [0, 1, 0]
        )
        self.assertEqual(self.all_trajectories["one_background"]["active_tool"], [0])
        self.assertEqual(
            self.all_trajectories["one_background"]["pending_scheduled_wakeup"], [0]
        )
        self.assertEqual(
            self.first_stops["one_background"].get("pending_background_agent_after"), 1
        )
        self.assertIs(self.first_stops["one_background"].get("predicate_after"), False)

    def test_one_background_first_stop_snapshot_matches_contract(self) -> None:
        # role: validator
        snapshot = self.first_fixture_snapshots["one_background"]
        row = self.first_snapshot_rows["one_background"]
        expected = EXPECTED_FIRST_STOP_SNAPSHOTS["one_background"]
        for field, value in expected.items():
            self.assertEqual(snapshot.get(field), value)
        self.assertEqual(row.get("active_tool_after"), expected["active_tool_at_stop"])
        self.assertEqual(
            row.get("pending_background_agent_after"),
            expected["pending_background_agent_at_stop"],
        )
        self.assertEqual(
            row.get("pending_scheduled_wakeup_after"),
            expected["pending_scheduled_wakeup_at_stop"],
        )
        self.assertIs(row.get("predicate_after"), expected["predicate"])

    def test_five_background_evaluator_matches_baseline(self) -> None:
        # role: validator
        expected = self.baseline["five_background"]
        actual = self.finalizations["five_background"]
        self.assertEqual(actual.get("stop_count"), expected.get("stop_count"))
        self.assertEqual(actual.get("final_active_tool"), expected.get("final_active_tool"))
        self.assertEqual(
            actual.get("final_pending_background_agent"),
            expected.get("final_pending_background_agent"),
        )
        self.assertEqual(
            actual.get("final_pending_scheduled_wakeup"),
            expected.get("final_pending_scheduled_wakeup"),
        )
        self.assertEqual(actual.get("reason"), expected.get("final_reason"))

    def test_five_background_initial_stop_predicate_false_when_pending(self) -> None:
        # role: validator
        first_stop = self.first_stops["five_background"]
        expected = EXPECTED_FIRST_STOP_SNAPSHOTS["five_background"]
        self.assertEqual(first_stop.get("active_tool_after"), expected["active_tool_at_stop"])
        self.assertEqual(
            first_stop.get("pending_background_agent_after"),
            expected["pending_background_agent_at_stop"],
        )
        self.assertEqual(
            first_stop.get("pending_scheduled_wakeup_after"),
            expected["pending_scheduled_wakeup_at_stop"],
        )
        self.assertIs(first_stop.get("predicate_after"), expected["predicate"])

    def test_five_background_finalization_reason_is_predicate_satisfied(self) -> None:
        # role: validator
        self.assertEqual(
            self.finalizations["five_background"].get("reason"),
            "completion_predicate_satisfied",
        )

    def test_five_background_finalization_reason_not_timeout(self) -> None:
        # role: validator
        reason = str(self.finalizations["five_background"].get("reason", "")).lower()
        self.assertNotIn(reason, FORBIDDEN_TIMEOUT_TERMS)
        for forbidden in FORBIDDEN_TIMEOUT_TERMS:
            self.assertNotIn(forbidden, reason)

    def test_five_background_timeline_has_required_fields(self) -> None:
        # role: validator
        rows = self.timelines["five_background"]
        self.assertGreater(len(rows), 0)
        for index, row in enumerate(rows):
            with self.subTest(scenario="five_background", row=index):
                self.assertTrue(TIMELINE_REQUIRED_FIELDS.issubset(row))

    def test_five_background_stop_count_matches_contract(self) -> None:
        # role: validator
        stop_count = self.finalizations["five_background"].get("stop_count")
        self.assertEqual(self.results["five_background"].get("stop_count"), stop_count)
        self.assertEqual(len(self.stop_rows["five_background"]), stop_count)
        self.assertGreaterEqual(stop_count, MINIMUM_STOP_COUNTS["five_background"])

    def test_five_background_counter_trajectory_matches_contract(self) -> None:
        # role: validator
        for counter_name, expected in EXPECTED_FIXTURE_TRAJECTORIES["five_background"].items():
            self.assertEqual(self.fixture_trajectories["five_background"][counter_name], expected)
        self.assertEqual(
            self.stop_trajectories["five_background"]["pending_background_agent"],
            [0, 5, 4, 3, 2, 1, 0],
        )
        self.assertEqual(self.all_trajectories["five_background"]["active_tool"], [0])
        self.assertEqual(
            self.all_trajectories["five_background"]["pending_scheduled_wakeup"], [0]
        )
        self.assertGreaterEqual(len(self.stop_rows["five_background"]), 6)
        for row in self.stop_rows["five_background"]:
            if row.get("pending_background_agent_after") != 0:
                self.assertIs(row.get("predicate_after"), False)

    def test_five_background_first_stop_snapshot_matches_contract(self) -> None:
        # role: validator
        snapshot = self.first_fixture_snapshots["five_background"]
        row = self.first_snapshot_rows["five_background"]
        expected = EXPECTED_FIRST_STOP_SNAPSHOTS["five_background"]
        for field, value in expected.items():
            self.assertEqual(snapshot.get(field), value)
        self.assertEqual(row.get("active_tool_after"), expected["active_tool_at_stop"])
        self.assertEqual(
            row.get("pending_background_agent_after"),
            expected["pending_background_agent_at_stop"],
        )
        self.assertEqual(
            row.get("pending_scheduled_wakeup_after"),
            expected["pending_scheduled_wakeup_at_stop"],
        )
        self.assertIs(row.get("predicate_after"), expected["predicate"])

    def test_scheduled_wakeup_evaluator_matches_baseline(self) -> None:
        # role: validator
        expected = self.baseline["scheduled_wakeup"]
        actual = self.finalizations["scheduled_wakeup"]
        self.assertEqual(actual.get("stop_count"), expected.get("stop_count"))
        self.assertEqual(actual.get("final_active_tool"), expected.get("final_active_tool"))
        self.assertEqual(
            actual.get("final_pending_background_agent"),
            expected.get("final_pending_background_agent"),
        )
        self.assertEqual(
            actual.get("final_pending_scheduled_wakeup"),
            expected.get("final_pending_scheduled_wakeup"),
        )
        self.assertEqual(actual.get("reason"), expected.get("final_reason"))

    def test_scheduled_wakeup_initial_stop_predicate_false_when_pending(self) -> None:
        # role: validator
        first_stop = self.first_stops["scheduled_wakeup"]
        expected = EXPECTED_FIRST_STOP_SNAPSHOTS["scheduled_wakeup"]
        self.assertEqual(first_stop.get("active_tool_after"), expected["active_tool_at_stop"])
        self.assertEqual(
            first_stop.get("pending_background_agent_after"),
            expected["pending_background_agent_at_stop"],
        )
        self.assertEqual(
            first_stop.get("pending_scheduled_wakeup_after"),
            expected["pending_scheduled_wakeup_at_stop"],
        )
        self.assertIs(first_stop.get("predicate_after"), expected["predicate"])

    def test_scheduled_wakeup_finalization_reason_is_predicate_satisfied(self) -> None:
        # role: validator
        self.assertEqual(
            self.finalizations["scheduled_wakeup"].get("reason"),
            "completion_predicate_satisfied",
        )

    def test_scheduled_wakeup_finalization_reason_not_timeout(self) -> None:
        # role: validator
        reason = str(self.finalizations["scheduled_wakeup"].get("reason", "")).lower()
        self.assertNotIn(reason, FORBIDDEN_TIMEOUT_TERMS)
        for forbidden in FORBIDDEN_TIMEOUT_TERMS:
            self.assertNotIn(forbidden, reason)

    def test_scheduled_wakeup_timeline_has_required_fields(self) -> None:
        # role: validator
        rows = self.timelines["scheduled_wakeup"]
        self.assertGreater(len(rows), 0)
        for index, row in enumerate(rows):
            with self.subTest(scenario="scheduled_wakeup", row=index):
                self.assertTrue(TIMELINE_REQUIRED_FIELDS.issubset(row))

    def test_scheduled_wakeup_stop_count_matches_contract(self) -> None:
        # role: validator
        stop_count = self.finalizations["scheduled_wakeup"].get("stop_count")
        self.assertEqual(self.results["scheduled_wakeup"].get("stop_count"), stop_count)
        self.assertEqual(len(self.stop_rows["scheduled_wakeup"]), stop_count)
        self.assertGreaterEqual(stop_count, MINIMUM_STOP_COUNTS["scheduled_wakeup"])

    def test_scheduled_wakeup_counter_trajectory_matches_contract(self) -> None:
        # role: validator
        for counter_name, expected in EXPECTED_FIXTURE_TRAJECTORIES["scheduled_wakeup"].items():
            self.assertEqual(self.fixture_trajectories["scheduled_wakeup"][counter_name], expected)
        self.assertEqual(
            self.stop_trajectories["scheduled_wakeup"]["pending_scheduled_wakeup"],
            [0, 1, 0],
        )
        self.assertEqual(self.all_trajectories["scheduled_wakeup"]["active_tool"], [0])
        self.assertEqual(
            self.all_trajectories["scheduled_wakeup"]["pending_background_agent"], [0]
        )
        self.assertEqual(
            self.first_stops["scheduled_wakeup"].get("pending_scheduled_wakeup_after"), 1
        )
        self.assertIs(self.first_stops["scheduled_wakeup"].get("predicate_after"), False)

    def test_scheduled_wakeup_first_stop_snapshot_matches_contract(self) -> None:
        # role: validator
        snapshot = self.first_fixture_snapshots["scheduled_wakeup"]
        row = self.first_snapshot_rows["scheduled_wakeup"]
        expected = EXPECTED_FIRST_STOP_SNAPSHOTS["scheduled_wakeup"]
        for field, value in expected.items():
            self.assertEqual(snapshot.get(field), value)
        self.assertEqual(row.get("active_tool_after"), expected["active_tool_at_stop"])
        self.assertEqual(
            row.get("pending_background_agent_after"),
            expected["pending_background_agent_at_stop"],
        )
        self.assertEqual(
            row.get("pending_scheduled_wakeup_after"),
            expected["pending_scheduled_wakeup_at_stop"],
        )
        self.assertIs(row.get("predicate_after"), expected["predicate"])

    def test_predicate_requires_pending_background_agent_count(self) -> None:
        # role: validator
        first_stop = self.synthetic_first_stop
        self.assertEqual(first_stop.get("active_tool_after"), 0)
        self.assertEqual(first_stop.get("pending_background_agent_after"), 1)
        self.assertEqual(first_stop.get("pending_scheduled_wakeup_after"), 0)
        self.assertIs(first_stop.get("predicate_after"), False)

    def test_anti_timeout_source_scan_no_forbidden_strings(self) -> None:
        # role: validator
        self.assertEqual([], self.source_scan_matches)

    def test_all_scenario_fixtures_validate_against_schema(self) -> None:
        # role: validator
        for scenario_id in SCENARIOS:
            with self.subTest(scenario=scenario_id):
                payload = self.scenarios[scenario_id]
                self.assertEqual(payload.get("scenario_id"), scenario_id)
                self.assertIsInstance(payload.get("description"), str)
                events = payload.get("events")
                self.assertIsInstance(events, list)
                self.assertGreaterEqual(len(events), 1)
                for index, event in enumerate(events):
                    self.assertIsInstance(event, dict)
                    self.assertIsInstance(event.get("seq"), int)
                    self.assertIsInstance(event.get("event"), str)
                    self.assertIn(event.get("event"), EVENT_NAMES)
                    self.assertIsInstance(event.get("timestamp"), str)
                    for optional in ["tool", "tool_kind", "subagent_id", "wakeup_id"]:
                        if optional in event:
                            self.assertIsInstance(event[optional], str)
                    if "tool_kind" in event:
                        self.assertIn(event["tool_kind"], {"foreground", "agent"})
                    if index > 0:
                        previous = events[index - 1]
                        if isinstance(previous, dict) and isinstance(previous.get("seq"), int):
                            self.assertGreater(event["seq"], previous["seq"])
                trajectory = payload.get("expected_counter_trajectory")
                self.assertIsInstance(trajectory, dict)
                for field in COUNTER_FIELDS:
                    self.assertIn(field, trajectory)
                    values = trajectory[field]
                    self.assertIsInstance(values, list)
                    self.assertGreaterEqual(len(values), 1)
                    for value in values:
                        self.assertIsInstance(value, int)
                        self.assertGreaterEqual(value, 0)
                snapshots = payload.get("expected_stop_snapshots")
                self.assertIsInstance(snapshots, list)
                self.assertGreaterEqual(len(snapshots), 1)
                for snapshot in snapshots:
                    self.assertIsInstance(snapshot, dict)
                    self.assertIsInstance(snapshot.get("after_seq"), int)
                    self.assertIn(snapshot.get("stop_for"), {"foreground", "subagent", "wakeup"})
                    for field in [
                        "active_tool_at_stop",
                        "pending_background_agent_at_stop",
                        "pending_scheduled_wakeup_at_stop",
                    ]:
                        self.assertIsInstance(snapshot.get(field), int)
                        self.assertGreaterEqual(snapshot[field], 0)
                    self.assertIsInstance(snapshot.get("predicate"), bool)
                expected = payload.get("expected_finalization")
                self.assertIsInstance(expected, dict)
                self.assertIn(expected.get("reason"), FINALIZATION_REASONS)
                self.assertIsInstance(expected.get("stop_count"), int)
                self.assertGreaterEqual(expected["stop_count"], 1)

    def test_completion_baseline_covers_all_required_scenarios(self) -> None:
        # role: validator
        self.assertEqual(set(SCENARIOS), set(self.baseline))
        for scenario_id, row in self.baseline.items():
            with self.subTest(scenario=scenario_id):
                self.assertIsInstance(row, dict)
                for field in [
                    "stop_count",
                    "final_active_tool",
                    "final_pending_background_agent",
                    "final_pending_scheduled_wakeup",
                ]:
                    self.assertIsInstance(row.get(field), int)
                    self.assertGreaterEqual(row[field], 0)
                self.assertEqual(row.get("final_active_tool"), 0)
                self.assertEqual(row.get("final_pending_background_agent"), 0)
                self.assertEqual(row.get("final_pending_scheduled_wakeup"), 0)
                self.assertEqual(row.get("final_reason"), "completion_predicate_satisfied")
                if scenario_id in EXACT_STOP_COUNTS:
                    self.assertEqual(row.get("stop_count"), EXACT_STOP_COUNTS[scenario_id])
                else:
                    self.assertGreaterEqual(row.get("stop_count"), MINIMUM_STOP_COUNTS[scenario_id])

    def test_eval_output_includes_stop_events_and_counter_snapshots_and_wakeup_state_and_finalization_reason(
        self,
    ) -> None:
        # role: validator
        for scenario_id in SCENARIOS:
            with self.subTest(scenario=scenario_id):
                stops = self.stop_rows[scenario_id]
                self.assertGreaterEqual(len(stops), 1)
                for row in stops:
                    self.assertTrue(TIMELINE_REQUIRED_FIELDS.issubset(row))
                    self.assertIsInstance(row["active_tool_after"], int)
                    self.assertIsInstance(row["pending_background_agent_after"], int)
                    self.assertIsInstance(row["pending_scheduled_wakeup_after"], int)
                    self.assertIsInstance(row["predicate_after"], bool)
                final = self.finalizations[scenario_id]
                self.assertIn("reason", final)
                self.assertEqual(final["reason"], "completion_predicate_satisfied")


# role: accessor
def read_provenance_text() -> str:
    return PROVENANCE_PATH.read_text(encoding="utf-8")


# role: accessor
def read_provenance_bytes() -> bytes:
    return PROVENANCE_PATH.read_bytes()


# role: accessor
def read_manifest_text() -> str:
    return PROVENANCE_MANIFEST_PATH.read_text(encoding="utf-8")


# role: parser
def parse_provenance_json(text: str) -> dict[str, Any]:
    return json.loads(text)


# role: parser
def parse_manifest_json(text: str) -> dict[str, Any]:
    return json.loads(text)


# role: formatter
def format_provenance_json(payload: dict[str, Any]) -> str:
    return json.dumps(payload, sort_keys=True)


# role: filter
def select_valid_manifest_entries(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    return [
        entry
        for entry in manifest["entries"]
        if isinstance(entry, dict) and isinstance(entry.get("stable_id"), str)
    ]


# role: mapper
def project_manifest_entry_stable_ids(valid_entries: list[dict[str, Any]]) -> set[str]:
    return {entry["stable_id"] for entry in valid_entries}


# role: predicate
def is_path_naming_key(key: str) -> bool:
    return (
        key == "manifest_ref"
        or key.endswith("_path")
        or key.endswith("_ref")
        or key.endswith("_dir")
    )


# role: filter
def select_path_naming_keys(payload: dict[str, Any]) -> Iterable[str]:
    return [
        key
        for key, value in payload.items()
        if is_path_naming_key(key) and isinstance(value, str)
    ]


# role: mapper
def compile_repo_relative_path_pattern() -> re.Pattern[str]:
    return re.compile(r"^(?:/|~|[A-Za-z]:\\|\\\\\?\\)")


# role: mapper
def compile_absolute_path_pattern() -> re.Pattern[str]:
    return re.compile(r"^(?:/|[A-Za-z]:\\|~)")


REPO_RELATIVE_PATH_PATTERN = compile_repo_relative_path_pattern()
ABSOLUTE_PATH_PATTERN = compile_absolute_path_pattern()


# role: parser
def tokenize_text_whitespace(text: str) -> list[str]:
    return text.split()


# role: filter
def select_absolute_path_token_matches(tokens: list[str]) -> list[str]:
    return [token for token in tokens if ABSOLUTE_PATH_PATTERN.match(token)]


class ProvenanceCompatibility(unittest.TestCase):
    repo_root = EVAL_ROOT.parents[1]
    provenance_relpath = Path("evals/claude-completion-signal/provenance.json")
    manifest_relpath = Path("evals/_provenance/age-89-dossier-manifest.json")
    provenance_path = repo_root / provenance_relpath
    manifest_path = repo_root / manifest_relpath

    # role: orchestration
    @classmethod
    def setUpClass(cls) -> None:
        cls.provenance_text = read_provenance_text()
        cls.provenance_bytes_text = read_provenance_bytes().decode("utf-8")
        cls.provenance = parse_provenance_json(cls.provenance_text)
        cls.serialized_provenance = format_provenance_json(cls.provenance)
        cls.manifest = parse_manifest_json(read_manifest_text())
        cls.repo_relative_path_pattern = REPO_RELATIVE_PATH_PATTERN
        cls.path_naming_keys = (
            select_path_naming_keys(cls.provenance)
            if isinstance(cls.provenance, dict)
            else []
        )
        valid_manifest_entries = (
            select_valid_manifest_entries(cls.manifest)
            if isinstance(cls.manifest, dict)
            and isinstance(cls.manifest.get("entries"), list)
            else []
        )
        cls.manifest_stable_ids = project_manifest_entry_stable_ids(
            valid_manifest_entries
        )
        cls.absolute_path_tokens = select_absolute_path_token_matches(
            tokenize_text_whitespace(cls.provenance_bytes_text)
        )

    def test_provenance_consumer_shape_is_valid(self) -> None:
        # role: validator
        provenance = self.provenance
        self.assertIsInstance(provenance, dict)
        if "consumer_id" in provenance:
            self.assertIsInstance(provenance["consumer_id"], str)
            self.assertEqual(provenance["consumer_id"], "age-127-completion-signal-eval")
        self.assertIn("manifest_ref", provenance)
        self.assertIsInstance(provenance["manifest_ref"], str)
        self.assertIn("information_only", provenance)
        self.assertIs(provenance["information_only"], True)
        self.assertIn("cited_entries", provenance)
        cited_entries = provenance["cited_entries"]
        self.assertIsInstance(cited_entries, list)
        self.assertGreaterEqual(len(cited_entries), 1)
        for index, entry in enumerate(cited_entries):
            with self.subTest(cited_entry=index):
                self.assertIsInstance(entry, dict)
                self.assertIsInstance(entry.get("stable_id"), str)
                self.assertNotEqual(entry["stable_id"].strip(), "")
                self.assertIsInstance(entry.get("purpose"), str)
                self.assertNotEqual(entry["purpose"].strip(), "")

    def test_provenance_manifest_ref_equals_repo_relative_canonical_path(self) -> None:
        # role: validator
        provenance = self.provenance
        self.assertEqual(
            provenance.get("manifest_ref"),
            "evals/_provenance/age-89-dossier-manifest.json",
        )

    def test_provenance_path_naming_fields_are_repo_relative(self) -> None:
        # role: validator
        provenance = self.provenance
        self.assertIsInstance(provenance, dict)
        for key in self.path_naming_keys:
            value = provenance[key]
            with self.subTest(path_field=key):
                self.assertIsNone(
                    self.repo_relative_path_pattern.match(value),
                    f"{key} must be repo-relative, got {value!r}",
                )

    def test_provenance_cited_stable_ids_exist_in_age126_manifest(self) -> None:
        # role: validator
        provenance = self.provenance
        manifest = self.manifest
        self.assertIsInstance(manifest, dict)
        entries = manifest.get("entries")
        self.assertIsInstance(entries, list)
        cited_entries = provenance.get("cited_entries")
        self.assertIsInstance(cited_entries, list)
        for index, entry in enumerate(cited_entries):
            with self.subTest(cited_entry=index):
                self.assertIsInstance(entry, dict)
                stable_id = entry.get("stable_id")
                self.assertIsInstance(stable_id, str)
                self.assertIn(stable_id, self.manifest_stable_ids)

    def test_provenance_contains_no_pp007_marker_substrings(self) -> None:
        # role: validator
        raw = self.provenance_text
        serialized = self.serialized_provenance
        for marker in ["/home/", "/Users/", "C:\\"]:
            with self.subTest(marker=marker, surface="raw"):
                self.assertNotIn(marker, raw)
        for marker in ["/home/", "/Users/", "C:\\\\"]:
            with self.subTest(marker=marker, surface="serialized"):
                self.assertNotIn(marker, serialized)

    def test_provenance_passes_absolute_path_token_regex_scan(self) -> None:
        # role: validator
        self.assertEqual([], self.absolute_path_tokens)


if __name__ == "__main__":
    unittest.main()
