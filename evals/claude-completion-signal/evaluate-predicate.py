#!/usr/bin/env python3
"""Replay AGE-105 Claude completion-signal fixtures."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any, Iterable


SCENARIO_IDS = [
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

COUNTER_TRAJECTORY_FIELDS = {
    "active_tool",
    "pending_background_agent",
    "pending_scheduled_wakeup",
}

FINALIZATION_REASONS = {
    "completion_predicate_satisfied",
    "checkpoint_pending_work",
}


# role: predicate
def predicate(
    stop_fired_for_current_turn: bool,
    active_tool_counter: int,
    pending_background_agent_count: int,
    pending_scheduled_wakeup_count: int,
) -> bool:
    return (
        stop_fired_for_current_turn
        and active_tool_counter == 0
        and pending_background_agent_count == 0
        and pending_scheduled_wakeup_count == 0
    )


# role: accessor
def read_scenario_bytes(path: Path) -> bytes:
    return path.read_bytes()


# role: parser
def decode_scenario_json(raw: bytes) -> Any:
    return json.loads(raw.decode("utf-8"))


# role: validator
def validate_counter_trajectory_shape(trajectory: Any) -> dict[str, Any]:
    if not isinstance(trajectory, dict):
        raise ValueError("expected_counter_trajectory must be an object")
    for field in COUNTER_TRAJECTORY_FIELDS:
        values = trajectory.get(field)
        if not isinstance(values, list):
            raise ValueError(f"expected_counter_trajectory.{field} must be a list")
        if not values:
            raise ValueError(f"expected_counter_trajectory.{field} must not be empty")
        for value in values:
            if not isinstance(value, int) or value < 0:
                raise ValueError(
                    f"expected_counter_trajectory.{field} values must be nonnegative ints"
                )
    return trajectory


# role: validator
def validate_event_shape(event: Any) -> dict[str, Any]:
    if not isinstance(event, dict):
        raise ValueError("scenario events must be objects")
    if not isinstance(event.get("seq"), int):
        raise ValueError("event.seq must be an int")
    if event.get("event") not in EVENT_NAMES:
        raise ValueError("event.event is not supported")
    if not isinstance(event.get("timestamp"), str):
        raise ValueError("event.timestamp must be a string")
    for optional in ["tool", "tool_kind", "subagent_id", "wakeup_id"]:
        if optional in event and not isinstance(event[optional], str):
            raise ValueError(f"event.{optional} must be a string")
    if "tool_kind" in event and event["tool_kind"] not in {"foreground", "agent"}:
        raise ValueError("event.tool_kind is not supported")
    return event


# role: validator
def validate_events_shape(events: Any) -> list[dict[str, Any]]:
    if not isinstance(events, list) or not events:
        raise ValueError("events must be a non-empty list")
    validated = [validate_event_shape(event) for event in events]
    for previous, current in zip(validated, validated[1:]):
        if current["seq"] <= previous["seq"]:
            raise ValueError("event.seq values must increase")
    return validated


# role: validator
def validate_stop_snapshot_shape(snapshot: Any) -> dict[str, Any]:
    if not isinstance(snapshot, dict):
        raise ValueError("expected_stop_snapshots entries must be objects")
    if not isinstance(snapshot.get("after_seq"), int):
        raise ValueError("expected_stop_snapshots.after_seq must be an int")
    if snapshot.get("stop_for") not in {"foreground", "subagent", "wakeup"}:
        raise ValueError("expected_stop_snapshots.stop_for is not supported")
    for field in [
        "active_tool_at_stop",
        "pending_background_agent_at_stop",
        "pending_scheduled_wakeup_at_stop",
    ]:
        if not isinstance(snapshot.get(field), int) or snapshot[field] < 0:
            raise ValueError(f"expected_stop_snapshots.{field} must be a nonnegative int")
    if not isinstance(snapshot.get("predicate"), bool):
        raise ValueError("expected_stop_snapshots.predicate must be a bool")
    return snapshot


# role: validator
def validate_stop_snapshots_shape(snapshots: Any) -> list[dict[str, Any]]:
    if not isinstance(snapshots, list) or not snapshots:
        raise ValueError("expected_stop_snapshots must be a non-empty list")
    return [validate_stop_snapshot_shape(snapshot) for snapshot in snapshots]


# role: validator
def validate_finalization_shape(finalization: Any) -> dict[str, Any]:
    if not isinstance(finalization, dict):
        raise ValueError("expected_finalization must be an object")
    if finalization.get("reason") not in FINALIZATION_REASONS:
        raise ValueError("expected_finalization.reason is not supported")
    if not isinstance(finalization.get("stop_count"), int) or finalization["stop_count"] < 1:
        raise ValueError("expected_finalization.stop_count must be a positive int")
    return finalization


# role: validator
def validate_scenario_shape(raw_scenario: Any) -> dict[str, Any]:
    if not isinstance(raw_scenario, dict):
        raise ValueError("scenario payload must be an object")
    if not isinstance(raw_scenario.get("scenario_id"), str):
        raise ValueError("scenario_id must be a string")
    if not isinstance(raw_scenario.get("description"), str):
        raise ValueError("description must be a string")
    validate_events_shape(raw_scenario.get("events"))
    validate_counter_trajectory_shape(raw_scenario.get("expected_counter_trajectory"))
    validate_stop_snapshots_shape(raw_scenario.get("expected_stop_snapshots"))
    validate_finalization_shape(raw_scenario.get("expected_finalization"))
    return raw_scenario


# role: orchestration
def load_scenario(path: Path) -> dict[str, Any]:
    return validate_scenario_shape(decode_scenario_json(read_scenario_bytes(path)))


# role: predicate
def is_background_launch(event: dict[str, Any]) -> bool:
    return event.get("event") == "PreToolUse" and event.get("tool_kind") == "agent"


# role: predicate
def is_foreground_tool_start(event: dict[str, Any]) -> bool:
    return event.get("event") == "PreToolUse" and event.get("tool_kind") == "foreground"


# role: predicate
def is_foreground_tool_end(event: dict[str, Any]) -> bool:
    return event.get("event") == "PostToolUse" and event.get("tool_kind") == "foreground"


# role: predicate
def is_background_completion(event: dict[str, Any]) -> bool:
    return event.get("event") == "SubagentStop"


# role: predicate
def is_scheduled_wakeup(event: dict[str, Any]) -> bool:
    return event.get("event") == "ScheduleWakeup"


# role: predicate
def is_wakeup_delivery(event: dict[str, Any]) -> bool:
    return event.get("event") == "Notification" and "wakeup_id" in event


# role: predicate
def is_stop_event(event: dict[str, Any]) -> bool:
    return event.get("event") == "Stop"


# role: mapper
def increment(value: int) -> int:
    return value + 1


# role: mapper
def decrement_floor_zero(value: int) -> int:
    return max(0, value - 1)


# role: mapper
def clear_counter(_: int) -> int:
    return 0


# role: mapper
def map_event_to_state(
    event: dict[str, Any],
    previous_state: tuple[int, int, int],
) -> tuple[int, int, int]:
    active_tool, pending_background_agent, pending_scheduled_wakeup = previous_state
    if is_background_launch(event):
        pending_background_agent = increment(pending_background_agent)
    if is_foreground_tool_start(event):
        active_tool = increment(active_tool)
    if is_foreground_tool_end(event):
        active_tool = decrement_floor_zero(active_tool)
    if is_background_completion(event):
        pending_background_agent = decrement_floor_zero(pending_background_agent)
    if is_scheduled_wakeup(event):
        pending_scheduled_wakeup = increment(pending_scheduled_wakeup)
    if is_wakeup_delivery(event):
        pending_scheduled_wakeup = clear_counter(pending_scheduled_wakeup)
    return active_tool, pending_background_agent, pending_scheduled_wakeup


# role: mapper
def map_event_to_timeline_row(
    event: dict[str, Any],
    state: tuple[int, int, int],
) -> dict[str, Any]:
    active_tool, pending_background_agent, pending_scheduled_wakeup = state
    return {
        "seq": event["seq"],
        "event": event["event"],
        "active_tool_after": active_tool,
        "pending_background_agent_after": pending_background_agent,
        "pending_scheduled_wakeup_after": pending_scheduled_wakeup,
        "predicate_after": predicate(
            is_stop_event(event),
            active_tool,
            pending_background_agent,
            pending_scheduled_wakeup,
        ),
    }


# role: mapper
def map_events_to_timeline(events: Iterable[dict[str, Any]]) -> list[dict[str, Any]]:
    timeline = []
    state = (0, 0, 0)
    for event in events:
        state = map_event_to_state(event, state)
        timeline.append(map_event_to_timeline_row(event, state))
    return timeline


# role: filter
def select_stop_rows(timeline: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [row for row in timeline if row["event"] == "Stop"]


# role: accessor
def get_last_row(rows: list[dict[str, Any]]) -> dict[str, Any]:
    return rows[-1]


# role: accessor
def extract_predicate_value(row: dict[str, Any]) -> bool:
    return row["predicate_after"]


# role: accessor
def count_rows(rows: list[dict[str, Any]]) -> int:
    return len(rows)


# role: mapper
def map_finalization_reason(final_stop_predicate: bool) -> str:
    if final_stop_predicate:
        return "completion_predicate_satisfied"
    return "checkpoint_pending_work"


# role: formatter
def format_finalization_record(
    stop_count: int,
    final_row: dict[str, Any],
    reason: str,
) -> dict[str, Any]:
    return {
        "reason": reason,
        "stop_count": stop_count,
        "final_active_tool": final_row["active_tool_after"],
        "final_pending_background_agent": final_row["pending_background_agent_after"],
        "final_pending_scheduled_wakeup": final_row[
            "pending_scheduled_wakeup_after"
        ],
    }


# role: formatter
def format_evaluation_result(
    scenario_id: str,
    timeline: list[dict[str, Any]],
    finalization: dict[str, Any],
) -> dict[str, Any]:
    return {
        "scenario_id": scenario_id,
        "stop_count": finalization["stop_count"],
        "timeline": timeline,
        "finalization": finalization,
    }


# role: orchestration
def evaluate(scenario: dict[str, Any]) -> dict[str, Any]:
    validated = validate_scenario_shape(scenario)
    timeline = map_events_to_timeline(validated["events"])
    stop_rows = select_stop_rows(timeline)
    final_stop = get_last_row(stop_rows)
    reason = map_finalization_reason(extract_predicate_value(final_stop))
    finalization = format_finalization_record(
        count_rows(stop_rows),
        get_last_row(timeline),
        reason,
    )
    return format_evaluation_result(str(validated["scenario_id"]), timeline, finalization)


# role: mapper
def bundled_scenario_paths(eval_root: Path) -> list[Path]:
    return [
        eval_root / "fixtures" / "scenarios" / f"{scenario_id}.json"
        for scenario_id in SCENARIO_IDS
    ]


# role: parser
def parse_cli_args(argv: list[str] | None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Replay AGE-105 completion-signal scenarios"
    )
    parser.add_argument("scenario_paths", nargs="*", type=Path)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--json", action="store_true")
    return parser.parse_args(argv)


# role: validator
def validate_cli_paths(args: argparse.Namespace) -> argparse.Namespace:
    if not args.dry_run and not args.scenario_paths:
        raise ValueError("provide scenario paths or pass --dry-run")
    for path in args.scenario_paths:
        if not path.is_file():
            raise ValueError(f"scenario path does not exist: {path}")
    return args


# role: mapper
def map_cli_scenario_paths(args: argparse.Namespace, eval_root: Path) -> list[Path]:
    if args.dry_run:
        return bundled_scenario_paths(eval_root)
    return list(args.scenario_paths)


# role: orchestration
def evaluate_scenario_paths(paths: Iterable[Path]) -> list[dict[str, Any]]:
    results = []
    for path in paths:
        results.append(evaluate(load_scenario(path)))
    return results


# role: formatter
def dump_result_json(result: dict[str, Any]) -> str:
    return json.dumps(result, sort_keys=True)


# role: formatter
def format_cli_json_lines(results: Iterable[dict[str, Any]]) -> list[str]:
    return [dump_result_json(result) for result in results]


# role: orchestration
def emit_lines(lines: Iterable[str]) -> None:
    for line in lines:
        print(line)


# role: orchestration
def main(argv: list[str] | None = None) -> int:
    args = validate_cli_paths(parse_cli_args(argv))
    eval_root = Path(__file__).resolve().parent
    paths = map_cli_scenario_paths(args, eval_root)
    emit_lines(format_cli_json_lines(evaluate_scenario_paths(paths)))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
