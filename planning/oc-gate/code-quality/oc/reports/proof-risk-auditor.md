# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | `phase-3-proposal` | n/a | n/a | Valid mode. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Used to resolve supplied relative context. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/opencode-contract/gap-matrix.md` | 32975 | `61145931d3a4` | Read before scoring. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/contracts/oc.contract.md` | 16525 | `c8cac2b917a9` | Phase 6 contract read before scoring. |
| code-quality convention | `/home/nes/ai/conventions/code-quality.md` | 30798 | `fa8b6499cc2e` | Required convention read. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/gates/touched-surfaces.md` | 1156 | `6cbceb0ae602` | Read as context for scope. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/gates/diff.patch` | 81642 | `0f3998afb1a0` | Read as context for shipped fake-provider/proxy surfaces. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/code-quality/oc/reports/proof-risk-auditor.md` | n/a | n/a | This report is the only written path. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| `## Proof plan` | No | `planning/opencode-contract/gap-matrix.md` contains `## Verified Inputs`, `## Code Wiring Notes`, `## Gap Matrix`, `## Proposed OpenCode Config Shape After Code Support`, `## Prioritized Worklist`, `## Code Changes Needed`, and `## Safe Test Environment Pattern`, but no exact `## Proof plan` section. |
| `Runtime claim` | No | No required field exists. Candidate runtime topics appear only as matrix/worklist prose, e.g. capture/resume/wake/turns/terminal/routing rows in `planning/opencode-contract/gap-matrix.md:63-74` and `planning/opencode-contract/gap-matrix.md:185-193`. |
| `Proof method` | No | No required field exists. Test commands are listed in `Concrete test` table cells, e.g. `planning/opencode-contract/gap-matrix.md:63`, `planning/opencode-contract/gap-matrix.md:67-69`, `planning/opencode-contract/gap-matrix.md:71`, and `planning/opencode-contract/gap-matrix.md:74`, not in a proof-plan field. |
| `Evidence-class match` | No | No required field exists. `planning/opencode-contract/gap-matrix.md:161` says proof commands should use fake provider binaries and avoid real `agents`/`opencode`, but does not explain why that evidence class matches each runtime claim rather than a proxy surface. |

## Findings
| Finding ID | Severity | Runtime claim | Proof plan ref | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|---|
| PR-001 | HIGH | All claimed OpenCode P0/P1 runtime behaviors are unbound because the proposal has no exact proof-plan section. | Missing `## Proof plan`. | None in the required section. | Unclassified; fallback evidence is matrix test prose. | A `## Proof plan` section that binds claims to methods. | FILE `planning/opencode-contract/gap-matrix.md`: matrix/worklist/test prose exists at lines 58-78 and 105-193, but no exact proof-plan heading is present. | Yes |
| PR-002 | HIGH | The proposal does not name the runtime behavior asserted by the fix in a `Runtime claim` field. | Missing `Runtime claim`. | Concrete test rows imply targets but do not satisfy the required field. | Implicit fake-provider, temp-state, and unit/integration harness evidence. | One or more explicit runtime claims for session capture, resume/wake, terminal classification, turn ingest, and routing. | FILE `planning/opencode-contract/gap-matrix.md`: candidate claims are dispersed across launch/capture/resume/wake/turns/terminal/routing rows at lines 63-74 and the minimum acceptance flow at lines 185-193. | Yes |
| PR-003 | HIGH | The proposal does not name the validation surface that will exercise each runtime claim in a `Proof method` field. | Missing `Proof method`. | Matrix cells list individual `cargo test` commands, but not as the required proof method. | Fake provider scripts, mocked quota scripts, temp DB/config roots, and parser fixtures. | Explicit method list per runtime claim, including whether each method is runner-runtime, adapter fixture, or external OpenCode evidence. | FILE `planning/opencode-contract/gap-matrix.md`: concrete tests are listed in row cells at lines 63, 65, 67-69, 71, and 74; FILE `planning/oc-gate/gates/diff.patch`: shipped test surfaces include fake capture at lines 1261-1345, terminal classification at lines 867-917, turn ingest at lines 952-989, routing at lines 1050-1111, resume at lines 1855-1906, and wake at lines 2121-2165. | Yes |
| PR-004 | HIGH | The proposal does not explain why fake-provider/temp-state evidence exercises the asserted runtime claim rather than a proxy surface. | Missing `Evidence-class match`. | None in the required section; fallback prose says fake providers should be used. | Proxy-only unless explicitly scoped: fake provider binaries, fixture JSONL, fake quota scripts, temp state/config, and local adapter fixtures. | Evidence-class explanation per claim. Runner-owned launch/capture/resume/wake paths may be provable with fake providers, while real OpenCode event/auth/storage behavior needs external runtime evidence or explicit out-of-scope wording. | FILE `planning/opencode-contract/gap-matrix.md`: safe pattern requires fake provider binaries and no real `agents`/`opencode` at line 161; fake `opencode1` behavior and acceptance flow are listed at lines 175-193 without an evidence-class match statement. | Yes |
| PR-005 | HIGH | Real OpenCode `--format json` event/session behavior, including `step_start.sessionID`, deterministic capture, and 429/quota error shape, is runtime-artifact-bound to the actual OpenCode binary and its stream. | No proof-plan ref; inferred from gap rows. | Fallback proof is fake-provider JSONL and scripted stdout/stderr fixtures. | Proxy-only for real OpenCode behavior; acceptable only for runner-owned parser/dispatch if explicitly scoped that way. | Isolated real OpenCode sandbox evidence, or a proof plan that narrows the runtime claim to runner-owned handling of the documented/fixture stream. | FILE `planning/opencode-contract/gap-matrix.md`: event shape is from docs/source, not a local production invocation at line 27; direct `opencode1 run --format json` hung at 120 seconds at line 39; fake-only proof environment is mandated at line 161. FILE `planning/oc-gate/gates/diff.patch`: fake capture scripts emit `step_start.sessionID` at lines 1261-1290 and assert fake launch argv at lines 1294-1345; fake terminal 429/quota tests inject JSON at lines 867-917. | Yes |
| PR-006 | HIGH | Real OpenCode turn-script ingest and five-account routing depend on native OpenCode storage layout and account/auth/quota mapping. | No proof-plan ref; inferred from gap rows. | Fallback proof is local `opencode-turns` fixture files and fake quota scripts. | Proxy-only for native storage and account/auth mapping; runner-owned turn ingestion/routing selection is only partially exercised. | Verified native OpenCode message layout/account mapping evidence, or explicit proof-plan scoping to local adapter fixture behavior and generic routing over fake quota outputs. | FILE `planning/opencode-contract/gap-matrix.md`: quota/auth mapping is unresolved at lines 65 and 117-118; turn scripts were absent and need adding at lines 71 and 119; no real `agents`/`opencode` against production state at line 5. FILE `planning/oc-gate/gates/diff.patch`: turn ingest test builds temp `storage/message/ses_fixture/msg_*.json` fixtures at lines 952-989; `scripts/opencode-turns` assumes `BASE_DIR/storage/message/ses_<id>/msg_*.json` at lines 1556-1566; five-account routing uses fake quota output scripts at lines 1050-1111. | Yes |

