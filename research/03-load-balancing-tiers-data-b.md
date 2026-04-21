# Data Probe B — Caller Environment Survey

Scope searched:

- `/home/nes/work`
- `/home/nes/projects/server-manager`
- `/home/nes/projects/agent-runner`
- `/home/nes/projects/*/AGENTS.md`
- `/home/nes/.claude`
- `/home/nes/.codex`

Concrete shell observations:

- `command -v claude` returned `/home/nes/.local/bin/claude`.
- `command -v codex` returned `/home/nes/.npm-global/bin/codex`.
- `command -v agents` returned `/home/nes/.local/bin/agents`.
- `/home/nes/.claude/commands` exists, but `rg -n 'agents|uv run agents|~/.local/bin/agents' /home/nes/.claude/commands` returned no matches.
- `/home/nes/.claude/agents` is absent.
- `/home/nes/.codex/commands` is absent.
- `find /home/nes/.codex -maxdepth 3 \( -path '*/commands/*' -o -path '*/mcp*' -o -path '*/plugins/*' \) -type f` returned only `/home/nes/.codex/.tmp/plugins/.gitignore` and `/home/nes/.codex/.tmp/plugins/README.md`.

## 6.1 — Current invocation patterns of `agents`

The command-bearing occurrences below are grouped by file and line range. Classification is limited to what the text itself shows.

