# AGE-319 v30 successor admission slice

## Delivered boundary

An installed Runner process can offer itself for one exact pending v30 session row. The Broker derives its process identity from the pinned socket peer and retains an immutable offer with a new successor generation, source/attempt, root/owner, payload identity, and the original recipient identity. The offer has no F or ACK authority.

Only the original released D-bound root, on its own pinned Broker socket, can approve that offer. The Broker checks the offered process is still live, has the same installed Runner image, and matches the offered PID, boot, start time, and PID namespace identity. State commits the immutable successor admission first; the Broker sidecar then commits the matching admission. Exact readback requires both rows and the original State recipient attachment to agree. A partial State-only write is reported as incomplete and the original root can repair it with the same offer ID. A committed State row already fences original F, including across Broker restart. The original `fresh_lane_session`, `fresh_lane_recipient_attachment`, and `fresh_recipient_binding` remain unchanged.

The new Broker recipient opcodes are `OfferSuccessor`, `ReadSuccessorOffer`, `AdmitSuccessor`, and `ReadSuccessorAdmission`. A private installed Runner entry and original-root control path exercise the complete socket-authenticated transition in the connected fixture. Production Runner scheduling of a successor is not wired in this slice. This is an admission/readback slice: the successor cannot submit F or ACK, and no terminal or owner-close code treats admission as delivery.

## Verification

- `cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_successor_admission durable_exact_successor_offer_and_cross_store_readback -- --nocapture` — passed. Separate live peer offer and duplicate readback; wrong original/sibling peer, source, row, and already-ACKed row refused; forced State-only partial write fenced original F and required exact repair; Broker restart retained successor readback. Successor F remained refused.
- `AGE319_CONNECTED_RUNNER_BIN=... AGE319_CONNECTED_BASH_BIN=... cargo test -p oulipoly-kernel-broker --features age319-private-broker-fixture --test age319_connected_control root_mapped_connected_l_admits_distinct_v30_successor_without_f_or_ack -- --nocapture` — passed with current Runner and a current Bash image built from the read-only Bash trunk into a temporary Runner-worktree target. Actual released original root rejected an exited candidate, approved a distinct live Runner successor after real Bash W and Q, recovered a deliberately lost approval reply, and read the same admission on duplicate approval. State and Broker generations matched; original attachment persisted; no F grant or ACK evidence was written. The fixture intentionally stopped before root terminal settlement.
- Existing `root_mapped_connected_l_delivers_real_bash_async_to_recipient_and_acks` and `private_fresh_recipient_delivery_ack_collision_and_restart` tests — passed as regression checks for the original recipient route.
- `cargo check -p oulipoly-state -p oulipoly-kernel-broker -p oulipoly-agent-runner` (default), `cargo check -p oulipoly-state -p oulipoly-kernel-broker --features oulipoly-kernel-broker/age319-private-broker-fixture`, `cargo check -p oulipoly-agent-runner --features age319-private-broker-fixture`, `cargo build -p oulipoly-agent-runner --features age319-private-broker-fixture`, `cargo fmt --all --check`, and `git diff --check` — passed.

## Next explicit obligations

1. Add successor F submission and exact recovery for **only** the admitted generation and row/source. Bind the grant and retained bytes to the Broker-pinned successor peer; a copied offer ID, session ID, or delivery token must not authorize another process. Preserve the original F fence and test competing original/sibling/stale peers and lost replies.
2. Add successor receipt persistence and one-use token ACK, with immutable evidence that joins the admission generation, source/attempt, row payload, actual receiving peer, and ACK. Test no ACK without receipt, wrong peer/token, duplicate ACK, and restart readback.
3. Extend fresh root terminal and owner close to join that exact successor F/ACK evidence while keeping the original root actor and original attachment historical. A partial admission or pending F/ACK must remain terminal unknown, not accepted. Then run an installed sleeping-recipient proof with a real wake/receive path; this report does not claim sleeping acceptance.
4. Wire the production Runner decision and control route that starts or selects a successor for pending work. The private connected entry proves the authenticated Broker path but does not schedule production successors.

No PR, merge, host installation, host State change, service change, or provider API call was made.
