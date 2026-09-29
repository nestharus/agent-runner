# AGE-319 broker-certified original receiver receipt — 2026-09-29

## Disposition

The featureless busy async original Runner now writes the actual returned F
bytes to the exact receipt path issued by Broker. Broker independently opens
that physical file, certifies its content and inode against the pinned grant,
row, source, attempt, payload, recipient identity, UID and token, and requires
that immutable certificate before accepting the one-use manual ACK. The ACK
transaction locks the same certificate identity. Terminal, root drain, owner
close, installed certificate and later-entry readback revalidate the file;
loss or alteration makes the original F evidence unknown and blocks a settled
close. This closes the evidence gap in the prior featureless async report.

This is **not production readiness**. The sleeping successor obligation and
real host non-root Runner/root Broker UID split remain unproved. Host-root
admission is still closed.

## Evidence design

- Broker derives `<fixed-root>-original-receipts/<authenticated-uid>/<grant>.json`
  outside root-only State. It creates a root-owned `0711` receipt root and a
  UID-owned `0700` child. F fixes that UID beside the grant and returns the
  exact path. The child Runner creates a `0400` receipt exclusively from the
  F reply bytes, then fsyncs the file and child directory before certification.
- The receipt includes the request ID, grant without the secret token, pinned
  recipient identity and UID, exact payload bytes, and token digest. Broker
  opens with `O_NOFOLLOW`, checks owner, mode, link count, size, named inode and
  content, and compares decoded bytes with the retained F row at certification.
  The immutable State certificate records the digest, device and inode plus
  the joined grant/row/source/attempt/payload/identity/UID/token fields.
- Manual ACK requires a valid certified file, exact token and pinned socket
  peer/UID. Its mailbox delivery, ACK evidence and immutable receipt lock are
  committed in one transaction. Subsequent reads reopen the same named file
  and recheck the immutable joins and physical identity. A changed or missing
  file cannot be read as an ACK or an `acked` original terminal. Root inventory,
  owner proof and installed certificate carry the exact receipt identity; a
  direct installed-ledger read revalidates it as well.
- Native and delegated ACK paths retain their separate evidence. Original
  manual ACK cannot use a successor offer, and successor admission tests remain
  green. Lost F reply recovery keeps the original request ID; lost ACK reply
  uses the exact ACK readback. Existing file collision and token replay fail
  closed.

## Focused observations

- Original-recipient socket test: passed. It independently withheld the file
  before ACK, altered it before certification, discarded certification and ACK
  replies, retried readback, rejected duplicate ACK, removed and replaced the
  file after ACK, and restored it. Wrong recipient, token and collision controls
  passed. A mapped UID 1001 receiver wrote its UID-owned receipt while root-only
  State remained inaccessible to that process.
- Featureless installed async test: passed in 167.44 s after the final direct
  certificate readback change. It used real Bash, featureless Runner, Broker
  and launcher images, discarded F and ACK replies, rejected ACK replay, and
  checked the physical receipt against the selected W and raw child bytes. The
  same receipt identity appeared in the original terminal, physical owner
  proof and installed certificate. After Broker restart, tampering invalidated
  direct installed-certificate readback and terminal settlement and refused a
  later entry without another effect; restoring the original bytes allowed the
  later entry. Earlier final-schema run passed in 174.37 s. An earlier run
  timed out at the 70 s launcher wait before final F and was superseded by these
  completed runs; timeout diagnostics were retained in the test.
- Successor admission suite: 4 passed, including exact successor F/ACK and
  cross-store readback. The installed test also retained the normal and Bash
  sync controls from the prior featureless case.

## Commands and results

- `cargo test --locked -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_fresh_recipient_socket private_fresh_recipient_delivery_ack_collision_and_restart -- --exact --nocapture` — passed.
- `cargo test --locked -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_successor_admission -- --nocapture` — 4 passed.
- `cargo build --locked -p oulipoly-agent-runner -p oulipoly-kernel-broker --no-default-features` — passed; Broker rebuilt once more after the direct-ledger read change.
- `cargo check --locked -p oulipoly-agent-runner -p oulipoly-kernel-broker --no-default-features -q` — passed after the final change (existing unused-code warnings).
- `cargo check --locked -p oulipoly-agent-runner -p oulipoly-kernel-broker --features age319-private-broker-fixture --no-default-features -q` — passed.
- `AGE319_FEATURELESS_RUNNER_BIN=$PWD/src-tauri/target/debug/oulipoly-agent-runner AGE319_FEATURELESS_BASH_BIN=$PWD/src-tauri/target/age319-bash-featureless/debug/agent-bash cargo test --locked -p oulipoly-kernel-broker --test age319_featureless_installed --no-default-features -- --nocapture` — passed after final change (167.44 s).
- The Bash image was built from the read-only Bash trunk with output confined
  to this worktree's `src-tauri/target/age319-bash-featureless`.
- `cargo fmt --all -- --check`, direct `rustfmt --edition 2024 --check` for the
  included receipt module, and `git diff --check` — passed.

## Remaining gates

The sleeping successor still lacks the production durable scheduling and
recovery obligation. The mapped UID 1001 socket fixture proves filesystem and
peer mechanics in a disposable namespace; it does not prove the real host
non-root Runner/root Broker installed path. Early provider exit before the
selected child W is also unproved. Those cases and AGE-353 busy/sleeping
benchmarks remain outstanding before host-root admission or a production claim.
