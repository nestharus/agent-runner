# Fresh Continuation Integration

`oulipoly-runtime::fresh_continuation` defines the use-case API. Existing
application components integrate through its ports; the use case does not move
their internal responsibilities into the coordinator.

```text
CLI / dispatch
  no --fresh-continuation-request -> existing resume path
  --fresh-continuation-request    -> strict schema-v1 request reader
                                  -> FreshContinuationCoordinator
                                |-- ContinuationEvidenceValidator
                                |-- ContinuationStore
                                |-- ResumeRunner
                                |-- FreshRunner
                                `-- HandoffPublisher
```

## Application Adapters

| Runtime port | Existing application responsibility | Required hook shape |
|---|---|---|
| `ContinuationEvidenceValidator` | Question, answer, session graph, trace, ticket, workspace, and boundary inputs | Return a bounded `ValidatedContinuation` or a typed block before execution. |
| `ContinuationStore` | Durable runner state and invocation identity | Accept/replay one continuation and reserve exact resume/fresh identities without latest-row discovery. |
| `ResumeRunner` | `src-tauri` headless resume flow | Run or observe one reserved exact-session invocation and return `InvocationOutcome` after existing classification/finalization. |
| `FreshRunner` | `src-tauri` model routing/execution flow | Run or observe one reserved fresh invocation and return `InvocationOutcome` without turning retry or rotation into a second continuation invocation. |
| `HandoffPublisher` | Session-graph continuation output | Publish the typed handoff idempotently; publication never authorizes execution. |

## Adapter declarations

```yaml
adapter_declarations:
  - component: docs/architecture/age-290-fresh-continuation.md
    role: adapter
    Translates:
      - oulipoly-runtime-fresh-continuation-port-contract
      - src-tauri-fresh-continuation-application-contract
```

## Fresh Continuation Request Schema

This document is the runner-owned interface for request producers. A producer
pushes one strict JSON object into the path passed by
`--fresh-continuation-request`; the runner does not discover or infer fields.
Unknown fields are rejected.

```json
{
  "schema_version": 1,
  "kind": "fresh_continuation_request",
  "question_id": "<question identity>",
  "origin_invocation_id": "<invocation UUID>",
  "origin_session_id": "<provider session identity>",
  "planning_root": "<absolute planning directory>",
  "worktree": "<absolute worktree directory>",
  "last_successful_boundary": "<boundary description>",
  "active_blocked_boundary": "<boundary description>",
  "target_model": "<configured model name>",
  "evidence": {
    "question": { "path": "<absolute path>", "sha256": "<64 hexadecimal characters>" },
    "answer": { "path": "<absolute path>", "sha256": "<64 hexadecimal characters>" },
    "session_graph": { "path": "<absolute path>", "sha256": "<64 hexadecimal characters>" },
    "origin_trace": { "path": "<absolute path>", "sha256": "<64 hexadecimal characters>" },
    "ticket_snapshot": { "path": "<absolute path>", "sha256": "<64 hexadecimal characters>" }
  }
}
```

Artifact hashes are case-insensitive hexadecimal on input and are normalized
for comparison. The request's `origin_session_id` must match top-level
`--resume`. When `--project` is supplied, its canonical path must identify the
same directory as `worktree`.

## Composition Boundary

The runner dispatch layer is the composition root. It constructs the
coordinator from the filesystem evidence reader, SQLite continuation store,
exact invocation observers, reserved execution callbacks, and create-once
filesystem publisher only for `--fresh-continuation-request <PATH>`. The flag
requires top-level `--resume`, and `origin_session_id` must match that active
resume session. When `--project` supplies a working directory, it must match
`worktree`; when `--project` is absent, `worktree` becomes the effective working
directory. The flag rejects provider rotation. Without the flag, dispatch calls
the existing resume entry point unchanged.

The resume and balancing modules do not call back into fresh continuation.
The command callback resolves each `ReservedInvocation` to its exact UUID and
parent row, then calls the prepared-resume or one-attempt balancing entry point.
The action adapters observe that exact row after execution. This replaces the
prototype's post-run database search while keeping resume classification and
model execution owned by their current components.

## Execution and Recovery Semantics

Resume and fresh reservations provide at-most-once execution, not exactly-once
completion. Moving a reservation to `running` means physical execution may
have started. A restart therefore observes the same reserved UUID and exact
parent row; elapsed time never authorizes another execution attempt.

If a process stops after recording `running` but before creating an observable
invocation row or durable outcome, automatic execution remains suspended. An
operator or a future reconciliation workflow must establish what happened
before changing that state. This deliberately trades automatic liveness for
protection against duplicate provider turns and side effects.

Handoff publication is create-once and idempotent for identical bytes. The
publisher synchronizes the temporary file before linking it and synchronizes
the containing directory after creating the final entry on platforms that
support directory synchronization. A terminal database outcome records the
published path and hash, but that row is not sufficient proof that the file
still exists. Terminal replay reopens the exact create-once target, rejects
symlinks and non-files, and verifies the recorded hash before returning the
stored outcome. Missing, replaced, or corrupted handoffs fail replay without
relaunching either reserved invocation.

## Integration Sequence

1. Runtime contract tests establish coordinator behavior with fake ports.
2. Each existing subsystem is refactored behind its port and receives an
   adapter contract test for exact inputs and return values.
3. Dispatch is wired only after all adapter contracts pass.
4. One end-to-end test proves the composed path while all runtime and adapter
   tests remain unchanged.

If an adapter implementation still combines separable responsibilities,
decompose that adapter behind additional APIs and tests. Do not change the
parent runtime contracts to mirror incidental details of the current code.