| Location | Evidence | Shell form(s) present | Explicit env vars in shell form | Classification from text | Distinguishing signals |
|---|---|---|---|---|---|
| `/home/nes/work/AGENTS.md` | 156-175 | `agents -m gpt-high -p /home/nes/work/rfqautomation-linux "your prompt here"`; `agents -m claude-opus -p /home/nes/work/rfqautomation-linux "your prompt here"`; `agents -m gpt-high -p /home/nes/work/rfqautomation-linux -f prompt.md`; `agents my-agent "prompt args"` | None | Interactive call / unclear examples | Positional prompt on argv; `-p`; `-f`; named agent |
| `/home/nes/work/AGENTS.md` | 285-321 | `agents --model gpt-high --file prompt.md`; `agents --agent-file ~/work/agents/e2e-operator.md --model claude-opus --file prompt.md`; `cat spec.md \| agents --model glm`; `agents --model gpt-high -p /path/to/repo --file prompt.md` | None | Unclear examples | `--file`; `--agent-file`; piped stdin; `-p` |
| `/home/nes/work/AGENTS.md` | 360-395 | `agents -m seedream-t2i "A sunset over mountains" > sunset.jpeg`; `agents -m seedance-t2v-low -i duration=5 -i resolution=480p "A whale swimming" > whale.mp4`; `agents -m seedance-i2v-fast -i image=./photo.jpg "Slow camera orbit" > orbit.mp4`; `agents -m seedream-i2i -i image=input.png "Make it warmer" > edited.jpeg`; `~/.local/bin/agents --model gpt-xhigh --file "$1" > "$2" 2>&1` | None | Mixed: foreground binary-producing calls; background-wrapper example | `-i`; stdout redirected to binary file; wrapper script with stdout+stderr redirection |
| `/home/nes/work/WORKFLOW_STEP_LOG.md` | 9-25, 30-34 | Observed dispatches: `~/.local/bin/agents --agent-file /home/nes/work/agents/agentsmd-curator.md -p /home/nes/work -f /home/nes/work/AGENTSMD_AUDIT.prompt.md > /home/nes/work/AGENTSMD_AUDIT.log 2>&1`; `agents --model claude-opus -p /home/nes/work -f <prompt>` via `run_in_background`; repeated curator re-audit/edit invocations | None | Background workflow / observed past run | Absolute `--agent-file`; `-p`; `-f`; `> ... 2>&1`; explicit background orchestration text |
| `/home/nes/work/.run-rebase-436-437.sh` | 1-3 | `~/.local/bin/agents --agent-file /home/nes/work/agents/jj-operator.md -p /home/nes/work -f /home/nes/work/REBASE_436_437.prompt.md > "$LOG" 2>&1` | None | Background/automation shell script | Absolute `--agent-file`; log redirection |
| `/home/nes/work/agents/agentsmd-maintenance-orchestrator.md` | 33-40, 44-53, 81-105 | Foreground curator dispatch: `~/.local/bin/agents --agent-file ~/work/agents/agentsmd-curator.md -p /home/nes/work -f <prompt-file>`; parallel risk calls: `~/.local/bin/agents --model claude-opus -p /home/nes/work -f <audit-risk-prompt> &` / scope / shortcut; foreground edit dispatch with `--agent-file` again | None | Background workflow | Explicit “All sub-agents via agents CLI”; `&` + `wait`; absolute path usage |
| `/home/nes/work/agents/fastapi-review-operator.md` | 175-179, 228-232, 282-286, 337-341, 391-395, 475-486 | Five background facet runs: `agents -m claude-opus -p "$PROJECT_DIR" -f "$WORK_DIR/facet-*.md" > "$WORK_DIR/result-*.md" 2>&1 &`; proposal run: `agents -m gpt-high -p "$PROJECT_DIR" -f "$WORK_DIR/proposal.md" > "$WORK_DIR/result-proposal.md" 2>&1` | None | Background workflow | `-p`; `-f`; per-phase result files; explicit `wait` |
| `/home/nes/work/agents/pr-review-operator.md` | 126-126, 173-173, 210-210, 252-252, 265-283, 332-367, 486-486, 532-534 | Foreground redirected runs: `agents -m claude-opus -p "$PROJECT_DIR" -f "$WORK_DIR/risk-*.md" > "$WORK_DIR/result-*.md" 2>&1`; `agents -m gpt-high -p "$PROJECT_DIR" -f "$WORK_DIR/research.md" > "$WORK_DIR/result-research.md" 2>&1`; background gate kickoff: `agents -a /home/nes/work/agents/test-audit-gate.md -p "$PROJECT_DIR" -f "$WORK_DIR/test-audit-kickoff.md" > "$WORK_DIR/TEST_AUDIT_GATE.md" 2>&1 &`; foreground gauntlet kickoff: `agents -a /home/nes/work/agents/pr-justification-gauntlet.md -m claude-opus -p "$PROJECT_DIR" -f "$WORK_DIR/gauntlet-kickoff.md" > "$WORK_DIR/result-justification.md" 2>&1`; proposal/domain research runs | None | Background workflow | Mixed `-m` and `-a`; heavy use of kickoff prompt files; one explicit background launch |
| `/home/nes/work/agents/test-audit-gate.md` | 199-214 | `agents -m gpt-high -p "$project_dir" -f "$scratch_dir/TEST_AUDIT_SPEC.prompt.md" > "$scratch_dir/TEST_AUDIT_SPEC.md" 2>&1 &`; `agents -a /home/nes/work/agents/coverage-auditor.md -p "$project_dir" -f "$scratch_dir/TEST_AUDIT_QUALITY.prompt.md" > "$scratch_dir/TEST_AUDIT_QUALITY.md" 2>&1 &`; `agents -a /home/nes/work/agents/coverage-analyzer.md -p "$project_dir" -f "$scratch_dir/TEST_AUDIT_COVERAGE.prompt.md" > "$scratch_dir/TEST_AUDIT_COVERAGE.md" 2>&1 &` | None | Background workflow | Three parallel subprocesses; explicit PID capture and `wait` |
| `/home/nes/work/agents/pr-justification-gauntlet.md` | 154-157, 190-192, 214-216, 242-244 | `agents -a /home/nes/work/agents/pr-justification-interrogator.md -m claude-opus -p "$project_dir" -f "$RD/interrogator-prompt.md" > "$RD/interrogator-result.md" 2>&1`; same pattern for researcher, value assessor, adjudicator | None | Background workflow | Per-round orchestration; agent-file absolute paths; result-file redirection |
| `/home/nes/work/agents/pr-justification-value-assessor.md` | 93-101 | `agents -m gpt-high -p "$project_dir" -f <prompt_file> > <result_file>` | None | Background workflow / sub-agent example | Narrow factual sub-agent from inside another workflow |
| `/home/nes/work/agents/qa-operator.md` | 101-112 | `cd <worktree_path>` then `~/.local/bin/agents --model gemini-high -p <worktree_path> --file /tmp/qa-<slug>/qa-prompt.md > /tmp/qa-<slug>/qa-results.md 2>&1` | None | Foreground call; surrounding text also mentions background runs | `-p`; `--file`; result-file redirection |
| `/home/nes/work/agents/pipeline-artifacts-operator.md` | 137-146 | Wrapper script: `~/.local/bin/agents --model gpt-xhigh --file "$1" > "$2" 2>&1` | None | Background-wrapper example | Script wrapper inside worktree; stdout+stderr to artifact file |
| `/home/nes/projects/server-manager/AGENTS.md` | 399-405, 417-442 | `~/.local/bin/agents -m gpt-high -p <project> -f .tmp/<research-prompt>.md`; `agents -m gpt-high -p <worktree-path> "your prompt"`; `agents -m gpt-high -p <worktree-path> -f prompt.md`; `agents -m gpt-high -p worktrees/<branch> -f prompt.md` | None | Mixed: coordinator-launched workflows and interactive-call examples | Worktree isolation text; inline prompt and prompt-file variants |
| `/home/nes/projects/server-manager/product-strategy/roadmap orchestrator.md` | 133-140, 155-161, 191-197, 227-231, 271-277, 291-295, 338-344, 379-385 | Six parallel market-research calls `agents -m gpt-high -p worktrees/research-* -f .tmp/research-*.md &`; three parallel risk calls `agents -m claude-opus -p worktrees/exec-risk-* -f .tmp/executive-risk-*.md &`; multiple foreground stage calls with `-p <worktree> -f <agent.md>` | None | Background workflow | Explicit `&` + `wait`; stage-by-stage orchestration text |
| `/home/nes/projects/server-manager/prototyping/orchestrator.md` | 69-72, 86-89, 104-107, 122-125 | `agents -m gpt-high -p <worktree> -f prototyping/research\ agent.md`; `agents -m gpt-high -p <worktree> -f prototyping/prototype\ agent.md`; `agents -m claude-opus -p <worktree> -f prototyping/findings\ agent.md`; `agents -m gpt-high -p <worktree> -f prototyping/roadmap-update\ agent.md` | None | Background workflow | Worktree-scoped sequential stages |
| `/home/nes/projects/visual-code-editor/AGENTS.md` | 112-115, 167-169, 223-230 | `agents --model gemini-high --file /tmp/svg-prompt.md --project ~/projects/visual-code-editor`; `agents --model gpt-high --file /tmp/research-prompt.md --project ~/projects/visual-code-editor`; `agents --model gemini-video-high -p ~/projects/visual-code-editor "Review the animation video at ..."` | None | Interactive call / unclear examples | Direct argv prompt; no redirection; project-scoped |
| `/home/nes/projects/agent-implementation-skill/AGENTS.md` | 29-68, 137-158, 309-324 | `agents --model gpt-high --file prompt.md`; `agents --agent-file src/staleness/agents/alignment-judge.md --model claude-opus --file prompt.md`; `agents code-reviewer "Review this function"`; `cat spec.md \| agents --model glm`; `agents --model gpt-high -p /path/to/repo --file prompt.md`; multimodal stdout-to-file examples; inline prose examples `agents --model glm --agent-file agents/eval-judge.md` and `agents --model gpt-high --file <prompt.md>` | None | Mixed: interactive-call examples and workflow examples | Same structural shapes as `~/work/AGENTS.md`; plus multimodal `-i` output |
| `/home/nes/projects/agent-implementation-skill/execution-philosophy/AGENTS.md` | 36-67, 127-136, 277-285, 603-606, 683-702 | `agents --model gpt-xhigh --file /tmp/audit-prompt.md --project .`; `agents --model gpt-high "Summarize the key findings"`; `cat spec.md \| agents --model glm`; `agents --model seedream-t2i "A detailed technical diagram..."`; `agents --model seedream-i2i '{"prompt": ..., "image": ...}'`; `agents --model gpt-xhigh --file /tmp/audit-concern-name.md --project .`; `agents --model gemini-high --file /tmp/diagram-prompt.md --project .`; `agents --model seedance-i2v '{"prompt": ..., "image": ...}'`; `agents --model seedream-i2i -i image=diagrams/concept-art/fig-combined.jpg -i size=2848*1600 "prompt..." > diagrams/fig-output.png` | None | Mixed: interactive-call examples and workflow examples | Inline JSON prompt variant; `-i`; project-scoped audit calls |
| `/home/nes/projects/agent-runner/README.md` | 108-192, 324-365, 624-624 | Direct binary docs use `oulipoly-agent-runner`, not `agents`: one-shot examples, stdin example, multimodal stdout redirection, `trace`, and `repl`; one current `agents` example remains at 624: `agents -m seedream-t2i "A cat" > cat.jpeg` | None | Mixed: direct-binary examples and one `agents` example | Self-hosted examples expose both one-shot and `repl` shapes |
| `~/.claude` / `~/.codex` | Shell observations above | No command file or settings hit shelling out to `agents`; no `.claude/commands` match; no `.codex/commands` directory; Codex shell snapshots do contain inherited `OULIPOLY_PARENT_INVOCATION` (for example `/home/nes/.codex/shell_snapshots/019da9fb-1b06-7961-b5b0-a6143fd83ad5.1776673299240121397.sh:141`) | Shell snapshot contains inherited env only | No `agents` call site found in command/config search | Config-dir search did not surface a slash-command or MCP launcher for `agents` |

