# AGE-319 v30 successor F / receiver receipt / ACK

## Delivered boundary

An exact live Broker-pinned successor peer can submit F only for its own
cross-store admitted generation and the admission's original pending
session/row/source/attempt. The Broker creates a separate durable successor
grant and one-use token, then returns its verified retained payload bytes.
Exact F readback and recovery require that same pinned process identity; a
copied offer, session, grant ID, or token cannot authorize another peer.
Original F remains fenced by the immutable State admission. The original
recipient attachment and original grant/ACK tables are unchanged.

The receiving Runner constructs an exclusive 0400 receipt from the actual F
reply bytes, then fsyncs the file and parent directory. Broker independently
opens the fixed grant-specific receipt path without following a symlink,
checks its physical identity and exact grant/generation/row/source/attempt,
recipient identity, payload digest/length, and token digest, and commits a
separate immutable receipt certificate. ACK requires that certificate and
revalidates the same physical file. One transaction updates the mailbox row,
consumes the grant, and inserts immutable ACK evidence joined to the sidecar
admission, source row, receipt, and token. State admission and original
attachment are revalidated at F, receipt, ACK, and exact readback. A lost F or
ACK reply is recovered through the same request ID and live peer without
repeating the effect.

The private installed Runner entry and focused socket fixture exercise this
boundary. Production successor scheduling remains outside this slice. Root
terminal and owner close do not claim successor delivery: the connected root
still reads `pending_f` after successor ACK.

## Verification

Commands run in the specified Runner worktree, except the Bash build, which
read the clean Bash trunk and wrote its target under the Runner worktree:

```bash
CARGO_TARGET_DIR="/home/nes/projects/agent-runner/worktrees/age319-v30-successor-fack-20260928/src-tauri/target/age319-bash-featureless" \
  cargo build --locked --bin agent-bash
# Bash command working directory: /home/nes/projects/agent-bash-tool/trunk
cargo build -p oulipoly-agent-runner --features age319-private-broker-fixture
cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture \
  --test age319_successor_admission durable_exact_successor -- --nocapture --test-threads=1
cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture \
  --test age319_fresh_recipient_socket \
  private_fresh_recipient_delivery_ack_collision_and_restart -- --exact --nocapture
AGE319_CONNECTED_RUNNER_BIN="$PWD/src-tauri/target/debug/oulipoly-agent-runner" \
AGE319_CONNECTED_BASH_BIN="$PWD/src-tauri/target/age319-bash-featureless/debug/agent-bash" \
  cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture \
  --test age319_connected_control \
  root_mapped_connected_l_delivers_real_bash_async_to_admitted_successor_and_acks \
  -- --exact --nocapture
AGE319_CONNECTED_RUNNER_BIN="$PWD/src-tauri/target/debug/oulipoly-agent-runner" \
AGE319_CONNECTED_BASH_BIN="$PWD/src-tauri/target/age319-bash-featureless/debug/agent-bash" \
  cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture \
  --test age319_connected_control \
  root_mapped_connected_l_delivers_real_bash_async_to_recipient_and_acks \
  -- --exact --nocapture
cargo check -p oulipoly-state -p oulipoly-kernel-broker -p oulipoly-agent-runner
cargo fmt --all --check
git diff --check
```

All commands passed. The default Rust check emitted existing unused-code
warnings. Two earlier connected attempts stopped because the fixture compared
the initial `unknown` F phase to the later `submitted` readback; the assertion
was corrected to preserve the immutable grant comparison, and the final
connected run passed. A test-only mutability edit caused one compile failure;
it was restored before the final two-case socket run.

- Focused successor socket: admitted peer receives the original retained
  bytes; original and sibling peers with copied IDs/token cannot F, recover,
  or ACK. ACK fails with no receiver receipt, before certification, with a
  changed receipt, and with a wrong or reused token. Deliberately lost F and
  ACK replies recover exactly; one mailbox delivery attempt and the joined
  receipt/ACK rows persist across Broker restart. The fixture also reopens a
  published v30 admission lane missing the new delivery tables and checks the
  additive schema upgrade.
- Connected first install: real featureless Bash and current private Runner
  image produce one physical async child effect. The admitted successor's
  received bytes equal Broker's retained payload file; the decoded event and
  physical output/drain files agree on Bash W/Q and stdout/stderr. State and
  sidecar admissions, F grant, fsynced receiver receipt, and ACK evidence join
  on generation, original row, source, attempt, payload, peer, and token.
  Original attachment remains intact and terminal reports `pending_f`.
- Existing original recipient socket ACK/collision/restart and connected
  original async Bash recipient regressions pass.
- Rust checks, `cargo fmt --all --check`, and `git diff --check` pass.

## Remaining boundary

Terminal settlement and owner close still need a coherent successor evidence
join. A pending successor ACK remains unknown to terminal. Production Runner
selection/start of a successor and sleeping-recipient wake acceptance remain
future work; this slice claims neither.

Independent delegated review was not run because this work unit forbids agent
descendants. The parent owns PR, review, merge, and cleanup.

No PR, merge, host install, host State/service/alias change, real provider API
call, Bash trunk mutation, extra worktree, or agent descendant dispatch was made.
