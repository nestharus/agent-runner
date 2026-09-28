# AGE-319 featureless Broker ordinary Bash dispatch — 2026-09-28

## Result and commits

The featureless Broker now accepts the ordinary Bash v30 parent probe, X/c, physical ^/9, source %, and sync v/u route from a pinned installed Bash image. The private Broker and featureless Bash exercised the same dispatch branch against a disposable fake workload. The actual featureless Broker image builds and its challenged frame tests pass. An installed host-root service run was outside this work unit.

| Repository | Base | Head used for proof |
|---|---|---|
| Runner | `bbd331f9e5dee9b522d561a3fd026738d2c4319b` | `43a03ce59e1c27059582f3da7a16c5477fdbfaeb` (implementation; this report follows) |
| Bash | `6b553927982ccb0624de6cad5b6187d5589c8763` | Same head; no Bash source change or Runner SHA pin update |

No installed binary, service, live State/config/DB, selector, or AGE-353 stress was touched. Root owns PR, merge, and cleanup. There were no descendants.

## Production boundary

The existing schema-2 installed pair manifest requires `bash_sha256`. At Broker startup, the featureless lane opens `/usr/local/libexec/oulipoly/agent-bash`, checks its root-owned path and exact manifest digest, and retains that file descriptor for process-image comparison. A schema-1 pair has no Bash image authority: the Bash route refuses with `installed Bash image absent`. A fixture environment path is considered only in a private-feature build running as the private fixture.

Every ordinary request reattests the connected Bash executable, its pinned process identity, released v30 root and live PID1, root D/session, and consumed causal parent work. X selects the original sealed CLI argv, cwd, environment, executable intent, and source. c readback requires that exact ordinary selection. Before ^/K, the Broker checks the original sealed command again, including the post-C veto and K digest, and binds it to the same child and source. One physical K is observed by 9/Q; %/W accepts the selected tree event. The completion worker repairs Q/W after a restart. Sync v reserves one publication and sends Q-verified stdout/stderr by descriptor; u reports an uncertain reservation without replaying stream bytes. Async keeps its receipt and notification evidence.

Featureless dispatch rejects C/E/O/8/!/</private Bash modes. Private fixed recipes, synthetic entry syntax, and reply-drop/fault hooks remain behind `age319-private-broker-fixture`. Other provider, account, interactive, and manual routes still have private dispatch gates. The installed selector and host-root service remain closed.

## Verification and frozen evidence

Evidence is in `/home/nes/.local/share/age319-production-bash-dispatch-20260928/`. `SHA256SUMS` covers the exact images, proof scripts, and logs; `sha256sum -c SHA256SUMS` passed. The scripts show the complete environment and exact test binary invocation. Each private test used a disposable `OULIPOLY_DATA_DIR`, the frozen Runner/Broker images, and an explicit Bash image.

| Command or case | Result and evidence |
|---|---|
| Bash `cargo build --locked` and `cargo build --locked --features private-v30-admission` | Pass; `logs/bash-featureless-build.log`, `logs/bash-private-build.log` |
| Runner `cargo build --locked -p oulipoly-kernel-broker --bin oulipoly-kernel-broker` with no feature | Pass; `logs/featureless-broker-build-final.log` |
| Runner same build with `--test private_root_join --features age319-private-broker-fixture` | Pass; `logs/private-broker-build-final.log` |
| Featureless Broker challenged 0x90/9/%/v/u wire test | 1 passed; `logs/featureless-broker-wire-test-final.log` |
| `cargo test --locked -p oulipoly-kernel-broker --lib installed_pair -- --nocapture` | 3 passed, including missing/wrong Bash image and schema-1 no-Bash behavior; `logs/installed-pair-tests-final.log` |
| `python3 packaging/linux/test_paired_bundle.py` | 1 passed; `logs/paired-bundle-test.log` |
| `run-private-proofs.sh`: ordinary sync, async, restart | All passed with one fake workload, exact Q/W, sync binary streams/exit or async notification; corresponding `logs/ordinary-*.log` |
| Same script: wrong image, parent tamper, unsupported root/ready/cancel | Passed; wrong-image c readback refused, changed causal parent blocked K, unsupported modes refused before child K; `logs/ordinary-sync.log`, `logs/ordinary-parent-negative.log`, `logs/ordinary-unsupported-negative.log` |
| Same script: lost C/K/W replies and lost sync-begin reply | Passed exact readback/reconciliation with one physical effect; lost publication returned unknown without caller stream bytes; `logs/ordinary-c-k-w-loss.log`, `logs/ordinary-sync-begin-loss.log` |
| Real Codex F turn with private Bash | Passed; `logs/f_turn.log` |
| Real Codex F old-pending debt | First attempt failed closed with `Bash child scope uncertain` before native Bash result; the exact rerun on the same frozen images passed and checked refusal of the pending second entry. Both outcomes are retained in `logs/f_turn_old_pending_debt.log` and `logs/f_turn_old_pending_rerun.log`. |
| `cargo fmt --all -- --check` in both repos; Runner `git diff --check` | Pass; `logs/runner-fmt-check-final.log`, `logs/bash-fmt-check-final.log` |

Frozen image SHA-256:

| Image | SHA-256 |
|---|---|
| `featureless-broker` | `8d2f16f5708f9e061101b568edf191e5a976a84260cc59067b1fdb185d78abb4` |
| `private-broker` | `c2fae842255be1a4996dea807bdd6727c89fb01df53e3a067a21c6c8bbfaa839` |
| `featureless-bash` | `85eb75d2ac5831b8d18e936ca0c579af6902d8b1c32bb295bc9fc16a861b31f2` |
| `private-bash` | `362bbb3afd33c40d1184ad3f8ad2c3e0264edd0b9abff9efaa6fa4a1b944a860` |
| `oulipoly-agent-runner` (retained private image and Bash pin) | `a255cc28ecd617be57bcd41605dd4bd3eec3ec7c6339feb11d92ac283d043521` |
| `age319-fresh-provider-fixture` | `10f5b8ca85b9459e43d24ac283b3a6d6bd1c43971cadf61da2a4711b2aba45ee` |
| `private_root_join-bb1f7b84041bb108` | `80599aff6b6bbf2047673096268bfbf4efdfb8cbeae25adde202b974146f540c` |

## Limits and next boundary

The disposable private Broker proof runs the production ordinary branch with a featureless Bash image. The featureless Broker was built and its wire parser checked, but it was not run as the installed host-root service: production startup requires root-owned installed paths and the exact pair, while this work unit forbade an install or service change. The next boundary is root-owned installed-pair deployment with the exact Bash digest, followed by a controlled host-root service dispatch test and separately owned selector review. The first old-pending F attempt is an unresolved timing-sensitive fail-closed observation; the passing rerun does not erase it.