## 6.2 — Parent-invocation env var behavior today

### Where the env vars are read

| Env var | Evidence | Current behavior |
|---|---|---|
| `OULIPOLY_PARENT_INVOCATION` | `/home/nes/projects/agent-runner/src-tauri/src/main.rs:706-714` | Reads the raw env var, parses it as a `CompositeInvocationId`, resolves it against the state DB, and returns `Some(record.id)` only when both UUID lookup and provider/source match succeed. |
| `OULIPOLY_INVOCATION` | `/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:183-190` | Not read from env by the runner; formatted as a stderr line via `stderr_line()`. The current README shows shell capture of that stderr line at `/home/nes/projects/agent-runner/README.md:324-337`. |

### Where the env vars are set / written

| Env var | Evidence | Current behavior |
|---|---|---|
| `OULIPOLY_PARENT_INVOCATION` | `/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:241-265` | `build_command()` writes `cmd.env("OULIPOLY_PARENT_INVOCATION", parent_invocation_env)` when a parent payload is supplied. |
| `OULIPOLY_PARENT_INVOCATION` | `/home/nes/projects/agent-runner/src-tauri/src/main.rs:639-646` and `/home/nes/projects/agent-runner/src-tauri/src/executor/mod.rs:78-94` | The one-shot path serializes the current invocation and passes it into `execute_with_inputs_and_env(...)`, which forwards it into the executor. |
| `OULIPOLY_PARENT_INVOCATION` | `/home/nes/projects/agent-runner/src-tauri/src/main.rs:558-563` | The interactive `repl` path passes the current invocation env payload into `execute_interactive(...)`. |
| `OULIPOLY_INVOCATION` | `/home/nes/projects/agent-runner/src-tauri/src/main.rs:624-637` and `/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:183-190` | The one-shot path serializes the current invocation and prints `eprintln!("{}", invocation.stderr_line())`. |
| `OULIPOLY_INVOCATION` | `/home/nes/projects/agent-runner/src-tauri/src/main.rs:535-556` | The `repl` path serializes the current invocation and conditionally emits the stderr line. |

