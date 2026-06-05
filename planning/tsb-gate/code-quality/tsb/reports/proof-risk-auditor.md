# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | `phase-6` | n/a | n/a | Phase 6 per-component proof-risk context supplied by caller. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Used to resolve supplied artifacts. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/proposal.md` | 6179 | `a5f57b3f34e3` | Read before scoring. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/contracts/tsb.contract.md` | 17344 | `ad72a7aa6113` | Read before scoring, as required for Phase 6. |
| code-quality convention | `/home/nes/ai/conventions/code-quality.md` | 30798 | `fa8b6499cc2e` | Read before scoring. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/diff.patch` | 42598 | `97205fb15f07` | Read as delta context. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/touched-files.txt` | 210 | `28440caadc65` | Read as touched-surface context. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| `## Proof plan` | Yes | `planning/tsb-gate/proposal.md:7` contains the exact section heading. |
| `Runtime claim` | Yes | Claims are listed at `planning/tsb-gate/proposal.md:11`, `planning/tsb-gate/proposal.md:17`, `planning/tsb-gate/proposal.md:23`, `planning/tsb-gate/proposal.md:29`, `planning/tsb-gate/proposal.md:35`, `planning/tsb-gate/proposal.md:41`, `planning/tsb-gate/proposal.md:47`, `planning/tsb-gate/proposal.md:53`, `planning/tsb-gate/proposal.md:59`, and `planning/tsb-gate/proposal.md:65`. |
| `Proof method` | Partial | Valid proof methods are named at `planning/tsb-gate/proposal.md:13`, `planning/tsb-gate/proposal.md:19`, `planning/tsb-gate/proposal.md:25`, `planning/tsb-gate/proposal.md:31`, `planning/tsb-gate/proposal.md:37`, `planning/tsb-gate/proposal.md:43`, and `planning/tsb-gate/proposal.md:49`; lines `planning/tsb-gate/proposal.md:55`, `planning/tsb-gate/proposal.md:61`, and `planning/tsb-gate/proposal.md:67` explicitly say no shipped test/proof covers the stated runtime claim. |
| `Evidence-class match` | Partial | Evidence-class explanations are present at `planning/tsb-gate/proposal.md:15`, `planning/tsb-gate/proposal.md:21`, `planning/tsb-gate/proposal.md:27`, `planning/tsb-gate/proposal.md:33`, `planning/tsb-gate/proposal.md:39`, `planning/tsb-gate/proposal.md:45`, and `planning/tsb-gate/proposal.md:51`; lines `planning/tsb-gate/proposal.md:57`, `planning/tsb-gate/proposal.md:63`, and `planning/tsb-gate/proposal.md:69` explicitly state the claims are unproven by shipped tests. |

## Findings
| Finding ID | Severity | Runtime claim | Proof plan ref | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|---|
| PR-001 | HIGH | Timestampless implicit OpenCode discovery falls back to the configured max-session cap. | `planning/tsb-gate/proposal.md:53-57` | The proof plan says no shipped test directly covers an over-cap timestampless session list or `OPENCODE_TURNS_MAX_SESSIONS` truncation. | No validation surface; the cited timeout test uses two timestampless sessions and does not exercise cap enforcement. | `scripts/opencode-turns` runtime adapter behavior for `OPENCODE_TURNS_MAX_SESSIONS` over timestampless `opencode session list --json` output. | `planning/tsb-gate/proposal.md:53-57`; `planning/tsb-gate/contracts/tsb.contract.md:163-170`; `planning/tsb-gate/contracts/tsb.contract.md:80-86` | Yes |
| PR-002 | HIGH | Python adapter timeout cleanup kills all OpenCode process-group descendants, not just the direct process. | `planning/tsb-gate/proposal.md:59-63` | The proof plan says no shipped test creates a leaking descendant marker under `scripts/opencode-turns`; the shell timeout test proves elapsed-time bounding and degraded output only. | Proxy-only adjacent evidence; elapsed deadline/degraded output is not process-group descendant cleanup evidence. | `scripts/opencode-turns` runtime adapter timeout cleanup through `kill_process_group` for spawned OpenCode subprocess groups. | `planning/tsb-gate/proposal.md:59-63`; `planning/tsb-gate/contracts/tsb.contract.md:108-116`; `planning/tsb-gate/gates/diff.patch:836-913` | Yes |
| PR-003 | HIGH | Runtime session-script process-group timeout kills shell grandchildren. | `planning/tsb-gate/proposal.md:65-69` | The proof plan says no shipped session-script test mirrors the quota child-marker test; the existing session timeout test proves classification and conservative persistence behavior only. | Proxy-only adjacent evidence; timeout classification/no-persist assertions do not prove shell-grandchild process-group kill behavior. | `crates/oulipoly-runtime/src/sessions/mod.rs` runtime session-script timeout path and process-group kill behavior. | `planning/tsb-gate/proposal.md:65-69`; `planning/tsb-gate/contracts/tsb.contract.md:47-58`; `planning/tsb-gate/contracts/tsb.contract.md:171-179`; `planning/tsb-gate/gates/diff.patch:344-375` | Yes |

## Evidence-class decision

The proof plan names several runtime claims whose evidence class matches the scoped claim: fake OpenCode integration is acceptable for proving the adapter invokes the public `opencode session list --json`/`opencode export` command shape rather than private storage (`planning/tsb-gate/proposal.md:11-15`), script integration is acceptable for recent-window filtering and degraded best-effort behavior (`planning/tsb-gate/proposal.md:17-27`), and unit/runtime tests are acceptable for degraded-marker parsing, timeout classification, conservative persistence, and quota process-group child kill behavior (`planning/tsb-gate/proposal.md:29-51`). These claims are scoped to project adapter/runtime behavior, not to real external OpenCode availability.

The three HIGH findings are runtime-artifact-bound claims with no matching proof method. The contract declares `scripts/opencode-turns` as the shipped adapter and owner of `OPENCODE_TURNS_MAX_SESSIONS`, call timeouts, deadline behavior, and `kill_process_group`-related behavior (`planning/tsb-gate/contracts/tsb.contract.md:60-131`, `planning/tsb-gate/contracts/tsb.contract.md:163-170`). The contract also declares `crates/oulipoly-runtime/src/sessions/mod.rs` as owner of session-script execution deadlines and process-group kill on session script timeout (`planning/tsb-gate/contracts/tsb.contract.md:35-58`, `planning/tsb-gate/contracts/tsb.contract.md:171-179`). Because the proof plan explicitly says the cap and descendant/grandchild cleanup claims are unproven by shipped tests, the planned evidence class does not exercise those runtime claims.

## Residual ambiguity / stop-condition notes

No unreadable required artifact or unwritable output stop condition was encountered. No human-owned ambiguity is needed to score the proof plan: the proposal itself states the missing proof surfaces for PR-001, PR-002, and PR-003.

VERDICT: HIGH
