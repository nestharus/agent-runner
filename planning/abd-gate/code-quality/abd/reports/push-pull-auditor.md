# Push/Pull Coupling Audit

## Inputs Read

- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate`
- `wu_id=abd`
- `mode=phase-6`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wu-b/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate/contracts/abd.contract.md`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate/gates/diff.patch`
- `touched_surfaces_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate/gates/touched-surfaces.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate/code-quality/abd/reports/push-pull-auditor.md`

## References Read

- A1 metric source: `/home/nes/ai/conventions/code-quality.md` lines 106-112 define Push-vs-pull system coupling, the session-graph disambiguator, common-interface proof, and `uncontrolled-source coupler`.
- A1 canonical-doc-as-schema source: `/home/nes/ai/conventions/code-quality.md` lines 114-131 define declared schema-owner handling for generated artifacts and preserve HIGH for undeclared private layout.
- A1 touched-file ownership: `/home/nes/ai/conventions/code-quality.md` lines 21-27 and 143-149 require whole touched file/component review.
- Phase 6 contract requirement: `/home/nes/ai/conventions/code-quality.md` lines 169-173 require reading the contract and proposal before scoring.
- Threshold/failure-mode preservation: `/home/nes/ai/conventions/code-quality.md` lines 291-310 preserve numerical thresholds and include `uncontrolled-source coupler`.
- Terminology disambiguator: `/home/nes/ai/conventions/agent-questions-and-session-graph.md` lines 230-242 define the separate session-graph Pull-vs-Push Policy.
- Proposal metadata/common-interface contract: `planning/wu-b/proposal.md` lines 67-83 define `meta.json` `caller_chain` shape, aliases, rc file read, and extra metadata preservation.
- Proposal mailbox/common-interface contract: `planning/wu-b/proposal.md` lines 113-188 define the sidecar mailbox schema and stable `agent_bash_complete` payload fields.
- Proposal delivery contract: `planning/wu-b/proposal.md` lines 217-267 define headless delivery, prompt envelope shape, ordering, and batch rules.
- Proposal PTY/topology contract: `planning/wu-b/proposal.md` lines 269-289 define the queued PTY v1 behavior and future broker seam.
- Phase 6 adapter declarations: `planning/abd-gate/contracts/abd.contract.md` lines 500-556 declare adapter surfaces for `notify.rs`, `mailbox_delivery.rs`, CLI, and executor launch/supervision surfaces.
- Phase 6 intrinsic-surface declarations: `planning/abd-gate/contracts/abd.contract.md` lines 558-597 declare ownership for PID identity sidecar, mailbox sidecar, spawn identity capture, and auto-wake lifecycle.
- Changed production surfaces: `planning/abd-gate/gates/touched-surfaces.md` lines 3-29 list new and existing production files in scope.
- Diff evidence: `planning/abd-gate/gates/diff.patch` lines 1-5797 show touched manifests, runtime/state/Tauri files, and integration tests.

## Pull Sites Inspected

| ID | Puller | Source | Pull mechanism | Ownership/interface evidence | Verdict | Evidence |
|---|---|---|---|---|---|---|
| PP-001 | `src-tauri/src/commands/notify.rs` | Agent-bash spooler `meta.json`, rc file, and path arguments | `std::fs::read_to_string`, JSON parse, field extraction, rc parse | LOW common-interface proof. The proposal declares `meta.json caller_chain`, accepted field aliases, rc path reads, and extra metadata preservation; the Phase 6 contract declares `notify.rs` as an adapter translating the agent-bash async spooler completion contract. | LOW | `notify.rs` lines 114-120, 155-223, 306-330; `proposal.md` lines 67-83; `abd.contract.md` lines 500-508. |
| PP-002 | `src-tauri/src/commands/notify.rs` | PID identity sidecar and versioned `state.db` fallback for owner-session resolution | Read-only sidecar open, `lookup_by_identity`, read-only `StateDb::get_invocation_by_uuid` fallback | LOW source-control/common-interface proof. PID sidecar and state DB APIs are same-repo controlled interfaces; proposal declares the death-safe lookup algorithm; contract declares PID sidecar ownership. | LOW | `notify.rs` lines 225-304; `proposal.md` lines 85-111; `abd.contract.md` lines 562-571. |
| PP-003 | `crates/oulipoly-state/src/mailbox.rs` | Mailbox/session-runtime/wake-claim tables in `pid-identity.db` sidecar | SQLite SELECT/INSERT/UPDATE/DELETE, `PRAGMA table_info`, additive table creation | LOW source-control proof. `mailbox.rs` owns the sidecar table schema and operations, and the Phase 6 contract declares `mailbox_sidecar` ownership. | LOW | `mailbox.rs` lines 5-7, 225-283, 285-474, 483-612, 632-701, 703-777, 792-959, 1006-1037; `abd.contract.md` lines 572-580. |
| PP-004 | `crates/oulipoly-state/src/pid_identity.rs` | PID identity sidecar and Linux process identity surfaces | SQLite sidecar reads/writes, `/proc/<pid>/stat`, `/proc/sys/kernel/random/boot_id`, `getpgid` | LOW source-control/common-interface proof. The module owns `pid-identity.db` schema; Linux procfs and Unix process group reads are explicitly declared as owned intrinsic-surface operations in the Phase 6 contract. | LOW | `pid_identity.rs` lines 5-7, 81-113, 115-226, 229-288, 321-380; `abd.contract.md` lines 562-571. |
| PP-005 | `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | Parent invocation env payload and child PID sidecar/runtime updates | Parse parent invocation env, record live child identity, open mailbox sidecar to mark runtime running | LOW common-interface/source-control proof. Parent invocation env is an in-repo `CompositeInvocationId` interface, and contract declares child spawn identity capture including env-to-context mapping and session-runtime updates. | LOW | `spawn_identity.rs` lines 40-57, 59-115; `abd.contract.md` lines 581-588. |
| PP-006 | `src-tauri/src/mailbox_delivery.rs` | Mailbox rows for resume notification delivery | Open optional mailbox sidecar, `list_pending`, format envelope from row fields, mark delivered | LOW common-interface proof. Proposal declares mailbox row fields and provider resume notification-envelope contract; contract declares `mailbox_delivery.rs` as adapter translating those two contracts. | LOW | `mailbox_delivery.rs` lines 17-62, 69-149; `proposal.md` lines 217-267; `abd.contract.md` lines 509-513. |
| PP-007 | `src-tauri/src/wake_coordinator.rs` | Auto-wake env vars, session-runtime/wake-claim sidecar rows, current executable, detached process topology | `std::env::var`, `MailboxDb` reads/writes, `std::env::current_exe`, `Command::new(...).spawn()`, Unix `setsid` | LOW source-control/common-interface proof. The auto-wake env family, detached resume launch, claim lifecycle, and child validation are declared intrinsic-surface operations. | LOW | `wake_coordinator.rs` lines 16-23, 59-82, 93-155, 170-281, 284-340; `abd.contract.md` lines 589-596. |
| PP-008 | Executor launch surfaces: `headless.rs`, `interactive.rs`, `resume_execution.rs`, `provider_execution.rs`, `supervision/mod.rs` | Provider launch/supervisor output/std process child lifecycle/return-channel IPC | Provider launch assembly, supervised child spawn/drain/poll, process status, return-channel cleanup, model identity threading | LOW common-interface proof. The Phase 6 contract declares these executor surfaces as adapters over provider launch, supervisor output, std child lifecycle, stdio pipe drain, return-channel IPC, terminal signal, and execution-result contracts. | LOW | `headless.rs` lines 36-70 and 101-135; `interactive.rs` lines 73-123; `resume_execution.rs` lines 105-157; `provider_execution.rs` lines 62-102; `supervision/mod.rs` lines 100-180; `abd.contract.md` lines 522-556. |
| PP-009 | Resume/repl/dispatch orchestration files | In-repo runtime service ports, state/config environment, resume target and migration services | Service-port calls, environment loading, resolved resume fields, direct calls into mailbox delivery and executor interfaces | LOW source-control proof. These are same-repo service/DTO/API boundaries and minimal orchestration wiring; no external private source is pulled directly. | LOW | `dispatch.rs` lines 83-115, 154-255, 283-297; `run/resume/orchestration.rs` lines 42-86, 109-170, 190-368, 823-850; `run/repl/orchestration.rs` lines 39-114, 188-216, 290-425. |
| PP-010 | CLI/main command surfaces | OS argv, clap-derived CLI command schema, process exit code | `std::env::args`, `Cli::parse_from`, clap `Subcommand` definitions | LOW common-interface proof. `main.rs` and `usage/cli.rs` are declared adapters from process argv/clap into the public runner CLI surface. | LOW | `main.rs` lines 56-93, 99-126; `usage/cli.rs` lines 18-103, 105-236, 238-349; `abd.contract.md` lines 514-521. |
| PP-011 | Test surfaces touched by the diff | Tempdir-isolated sidecar/state files, fixture provider scripts, CLI stdout/stderr/JSON output | Test `Command` invocation, fixture file writes/reads, JSON assertions, sidecar reads | LOW source-control proof. Tests construct and own fixture producers and consume same-repo CLI/sidecar interfaces; the diff includes these test files but they do not introduce deployment-level private endpoint or uncontrolled-source reads. | LOW | `diff.patch` lines 447-622, 4693-6480; `age_pid_sidecar_spawn.rs` lines 52-97, 99-135; `wu_b_mailbox_integration.rs` lines 39-165, 214-260; `wu_d_proactive_wake_integration.rs` lines 34-100, 137-218. |
| PP-012 | Cargo manifests and lockfile touched by dependency additions | Cargo dependency metadata for `libc`/`tracing` | Manifest/lock dependency declaration | LOW source-control/common-package-interface proof. Dependency manifests do not pull a private runtime source; they declare package dependencies through Cargo's public package interface. | LOW | `diff.patch` lines 1-28, 623-634, 2667-2686. |