### Observed shell presence outside the repo

- `/home/nes/.codex/shell_snapshots/019da9fb-1b06-7961-b5b0-a6143fd83ad5.1776673299240121397.sh:141`
- `/home/nes/.codex/shell_snapshots/019daf8a-191e-7693-94cf-bff52e680acd.1776766556478074466.sh:140`
- `/home/nes/.codex/shell_snapshots/019daa24-0e23-7530-a275-c4041a155a57.1776675982908105080.sh:136`
- `/home/nes/.codex/shell_snapshots/019da28a-3141-72b0-999f-38e6362ae3bc.1776548458846346905.sh:149`
- `/home/nes/.codex/shell_snapshots/019da28d-9f91-7851-9d8c-bedf8bf52f5c.1776548683686093416.sh:149`

Each of those files contains `declare -x OULIPOLY_PARENT_INVOCATION="{...}"`.

### Evidence on whether `OULIPOLY_PARENT_INVOCATION` means “background-launched”

Current source evidence ties the variable to runner-to-runner parent propagation, not to a background-only path:

- The one-shot path passes it (`/home/nes/projects/agent-runner/src-tauri/src/main.rs:639-646`).
- The interactive `repl` path passes it (`/home/nes/projects/agent-runner/src-tauri/src/main.rs:558-563`).
- The executor writes it onto child `Command`s whenever a payload is provided (`/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:241-265`).
- The README describes it as cross-invocation tracking when one runner invokes another, with Claude Code `Task` given only as an example, not as an exclusive case (`/home/nes/projects/agent-runner/README.md:362-366`).

Shell search in `~/work`, `~/projects/server-manager`, `~/.claude/commands`, and `~/.codex` did not surface a workflow script that manually exports `OULIPOLY_PARENT_INVOCATION`; the non-test hits outside source were README prose and Codex shell snapshots.

