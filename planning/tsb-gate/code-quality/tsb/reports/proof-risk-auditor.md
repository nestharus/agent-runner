# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | phase-6 per-component code-quality | n/a | n/a | Caller-supplied Phase 6 context; contract read before scoring per `code-quality.md:169-173`. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Used to resolve referenced evidence paths. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/proposal.md` | 6493 | `28822d1c87db` | Read before scoring. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/contracts/tsb.contract.md` | 20219 | `2198f4bdbe2a` | Read before scoring; declares touched files, adapter surfaces, intrinsic surfaces, and test harnesses. |
| code-quality convention | `/home/nes/ai/conventions/code-quality.md` | 30798 | `fa8b6499cc2e` | Read as required by caller. |
| evidence log | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/evidence/runtime-tests.log` | 1171 | `d9b35ce0e04f` | Read for cited proof-method execution context. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/diff.patch` | 51948 | `2ae44c339ae2` | Read as Phase 6 delta context. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/touched-files.txt` | 210 | `28440caadc65` | Read as Phase 6 delta context. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| Exact `## Proof plan` section | Yes | `proposal.md:7` opens the exact section. |
| Runtime claim | Yes | Runtime claims are stated at `proposal.md:11`, `proposal.md:17`, `proposal.md:23`, `proposal.md:29`, `proposal.md:35`, `proposal.md:41`, `proposal.md:47`, `proposal.md:53`, `proposal.md:59`, and `proposal.md:65`. |
| Proof method | Yes | Proof methods are named at `proposal.md:13`, `proposal.md:19`, `proposal.md:25`, `proposal.md:31`, `proposal.md:37`, `proposal.md:43`, `proposal.md:49`, `proposal.md:55`, `proposal.md:61`, and `proposal.md:67`. |
| Evidence-class match | Yes | Evidence-class matches are stated at `proposal.md:15`, `proposal.md:21`, `proposal.md:27`, `proposal.md:33`, `proposal.md:39`, `proposal.md:45`, `proposal.md:51`, `proposal.md:57`, `proposal.md:63`, and `proposal.md:69`. |
| Self-certification only | No | The proof plan names concrete shell/Rust proof surfaces rather than relying only on generic pass statements; the separate evidence log lists executed gates at `runtime-tests.log:7-11`. |

## Findings
| Finding ID | Severity | Runtime claim | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|
| None | LOW | No missing proof-plan field, self-certification, proxy-only runtime proof, or evidence-class mismatch found. | All runtime claims have named proof methods. | Mock/fake OpenCode use is scoped to the declared public CLI contract, not substituted for a real-OpenCode availability claim. | `scripts/opencode-turns`, `crates/oulipoly-runtime/src/sessions/mod.rs`, and `crates/oulipoly-runtime/src/quota/process.rs` as declared in the contract. | `proposal.md:7-69`; `tsb.contract.md:144-170`; `tsb.contract.md:172-205`; `tsb.contract.md:209-232`. | No |