## Uncontrolled-Source Coupler Findings

| ID | Puller | Source | Implicit contract evidence | Missing proof | Decoupling direction | Failure mode |
|---|---|---|---|---|---|---|
| None | None | None | No concrete pull site inside the touched files/components lacked source-control proof or common-interface proof. | None | None | None |

## Residual Ambiguity / Stop-Condition Notes

- No `BLOCKED` condition applied: required paths were readable, the Phase 6 contract was present/readable, and the A1 metric source preserved the required rule text, disambiguator, failure mode, and thresholds.
- The cross-system seam called out by the work unit, `notify.rs` reading agent-bash spooler `meta.json` and rc/log/state paths, is declared by both `proposal.md` and the Phase 6 adapter declarations. It is scored LOW common-interface proof rather than HIGH private-source coupling.
- Existing orchestration files touched by minimal wiring were inspected under touched-file ownership. For A4, their visible pull sites remain routed through in-repo service ports, DTOs, state-owned APIs, or declared executor/CLI adapter contracts; no pre-existing uncontrolled-source coupler was found in those touched components.
- Deployment-level pull sites were checked for service, database, cache, filesystem, private endpoint, and service-topology reads. The visible database/filesystem reads are same-repo-owned sidecars, versioned state APIs, tempdir-owned tests, or declared OS/runtime APIs; no private endpoint or uncontrolled deployment topology pull was found.

Verdict: LOW

VERDICT: LOW
