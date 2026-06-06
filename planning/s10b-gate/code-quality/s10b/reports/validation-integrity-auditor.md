# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| Operator | `/home/nes/ai/agents/validation-integrity-auditor.md` | 11070 | `6983abb60806` | Required operator file. |
| Runtime claim | Inline caller value | 706 | `c6bcb8f7edff` | Claim scopes S10B launch/resume compatibility, schema-valid free-form `describe.concurrency`, no `state.db` schema change. |
| Worktree | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Readable repository worktree. |
| Diff | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/gates/diff.patch` | 80921 | `fed91f78e958` | Authoritative audited source delta. |
| Contract | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/contracts/s10b.contract.md` | 29878 | `df6f0c26b082` | Phase 6 component contract. |
| Proposal | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/proposal.md` | 9326 | `d306cf559cbe` | Approved proof and claim context. |
| Runtime evidence | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/evidence/runtime-tests.log` | 6386 | `965254d4d40f` | Deterministic test and live launch evidence summary. |
| Supporting schema reference | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/contract/v1/describe.schema.json` | 2621 | `a6dc7f06affa` | Checked unchanged frozen `describe.concurrency` schema surface. |
| Supporting registry reference | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/crates/oulipoly-provider/src/schemas.rs` | 15044 | `caf8f667c024` | Checked schema registry embedding for describe responses. |
| Supporting DTO reference | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/crates/oulipoly-provider/src/generated.rs` | 29560 | `e6f777c0a70e` | Checked current DTO line anchors after patch. |
| Supporting test reference | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/crates/oulipoly-provider/tests/client_invoke.rs` | 11774 | `cbc3803a8890` | Checked current schema-validation test line anchors after patch. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| None | None | No VI-001 through VI-007 finding fired. | LOW | Diff has no removed assertion, runtime-condition skip, real-to-mock substitution, fixture-to-stub replacement, or schema-contract relaxation that weakens the proof surface. | S10B launch/resume compatibility claim. | Not applicable. | `runtime-tests.log` records isolated workspace tests and live launch smoke; proposal explicitly does not claim live resume evidence. |

Finding records: none.

Reviewed candidate `VI-006` at `crates/oulipoly-provider/src/generated.rs` diff hunk `@@ -261,8 +284,12 @@ pub struct ProviderConcurrency`: the host DTO changes from required legacy fields to optional legacy fields plus flattened metadata. I did not record it as a validation-integrity weakening because the audited diff does not relax the authoritative frozen schema or schema registry. `contract/v1/describe.schema.json` already defines `DescribeResult.concurrency` as an object with `additionalProperties: true` and no required concurrency keys, and the added `crates/oulipoly-provider/tests/client_invoke.rs:42-87` test first validates the response through `SchemaRegistry::validate_response("describe", ...)` before asserting DTO deserialization and metadata preservation. This is the declared runtime compatibility fix, not a weakened proof gate.

Reviewed protocol fixture changes at `crates/oulipoly-runtime/tests/provider_settings_host.rs` and `crates/oulipoly-runtime/tests/s10_external_launch_session.rs`: the process-status fixture changes to the typed protocol shape and the added empty policy `env` fixture exercises inherited-env preservation. These changes do not remove assertions or replace runtime proof with a stub.

Reviewed source guard and CLI integration additions: `src-tauri/tests/age244_s7b_production_wiring_source_guard.rs` is static source-invariant evidence for construction-site coverage, while resolver behavior and launch/resume behavior are separately covered by deterministic provider-registry and CLI integration evidence named in the proposal and runtime log. No `VI-007` proxy-only proof finding fires for the live resume surface because the proposal and evidence log explicitly distinguish deterministic resume integration evidence from live launch smoke evidence and do not claim live resume success.

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| None | Not required. | Not required. | Not applicable. |

## Residual ambiguity / stop-condition notes

The durable gate package under `planning/s10b-gate/**` was not added to the audited touched-file/component set; only `diff_path` was treated as authoritative for the source delta, per caller instruction.

The runtime-evidence log contains live external launch smoke evidence and deterministic resume integration evidence. It also states that no `/tmp/s10-e2e` log contains `S10-RESUME-OK`; this absence was not treated as a validation-integrity failure because live resume evidence is not claimed.

No `state.db` schema change appears in the supplied diff, matching the runtime claim and contract note.

LOW
