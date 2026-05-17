# AGE-105 Claude Completion-Signal Eval

This eval is a replay-only contract for the corrected Claude completion
predicate:

```text
Stop fired for the current turn
AND active tool counter == 0
AND pending background agent count == 0
AND pending scheduled wakeup count == 0
```

The evaluator is intentionally local to `evals/claude-completion-signal/`.
It does not change runtime, trace, state, Tauri, or GUI behavior. Its job is to
pin the evidence shape for future completion work and reject any three-input
predicate that omits the background-agent counter.

## Scenarios

| Scenario | Required behavior |
|---|---|
| `clean_foreground` | A single foreground Stop finalizes when all counters are zero. |
| `foreground_tool` | `PreToolUse` increments the active foreground counter, `PostToolUse` drains it, and Stop finalizes. |
| `one_background` | The first Stop is a checkpoint while one background agent remains pending; a later Stop finalizes after `SubagentStop`. |
| `five_background` | Stops after partial background drains remain checkpoints until all five workers have stopped. |
| `scheduled_wakeup` | The first Stop after scheduling is a checkpoint while a wakeup remains pending; the wakeup turn's Stop finalizes. |

## Output Contract

`evaluate-predicate.py` returns one JSON-like object per scenario with:

- `scenario_id`
- `stop_count`
- `timeline`, including every event row with `seq`, `event`,
  `active_tool_after`, `pending_background_agent_after`,
  `pending_scheduled_wakeup_after`, and `predicate_after`
- `finalization`, including `reason`, stop count, and final counter values

`eval.sh --dry-run --json` emits five JSON result objects, one per bundled
scenario.

## Blocking Checks

Run from this directory:

```bash
python3 -m unittest contract_tests
bash run-tests.sh
bash eval.sh --dry-run --json
```

The replay fixtures are the blocking proof. Live Claude behavior is outside
this eval root and is not required for AGE-105.
