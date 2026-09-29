# AGE-319 v30 successor terminal and owner-close join

## Delivered boundary

The original released D-bound root's terminal notification changes from
`pending_f` to `acked` only when one exact admitted successor has an ACK. The
terminal reads the immutable State admission and matching Broker sidecar
admission, the original State recipient attachment and Broker binding, the
selected W/source/attempt and retained payload, the successor's separate F
grant and authenticated UID, its independently certified physical receiver
receipt, and the immutable ACK and delivered mailbox row. The structured
`successor_ack` certificate names the offer, generation, request, grant,
session/row, source/attempt, root/owner, pinned successor identity and UID,
payload, and receipt digest. The original recipient attachment and original F
grant/ACK tables are not repurposed. Original F remains fenced by State
admission.

State-only or sidecar-only admission, no F, F without ACK, missing or mismatched
ACK, and changed receipt leave notification unsettled and close blocked. The
terminal records a successor unknown stage for inconsistent evidence. Normal
owner-close preflight and its writer-fenced commit require the same exact
readback; the resulting physical close proof embeds the successor ACK.
Restart/readback revalidates that proof, and the installed normal terminal
certificate carries and revalidates the same ACK. A changed receipt makes both
closed-owner readback and installed caller status unknown. No additive schema
change was needed: the v30 State admission, sidecar grant, receipt, and ACK
tables already supply the join.

The private connected root now continues after successor ACK through normal
caller publication, root drain, owner close, and installed terminal
certification. It checks `pending_f` immediately after admission and `acked`
after ACK, while its original attachment remains historical. Its deliberate
lost admission reply still recovers by exact request ID. The socket fixture
also recovers lost F/ACK replies and verifies ACK after Broker restart.

## Verification

- `cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_successor_admission durable_exact_successor -- --nocapture --test-threads=1` — passed, including partial-admission, pre-F, pre-ACK, mismatched ACK/source, changed receipt, wrong peer/UID, lost-reply, and restart checks.
- `cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_fresh_recipient_socket private_fresh_recipient_delivery_ack_collision_and_restart -- --exact --nocapture` — passed.
- With current private Runner and featureless Bash images, `cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_connected_control root_mapped_connected_l_delivers_real_bash_async_to_ -- --nocapture --test-threads=1` — both original-recipient and successor connected cases passed. The successor case checks exact terminal, owner proof, and installed certificate evidence.
- With the same images, `cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_connected_control root_mapped_connected_l_successor_ -- --nocapture --test-threads=1` — Broker restart and changed-receipt certificate refusal passed.
- `cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture root_drain::tests -- --nocapture`, `cargo check -p oulipoly-state -p oulipoly-kernel-broker -p oulipoly-agent-runner --features oulipoly-kernel-broker/age319-private-broker-fixture`, `cargo fmt --all --check`, and `git diff --check` — passed. The Rust check emitted pre-existing unused-code warnings.

The Bash source came from the clean, read-only Bash trunk; its build target was
inside this Runner worktree. No Bash trunk mutation, host installation, host
State or service change, PR, merge, or descendant dispatch was made.

## Remaining boundary

Production successor selection/scheduling and sleeping-recipient wake are
separate work. The connected test uses a private installed Runner successor and
real featureless Bash async child; it does not establish those later paths or
claim host deployment readiness.