## 6.3 — TTY / interactive-mode detection

### `agents` / `oulipoly-agent-runner`

| Evidence | Current use of the TTY signal |
|---|---|
| `/home/nes/projects/agent-runner/src-tauri/src/main.rs:165-188` | `resolve_prompt()` checks `std::io::stdin().is_terminal()`. If stdin is a TTY and there is no positional prompt and no `--file`, it returns `No prompt provided...`. If stdin is not a TTY, it reads stdin into the prompt string. |
| `/home/nes/projects/agent-runner/src-tauri/src/main.rs:460-489` | `run_repl()` checks `std::io::stderr().is_terminal()` and uses that to decide whether to print the invocation line and the detailed resume line. |
| `/home/nes/projects/agent-runner/src-tauri/src/main.rs:381-395` | `should_emit_invocation_line(is_terminal)` returns `!is_terminal`; `should_emit_resume_detail_line(...)` is also gated on `!is_terminal`; `should_emit_resume_short_line(...)` always returns `true`. |
| `/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:344-389` | The interactive executor inherits stdin/stdout/stderr with `Stdio::inherit()`; the one-shot executor uses piped stdio or null stdin depending on prompt mode (`/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:293-330`). |

### Wrapped CLI behavior observed from the local environment

| Wrapped CLI | Evidence | Observed interactive/non-interactive split |
|---|---|---|
| Claude CLI | Shell observation: `claude --help` prints `Claude Code - starts an interactive session by default, use -p/--print for non-interactive output` | Help text exposes separate interactive default vs `-p/--print` one-shot mode. |
| Claude model config in this environment | `/home/nes/.config/oulipoly-agent-runner/models/claude-opus.toml:1-14` | Current one-shot provider args use `claude -p --model opus ...` (and `claude2` / `claude3` variants), which matches the non-interactive help text. |
| Codex CLI | Shell observation: `codex --help` prints `If no subcommand is specified, options will be forwarded to the interactive CLI.` It also lists `exec` as `Run Codex non-interactively` and `resume` as a separate command. | Help text exposes separate interactive default vs `exec` non-interactive subcommand. |
| Codex model config in this environment | `/home/nes/.config/oulipoly-agent-runner/models/gpt-high.toml:1-9` | Current one-shot provider args use `codex exec --dangerously-bypass-approvals-and-sandbox -m gpt-5.4 ...`, which matches the non-interactive help text. |

## 6.4 — Existing flags and env vars available

### CLI flags and positional args in the current parser

Source: `/home/nes/projects/agent-runner/src-tauri/src/main.rs:17-61` and `/home/nes/projects/agent-runner/src-tauri/src/main.rs:63-105`.

| Scope | Flag / arg | Evidence |
|---|---|---|
| Main CLI | positional `agent` | `main.rs:27-28` |
| Main CLI | positional `prompt_args...` | `main.rs:30-32` |
| Main CLI | `-m`, `--model` | `main.rs:34-36` |
| Main CLI | `-a`, `--agent-file` | `main.rs:38-40` |
| Main CLI | `-f`, `--file` | `main.rs:42-44` |
| Main CLI | `-p`, `--project` | `main.rs:46-48` |
| Main CLI | `--models-dir` | `main.rs:50-52` |
| Main CLI | `--agents-dir` | `main.rs:54-56` |
| Main CLI | `-i`, `--input KEY=VALUE` | `main.rs:58-60` |
| `trace` subcommand | positional `invocation_uuid` | `main.rs:65-68` |
| `trace` subcommand | `--json` | `main.rs:70-72` |
| `trace` subcommand | `--inline-transcript` | `main.rs:74-76` |
| `trace` subcommand | `--transcript` | `main.rs:78-83` |
| `trace` subcommand | `--max-depth` | `main.rs:85-87` |
| `repl` subcommand | positional `model` | `main.rs:89-93` |
| `repl` subcommand | `--resume` | `main.rs:94-97` |
| `repl` subcommand | `-p`, `--project` | `main.rs:98-100` |
| `repl` subcommand | `--models-dir` | `main.rs:102-104` |

### Env vars explicitly read by the current source

Searches used: `rg -n 'env::var\\(|env::var_os\\(' /home/nes/projects/agent-runner/src-tauri/src` and `rg -n 'std::env::var\\(|std::env::var_os\\(' ...`.