## Evidence-class decision

The proof-plan structure is incomplete. The artifact cannot receive a LOW proof-risk verdict because it lacks the exact `## Proof plan` section and all three required fields.

The fallback evidence visible in the proposal and diff is mixed. Fake-provider integration tests can be the right evidence class for runner-owned runtime behavior when the claim is precisely scoped to the runner's launch assembly, capture parser, DB resume resolution, Tauri resume command, wake orchestration, terminal recognizer dispatch, and generic routing selection. The supplied artifact does not make that scoping statement.

The same fake-provider tests are proxy-only for claims about real OpenCode external behavior. The proposal itself says the JSON event shape came from docs/source rather than a local production invocation, records a 120-second direct invocation hang, keeps account/auth mapping unresolved, and mandates fake provider binaries rather than real `agents` or real `opencode` for proof commands.

Requested surface assessment:

| Surface | Evidence-class decision |
|---|---|
| `session_capture` of `step_start.sessionID` | The shipped fake-provider tests exercise runner capture argv/parsing, but do not prove real OpenCode emits and terminates/streams in the required production shape. Missing proof-plan scoping keeps this HIGH. |
| Non-UUID resume/wake | The Tauri/resume/wake tests appear to exercise production runner paths with temp DB/config and fake provider scripts. They could match a runner-owned runtime claim, but no proof-plan field says that or separates it from real OpenCode behavior. |
| OpenCode 429/quota terminal classification | The shipped tests inject JSON error fixtures through direct/service execution. They exercise runner recognizer dispatch, not real OpenCode emission semantics. |
| Turn-script ingest | The shipped test and adapter exercise assumed local message-file fixtures. They do not prove native OpenCode storage compatibility unless the claim is scoped to the adapter fixture contract. |
| Five-account routing | The shipped routing test proves generic selection over five fake quota scripts. It does not prove real opencode account-to-auth-file mapping or native quota source correctness. |

## Residual ambiguity / stop-condition notes

No stop condition blocks report generation: `mode`, `proposal_path`, `worktree_path`, `contract_path`, and `report_path` were supplied and readable/writable. No `NEEDS_INPUT` is warranted because the missing proof-plan structure and evidence-class mismatch can be resolved from the supplied artifacts.

VERDICT: HIGH
