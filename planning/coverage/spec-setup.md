# spec-setup — One-time setup, detection, sync, actions

## Source files

- `crates/oulipoly-setup/src/lib.rs`
- `crates/oulipoly-setup/src/actions.rs`
- `crates/oulipoly-setup/src/agent.rs`
- `crates/oulipoly-setup/src/context.rs`
- `crates/oulipoly-setup/src/detection.rs`
- `crates/oulipoly-setup/src/memory.rs`
- `crates/oulipoly-setup/src/schemas.rs`
- `crates/oulipoly-setup/src/sync.rs`

## Preconditions

- A fresh or partially-configured host environment.
- The setup agent has been invoked by the first-run wizard or by a
  `--setup` CLI flag.
- Network access for OAuth flows (when needed by a provider).

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| First run, no providers installed. | `detection.rs` returns an empty installed set; `actions.rs` produces a wizard plan (install + auth flows). |
| Re-run with some providers installed. | `detection.rs` reports the installed set; `actions.rs` plans only the missing steps. |
| OAuth completes for a provider. | `sync.rs` writes the resulting credentials into the right config file; subsequent `detection.rs` recognizes the provider as authenticated. |
| User aborts mid-wizard. | `actions.rs` returns a cooperative cancel; partial state is consistent (no half-written config files). |
| Memory step requested. | `memory.rs` records the wizard's transient state into the documented memory store so a resumed wizard picks up where it left off. |
| Agent step (`agent.rs`). | Spawns an agent invocation per the wizard's schema; result is parsed into the wizard's state machine. |

## Edge cases

- Detection probe times out — record as "unknown" rather than treating
  as "not installed".
- A provider is partially-installed (binary present, no credentials) —
  detection reports installed-but-not-authed; wizard plans the auth
  step.
- User's host has a stale OAuth token — sync detects via the auth probe
  and re-prompts.
- Wizard step writes a config file with the wrong owner (multi-user
  host) — surface a clear permission error; do not silently retry.

## Error conditions

- `SetupDetectionFailed` — probe IO or parse failure.
- `SetupActionFailed` — an action step returned non-zero.
- `SetupAgentFailed` — the agent invocation produced a malformed
  response.
- `SetupSyncFailed` — credential file write failed.

## Boundaries

- Setup does NOT route real invocations — that is the balancer/executor
  during normal operation.
- Setup does NOT classify terminal signals — recognizer's domain.
- Setup does NOT mutate the state DB — `oulipoly-state` is independent;
  setup writes to filesystem config files (consumed by
  `oulipoly-config`).

## Declared test patterns

Per `~/ai/conventions/testing.md`: per-step action-shape tests, fixture
tests on the detection probe, agent-step round-trip tests.

- `crates/oulipoly-setup/tests/fixtures/claude_stub_main.rs`
- `crates/oulipoly-setup/tests/setup_agent_send_turn.rs`
- `src-tauri/tests/age36_wiring.rs`

## Cross-references

- `planning/coverage/spec-discovery.md` — runtime-side detection (this
  surface is wizard-side; both probe but for different consumers).
- `planning/coverage/spec-config.md` — sync target.
- `AGENTS.md` § "What This Is".
