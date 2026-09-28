# AGE-319 first-install activation and Broker route readback

Source slice: `age319-first-install-activation-20260928`, based on `9b003534`.
Status: implemented in source; no host activation or installation performed.

## Boundary delivered

- The installed Broker has an explicit offline root command,
  `--activate-first-install-v30`. It uses only the fixed schema-2 manifest and
  four image paths under `/usr/local/libexec/oulipoly`, with the fixed empty
  Broker State root at `/var/lib/oulipoly-kernel-broker`. The running Broker
  image must be the named installed Broker inode.
- The command first reads back the exact empty-v30 bootstrap identity. It
  refuses a prior service start, an old/unmarked root, nonempty State tables,
  a nonempty fresh-provider ledger, an interrupted activation stage, a wrong
  schema or any changed/missing/untrusted image. It hashes the manifest and
  all four images, records each SHA-256/device/inode, and keeps bootstrap
  `source_generation` distinct from installed `pair_generation`.
- A root-only 0600 record is file-synced, atomically published without
  replacement, and directory-synced. Retry validates and returns the same
  record. A partial stage is retained and refuses automatic retry. Broker
  startup validates the record against the current bootstrap source and all
  named files before it can report `fresh-only-open`; startup never creates
  the record.
- The old control socket reports `fresh-only-open` only for that validated
  binding. Installed-pair readback returns the pair generation and a separate
  source generation. Schema-2 pairs without activation report
  `broker-v30-closed`, including on an old root. The observation has a
  dedicated `0x91` opcode because `v` is already a fresh Bash request.
- Runner and legacy recipient routing reject `fresh-only-open`. Production
  installed L execution remains closed; this record does not admit a launch.

## Verification

- Root-mapped private fixture: exact activation, retry, schema-1 and changed
  preactivation image/mode refusal, partial stage and record refusal, nonempty State refusal,
  old and previously served root refusal, two Broker starts with exact route
  readback, changed pair refusal, and equal-byte replacement inode refusal.
- Focused protocol parser unit test covers the new five-field fresh-only
  readback and malformed responses.
- `cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_first_install_activation --test age319_empty_v30_startup -- --nocapture` — **passed**, 2 fixture tests (each root-mapped child also passed).
- `cargo test -p oulipoly-kernel-broker --lib installed_pair_response_tests --no-default-features` — **passed**, 1 protocol test.
- `cargo test -p oulipoly-agent-runner --bin oulipoly-agent-runner two_paired_entries_require_same_broker_generation_before_dispatch --no-default-features` — **passed**, 1 Runner entry-route test. An earlier `--lib` filter matched zero tests and is not counted.
- `cargo check -p oulipoly-kernel-broker -p oulipoly-agent-runner --no-default-features` — **passed** (existing unused-code warnings).
- `cargo fmt --all --check` and `git diff --check` — **passed**.

## Remaining boundary and review gap

Production L execution, installed launcher result/status, Runner fresh-only
entry and Bash production dispatch are separate slices. No host service,
alias, installed binary, real State or ticket was changed. This was an inline
source review and focused test pass; the full CRW cohort and workspace-wide
stress tests were not run under the requested fast handoff.
The private fixture does not replace a real fixed-path installation or
concurrent image-replacement stress test.
The known `age319_fresh_dual_lane` schema 32 versus source 38 assertion was
not exercised by these focused commands.