## Evidence-class decision
| Runtime claim ref | Required evidence class | Proof method class | Decision |
|---|---|---|---|
| `proposal.md:11` | Runtime adapter/integration evidence for the shipped OpenCode turn adapter using the public CLI and runtime JSONL ingest contract. | `proposal.md:13-15` names a concrete Rust integration test with a fake OpenCode accepting `session list --json` and `export ses_fixture`. | Match. The contract declares `scripts/opencode-turns` as translating the public OpenCode CLI surface and JSONL contract at `tsb.contract.md:146-154`, and the fake CLI fixture is scoped to that public command-shape contract rather than private storage. |
| `proposal.md:17` | Runtime adapter evidence for bounded implicit discovery in `scripts/opencode-turns`. | `proposal.md:19-21` names the shell integration test that invokes the adapter with timestamped mock sessions and verifies recent-window export selection. | Match. The contract declares `OPENCODE_TURNS_WINDOW_HOURS` and deadline/timestamp ownership for the adapter at `tsb.contract.md:176-187`, and the test harness surface explicitly covers adapter invocation, mock OpenCode CLI, env options, and stdout/export-log assertions at `tsb.contract.md:223-232`. |
| `proposal.md:23` | Runtime adapter evidence for timeout/degraded behavior that prevents the adapter from wedging pre-dispatch work. | `proposal.md:25-27` names the shell integration test with short call timeout/deadline, exit-zero assertion, degraded marker, partial count, and elapsed-time bound. | Match. The adapter owns internal OpenCode CLI call deadline and degraded-marker behavior per `tsb.contract.md:170` and `tsb.contract.md:176-187`; the proof method exercises the shipped script boundary rather than a static parser-only proxy. |
| `proposal.md:29` | Runtime session-ingest evidence for degraded marker handling in `crates/oulipoly-runtime/src/sessions/mod.rs`. | `proposal.md:31-33` names a Rust test scanning `{"degraded":true,"count":1}` and checking degradation is reported without malformed-turn error. | Match. The contract declares degraded-marker recognition and session-turn stdout ingestion as intrinsic/runtime-owned surfaces at `tsb.contract.md:188-197`; the proof method calls production session-ingest code. |
| `proposal.md:35` | Runtime session-script deadline evidence for timeout classification and conservative no-persist behavior. | `proposal.md:37-39` names a Rust test using `scan_provider_with_timeout(..., 1)` against `sleep 60`, then checking no turns persisted and `script_timeout` errors are reported. | Match. The contract assigns session script execution deadline, timeout token formatting, and StateDb ingest to `sessions/mod.rs` at `tsb.contract.md:156-161` and `tsb.contract.md:188-197`; the proof method exercises production code with a real shell timeout path. |
| `proposal.md:41` | Runtime quota-script deadline evidence for stable timeout classification. | `proposal.md:43-45` names a Rust test using `run_script_with_timeout("sleep 60", 1)` and checking `script_timeout` plus quota-script text. | Match. The contract assigns quota/auth shell command execution and `script_timeout` formatting to `quota/process.rs` at `tsb.contract.md:162-168` and `tsb.contract.md:197-205`; the proof method targets that production timeout path. |
| `proposal.md:47` | Runtime quota-script process-group kill evidence on Unix. | `proposal.md:49-51` names a Unix-only Rust test where a timed-out quota script starts a background marker writer and the marker does not appear after timeout cleanup. | Match. The contract declares quota process-group kill ownership at `tsb.contract.md:197-205`; the proof method exercises an OS process-group descendant behavior, not a mocked kill result. |
| `proposal.md:53` | Runtime adapter evidence for timestampless discovery cap fallback in `scripts/opencode-turns`. | `proposal.md:55-57` names the shell integration test with five timestampless sessions, `OPENCODE_TURNS_MAX_SESSIONS=3`, and exact export/stdout assertions. | Match. The contract declares `OPENCODE_TURNS_MAX_SESSIONS` and adapter option ownership at `tsb.contract.md:176-187`; the proof method runs the adapter under declared env-option and mock-CLI harness surfaces. |
| `proposal.md:59` | Runtime adapter evidence for process-group descendant cleanup on OpenCode CLI timeout. | `proposal.md:61-63` names the shell integration test where the mock export spawns a same-process-group descendant, wedges, and assertions verify degraded output and stopped/non-running descendant. | Match. The contract declares Python stdlib process-group kill ownership for `scripts/opencode-turns` at `tsb.contract.md:176-187`; the proof method verifies the runtime process behavior through the shipped script. |
| `proposal.md:65` | Runtime session-script process-group kill evidence on Unix. | `proposal.md:67-69` names a Unix-only Rust test where a timed-out turn script starts a shell grandchild marker writer and the marker does not exist after timeout cleanup. | Match. The contract declares session script process-group kill ownership at `tsb.contract.md:188-197`; the proof method exercises real shell grandchild behavior through production session script execution code. |

The proof plan uses mixed evidence where appropriate: fake OpenCode surfaces are declared test-harness/public-contract fixtures, while the production claims are bound to the shipped adapter script or runtime Rust modules declared in the Phase 6 contract. The plan does not claim real OpenCode service availability, built-container startup, deployed-service behavior, or production DB migration success; therefore the absence of those evidence classes is not a mismatch for the stated claims.

## Residual ambiguity / stop-condition notes
No stop condition fired. `contract_path` was readable and nonblank, satisfying the Phase 6 requirement in `code-quality.md:169-173` before scoring.

`proposal.md:45` and `proposal.md:51` state that the two quota timeout tests are shipped but not individually shown in `runtime-tests.log`; the log still records `cargo test --workspace` at `runtime-tests.log:11`. This is execution-evidence granularity, not proof-plan evidence-class mismatch, because the proof methods and claim/artifact bindings are explicitly named in the proposal.

VERDICT: LOW
