# AGE-319 installed CLI entry source slice — 2026-09-28

## Disposition

This branch is a **closed-path prerequisite**, not an executable first install.
The installed launcher still submits one `L` and production Broker still
refuses before root reservation. No host State, service, alias, Bash repository,
or ticket was changed. Do not install or activate these images for host use.

## Source boundary delivered

- The fixed Runner image may take the activated schema-2 `fresh-only-open`
  route only when the pair version/generation match Broker readback and it has
  a Broker-owned child gate. Direct launch, the legacy host entry variable,
  a schema-1 grant, a closed/draining route, and a missing source are refused.
  After reading the v30 physical grant, Runner checks its source generation
  against a new live installed-pair observation before the existing released
  child attestation and fresh U/D handoff. The grant FD, connected Broker
  peer, retained release, and exact root actors remain the effect authority.
- Broker lets `L` reach preflight only with a broker-owned sidecar and validated
  fresh activation while the entry gate is open. The handler still checks the
  connected launcher image, pair generation, activated pair/source binding,
  frame and descriptors. CLI syntax accepted by the existing root intent
  classifier proceeds to the final closed refusal. GUI, PTY, non-UTF8 args,
  and unsupported CLI modes refuse explicitly. Old/cutover v30 roots still
  cannot send `L` through this gate.
- Launcher recognizes only `exit <0..255> drained <exact request ID>\n` as a
  terminal result. A lost or malformed reply reports the original request ID
  and pair generation as unknown and never resubmits. A pre-effect Broker
  `error` response remains a refusal.

## Why production execution remains closed

`src-tauri/src/kernel_entry.rs::v30_host_entry` currently initiates E/P/G/A/J
from a Runner host process and forks the guardian. That path has no
Broker-owned connected launch grant for an installed schema-2 image; allowing
its environment variable as authority would let a direct installed Runner
construct a root. `root_join::launch` and the Broker's held J/release path can
place and attest a child once a valid guardian and entry exist, but `L` has no
production adapter into that custody sequence or durable request-to-root,
terminal-result and physical-drain ledger. The private installed executor is
feature-gated and uses separate supervision; it is not production proof.

The next source slice must make Broker own that initial CLI admission and
invoke the existing v30 custody/release flow with an exact physical grant,
then bind one request ID to one root and a terminal drain receipt. A read-only
status query must verify request, pair generation and owner without replay.
Only after featureless disposable-root end-to-end and host-root CLI plus
attached Bash proof can the first install and AGE-353 benchmarks proceed.

## Verification and review gap

Passed with featureless/default-disabled code:

- `cargo test --locked -p oulipoly-kernel-broker --bin oulipoly-kernel-broker fresh_installed_preflight_accepts_only_supported_headless_cli_shape --no-default-features`
- `cargo test --locked -p oulipoly-kernel-broker --bin oulipoly-kernel-broker v30_service_refuses_every_legacy_entry_and_work_operation --no-default-features`
- `cargo test --locked -p oulipoly-kernel-broker --lib only_exact_drain_for_submitted_request_becomes_exit_status --no-default-features`
- `cargo test --locked -p oulipoly-agent-runner --bin oulipoly-agent-runner two_paired_entries_require_same_broker_generation_before_dispatch --no-default-features`
- `cargo build --locked -p oulipoly-kernel-broker --bin oulipoly-kernel-broker --bin oulipoly-installed-launcher -p oulipoly-agent-runner --bin oulipoly-agent-runner --no-default-features`
- Runner's focused test and featureless binary build were repeated after the
  final schema-1 compatibility adjustment.
- `cargo fmt --all --check` and `git diff --check`

Existing unused-code warnings remain. Full CRW cohort review, workspace-wide
stress, disposable private-root installed end-to-end, and host-root
service/alias checks were not performed under this fast source handoff. No
claim of production readiness follows from the positive predicate tests.
