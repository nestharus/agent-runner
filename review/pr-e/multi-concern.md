# PR-E Multi-Concern Review

**Branch:** `init-02/pr-e`  
**Value under test:** V16 — one PR per concern unless concerns are mutually load-bearing.

## The concerns in the bundle

| # | Concern | Files |
|---|---|---|
| 1 | `repl` CLI surface and dispatch | [src-tauri/src/main.rs](/home/nes/projects/agent-runner/src-tauri/src/main.rs:89) |
| 2 | Declarative `interactive_args` config shape, round-trip, and load-time validation | [src-tauri/src/config/model.rs](/home/nes/projects/agent-runner/src-tauri/src/config/model.rs:6), [src-tauri/src/config/model.rs](/home/nes/projects/agent-runner/src-tauri/src/config/model.rs:344) |
| 3 | Interactive executor entry with inherited stdio and `wait()` | [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:338) |
| 4 | Interactive invocation lifecycle hardening: `FinalizerGuard`, TTY-gated invocation-line emission, parent-env propagation reuse | [src-tauri/src/main.rs](/home/nes/projects/agent-runner/src-tauri/src/main.rs:336), [src-tauri/src/main.rs](/home/nes/projects/agent-runner/src-tauri/src/main.rs:371), [src-tauri/src/main.rs](/home/nes/projects/agent-runner/src-tauri/src/main.rs:405) |
| 5 | Unix signal survival/forwarding plus new `signal-hook` dependency | [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:364), [src-tauri/src/executor/cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:607), [src-tauri/Cargo.toml](/home/nes/projects/agent-runner/src-tauri/Cargo.toml:22) |

## Could any ship independently?

**Concerns 1, 2, and 3 are mutually load-bearing.** `repl` without `interactive_args` is mostly a shell that errors on real providers, because the existing `args` surface is intentionally one-shot and the proposal explicitly rejects falling back to it. `interactive_args` without `repl` is dead schema: it round-trips, but no user-facing path can consume it. `execute_interactive()` without the `repl` entrypoint is likewise unreachable plumbing. The first point where user value appears is the combination: caller-visible `repl` plus provider-declared interactive argv plus an executor that hands off the terminal correctly.

**Concern 4 is not a reusable pre-PR in its current form.** `FinalizerGuard` in [main.rs](/home/nes/projects/agent-runner/src-tauri/src/main.rs:336) is tightly scoped to the new interactive lifecycle. It is not introduced as a shared invocation abstraction, and the existing one-shot path still finalizes manually. Shipping it alone would add dead code. The same applies to the `should_emit_invocation_line()` helper in [main.rs](/home/nes/projects/agent-runner/src-tauri/src/main.rs:371): this is not a real PR-A refactor, because the one-shot path still does unconditional `eprintln!`; the helper exists only to make the new interactive TTY gate testable.

**`OULIPOLY_PARENT_INVOCATION` propagation is also coupled, not standalone.** The underlying mechanism already existed in the one-shot executor; PR-E is just making the interactive path honor the same contract via `build_command(...)` and `run_repl(...)`. That is lifecycle completeness for `repl`, not a second product concern.

**Concern 5 only becomes meaningful because concern 3 uses inherited stdio plus `wait()`.** The signal block in [cli.rs](/home/nes/projects/agent-runner/src-tauri/src/executor/cli.rs:607) exists so the parent survives `SIGINT`/`SIGHUP` long enough to reap and finalize, while forwarding `SIGTERM` once. Without the interactive wait-path, this code has no reason to exist. In theory you could stage it as a hardening follow-up after `execute_interactive()`, but that would knowingly ship the normal Ctrl-C lifecycle broken for the new command, which is weak under V10 and not a good V16 split.

## Verdict

**No split recommended.** PR-E reads as one concern: “balanced interactive launch as a first-class runner mode.” The listed pieces are the minimum slices needed to make that mode real rather than hollow: declarative provider config, a distinct interactive executor, and the lifecycle/TTY/signal plumbing required for that executor to behave correctly. The only technically separable items are dead-schema or dead-helper pre-work, and V16 argues against splitting those out when they deliver no independent value.