| Env var | Evidence | Runtime or test-only |
|---|---|---|
| `OULIPOLY_PARENT_INVOCATION` | `/home/nes/projects/agent-runner/src-tauri/src/main.rs:706-714` | Runtime |
| `OPENAI_API_KEY` | `/home/nes/projects/agent-runner/src-tauri/src/setup/detection.rs:342-345` | Runtime |
| `XDG_CONFIG_HOME` | `/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:1901-1920` and `2246-2260` | Test-only helper/setup code in `state/db.rs` tests |

No other explicit `env::var(...)` / `env::var_os(...)` reads were returned by those source-tree searches.

## 6.5 — Caller categories observed in 6.1

Sample sizes below count concrete shell/code-block command lines that begin with `agents`, `~/.local/bin/agents`, or `cat ... | agents` in the scoped files. Usage-syntax lines such as `agents [OPTIONS] [AGENT] [PROMPT...]` are excluded from the cluster counts, and inline prose mentions such as `Launch: \`agents ...\`` are not part of this count.

| Cluster | Prototype command line | Explicit env vars in shell form | Fire-and-forget vs waits | stdin state | Sample size | Evidence anchor(s) |
|---|---|---|---|---|---|---|
| cluster A | `agents -m <model> -p <path> -f <prompt.md> [&]` and `agents -m <model> -p <path> -f <prompt.md> > <result> 2>&1 &` | None | Fire-and-forget until later `wait` / PID join | No stdin redirection shown | 18 | `/home/nes/projects/server-manager/product-strategy/roadmap orchestrator.md:133-140,227-231`; `/home/nes/work/agents/agentsmd-maintenance-orchestrator.md:85-92`; `/home/nes/work/agents/fastapi-review-operator.md:175-179,228-232,282-286,337-341,391-395`; `/home/nes/work/agents/test-audit-gate.md:203-213` |
| cluster B | `agents -m <model> -p <path> -f <prompt.md> > <result> 2>&1` | None | Awaits command exit | No stdin redirection shown | 4 | `/home/nes/work/agents/fastapi-review-operator.md:480-482`; `/home/nes/work/agents/pr-justification-value-assessor.md:96-98`; `/home/nes/work/agents/qa-operator.md:103-107`; `/home/nes/work/agents/pr-review-operator.md:533-534` |
| cluster C | `agents <named-agent> "prompt args"` or `agents --model <model> "prompt" [> output]` | None | Awaits command exit | Prompt on argv; no stdin pipe shown | 11 | `/home/nes/work/AGENTS.md:169,368,380`; `/home/nes/projects/agent-implementation-skill/AGENTS.md:61,145,157`; `/home/nes/projects/agent-implementation-skill/execution-philosophy/AGENTS.md:64,131,136,685,702` |
| cluster D | `agents -m <model> -i <key=value> ... "prompt" > <binary>` | None | Awaits command exit | Prompt on argv; no stdin pipe shown | 9 | `/home/nes/work/AGENTS.md:367-381`; `/home/nes/projects/agent-implementation-skill/AGENTS.md:143-158`; `/home/nes/projects/agent-implementation-skill/execution-philosophy/AGENTS.md:688-693` |
| cluster E | `agents -m <model> -p <path> -f <prompt.md>` | None | Awaits command exit | No stdin redirection shown | 21 | `/home/nes/work/AGENTS.md:166,320`; `/home/nes/projects/server-manager/AGENTS.md:404-405,426,442`; `/home/nes/projects/server-manager/product-strategy/roadmap orchestrator.md:160,196,276,294,343,384`; `/home/nes/projects/server-manager/prototyping/orchestrator.md:72,89,107,125`; `/home/nes/projects/visual-code-editor/AGENTS.md:114,168`; `/home/nes/projects/agent-implementation-skill/AGENTS.md:67`; `/home/nes/projects/agent-implementation-skill/execution-philosophy/AGENTS.md:61,278,605` |
| cluster F | `agents --agent-file <path> ... -f <prompt.md> > <log> 2>&1` or `agents -a <path> ... -f <prompt.md> > <result> 2>&1` | None | Awaits command exit | No stdin redirection shown | 6 | `/home/nes/work/.run-rebase-436-437.sh:1-3`; `/home/nes/work/agents/pr-justification-gauntlet.md:155-157,190-192,214-216,242-244`; `/home/nes/work/agents/pr-review-operator.md:364-367` |
| cluster G | `agents -m <model> -p <path> "your prompt"` | None | Awaits command exit | Prompt on argv; no stdin pipe shown | 4 | `/home/nes/work/AGENTS.md:162-163`; `/home/nes/projects/server-manager/AGENTS.md:423`; `/home/nes/projects/visual-code-editor/AGENTS.md:227-230` |
| cluster H | `cat <file> | agents --model <model>` | None | Awaits command exit | stdin piped | 3 | `/home/nes/work/AGENTS.md:316-317`; `/home/nes/projects/agent-implementation-skill/AGENTS.md:63-64`; `/home/nes/projects/agent-implementation-skill/execution-philosophy/AGENTS.md:66-67` |
| cluster I | `agents -a <agent-file> -p <path> -f <prompt.md> > <result> 2>&1 &` | None | Fire-and-forget until later `wait` / PID join | No stdin redirection shown | 3 | `/home/nes/work/agents/pr-review-operator.md:280-283`; `/home/nes/work/agents/test-audit-gate.md:207-210` |
| cluster J | `agents --model <model> --file <prompt.md>` | None | Awaits command exit | No stdin redirection shown | 2 | `/home/nes/work/AGENTS.md:311`; `/home/nes/projects/agent-implementation-skill/AGENTS.md:55` |
| cluster K | `~/.local/bin/agents --model <model> --file "$1" > "$2" 2>&1` | None | Awaits command exit inside wrapper script | No stdin redirection shown | 2 | `/home/nes/work/AGENTS.md:389-395`; `/home/nes/work/agents/pipeline-artifacts-operator.md:137-146` |
| cluster L | `agents --agent-file <path> [--model <model>] [-p <path>] --file <prompt.md>` | None | Awaits command exit | No stdin redirection shown | 4 | `/home/nes/work/AGENTS.md:313-315`; `/home/nes/work/agents/agentsmd-maintenance-orchestrator.md:48-53,101-105`; `/home/nes/projects/agent-implementation-skill/AGENTS.md:57-58` |

