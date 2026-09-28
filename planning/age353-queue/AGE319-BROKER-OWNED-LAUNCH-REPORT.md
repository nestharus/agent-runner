# AGE-319 Broker-owned launch slice — 2026-09-28

## Disposition

This is a **closed execution slice**, not a working first-install Runner/Bash
pair. A fresh production `L` now publishes one Broker-chosen root UUID in the
same fsynced request record before any possible fork. It still replies
`pending`; no control Runner, root PID1, State effect, terminal receipt, or
physical drain is created. No host State/service/alias, Bash repository, or
ticket was mutated. Do not install this pair for live use.

## Source result

- New records use `installed-launch-request-v2` and bind request, pair/source,
  launcher incarnation and UID, exact spec digest, and root UUID in one
  create-new publication. A second request cannot reuse a ledger root UUID.
- An exact duplicate remains pending and cannot enter the new-admission path;
  changed identity refuses. V1 request-only records remain readable as pending
  but cannot be upgraded or replayed into V2. Startup rejects malformed or
  duplicate root bindings. The challenged `l` status remains read-only and
  reveals no root ID or terminal claim.
- The `Admission::New { root_id }` result is available for the future
  Broker-owned custody transition. `L` deliberately does not consume it for
  effects until that transition can carry a connected grant.

## Why execution remains closed

`v30_host_entry` currently asks E to reserve a separate random root, then
forks its guardian from a Runner host process. The schema-2 installed Runner
gate rejects that host mode; enabling the old environment variable would
admit a direct Runner. Broker L has no connected, one-use host-control grant,
no exact E reservation against the bound root UUID, and no nonblocking
control-process lifecycle. It also retains no launch descriptors after L
returns. A request record alone cannot justify a later fork. Terminal State
settlement and PID1/parent-wait physical drain are likewise not connected to
the request; `exit <code> drained <request-id>` remains unavailable.

The next slice must spawn the pinned Runner under the original UID/cwd/stdio
and a Broker-owned connected grant, carry the bound UUID through exact E/P/G/A/J,
keep the main Broker loop serving, and only then use fresh K/Q/close and U/D.
An existing or recovered request must never reacquire launch descriptors or
start that path. Host-root and attached Bash proof remain outstanding.

## Verification and review gap

Passed with default features disabled:

- `cargo test --locked -p oulipoly-kernel-broker --lib installed_launch_ledger --no-default-features`
  — 4 passed, including a separate publishing process, lost reply, reopen,
  exact duplicate, changed owner/spec, V1 retry, and duplicate-root refusal.
- `cargo test --locked -p oulipoly-kernel-broker --bin oulipoly-kernel-broker fresh_installed_preflight_accepts_only_supported_headless_cli_shape --no-default-features`
  — 1 passed.
- `cargo build --locked -p oulipoly-kernel-broker --bin oulipoly-kernel-broker --bin oulipoly-installed-launcher -p oulipoly-agent-runner --bin oulipoly-agent-runner --no-default-features`
  — passed with existing unused-code warnings.
- `cargo fmt --all --check` and `git diff --check` — passed.

The full nine-domain CRW cohort and corrective review were not run because
this assignment explicitly prohibited subagents. No full workspace stress,
disposable-root end-to-end, host-root service/alias, or Bash integration test
was run. This is a source handoff for parent review, not production readiness.
