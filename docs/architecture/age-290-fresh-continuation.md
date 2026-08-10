# Fresh Continuation Integration

`oulipoly-runtime::fresh_continuation` defines the use-case API. Existing
application components integrate through its ports; the use case does not move
their internal responsibilities into the coordinator.

```text
CLI / dispatch
  no continuation request -> existing resume path
  continuation request    -> FreshContinuationCoordinator
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

## Composition Boundary

The runner dispatch layer is the composition root. It constructs the
coordinator from production adapters only for an explicit fresh-continuation
request. Without that request it calls the existing resume entry point
unchanged.

The resume and balancing modules do not call back into fresh continuation.
They expose adapter-level operations that accept a `ReservedInvocation` and
return the exact typed `InvocationOutcome`. This replaces the prototype's
post-run database search while keeping resume classification and model
execution owned by their current components.

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