## 6.6 — Existing workflows’ declared risk tolerance

### Direct documentation found

| Location | Evidence |
|---|---|
| `/home/nes/work/AGENTS.md:184-189` | Workflow table says: `3. Risk assessment ... All must return LOW. If MEDIUM/HIGH, revise proposal and re-run.` |
| `/home/nes/projects/server-manager/AGENTS.md:56-63` | Pipeline table says: `4. Risk assessment ... All must return LOW. If MEDIUM/HIGH, revise proposal and re-run.` |
| `/home/nes/projects/server-manager/AGENTS.md:102-105` | `If any is MEDIUM or HIGH, revise proposal and re-run the full risk gate — do not cherry-pick which risks to address`. |
| `/home/nes/projects/server-manager/AGENTS.md:315` | `Restart check — if major edits, re-run from step 4. Max 3 cycles.` |
| `/home/nes/projects/server-manager/AGENTS.md:372-375` | `All risk assessments must return LOW. If any returns MEDIUM/HIGH, the proposer revises and the full risk gate re-runs.` |
| `/home/nes/work/agents/agentsmd-maintenance-orchestrator.md:81-95` | Risk-gate section says to dispatch three parallel risk assessments, `wait`, and `All three must return LOW. If any returns MEDIUM/HIGH, revise the edit plan and re-run risk gate.` |

### Retry / rate-limit / backoff text found

| Location | Evidence |
|---|---|
| `/home/nes/work/agents/coderabbit-operator.md:120-127` | Decision table includes `Rate-limited | Sleep until clear, re-run same pass`. |
| `/home/nes/work/agents/test-audit-gate.md:256` | `Do not invent a fourth audit, a retry loop, or new infrastructure`. |
| `/home/nes/work/agents/e2e-operator.md:90` | `Do NOT use gh run watch — it polls every few seconds and burns API quota.` |

### What the search did not surface

Shell search `rg -n 'mid-call|tolerate|can tolerate' /home/nes/work/AGENTS.md /home/nes/projects/server-manager/AGENTS.md /home/nes/work/agents/*.md` did not return a passage that explicitly classifies workflows as “can tolerate mid-call failure” vs “cannot tolerate mid-call failure.”

The search did return re-run loops, restart caps, and one explicit rate-limit response (`Sleep until clear, re-run same pass`), but not a direct `User`/`Background`-style classification in the documents searched.
