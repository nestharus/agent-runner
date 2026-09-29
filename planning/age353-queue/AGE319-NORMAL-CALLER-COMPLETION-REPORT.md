# AGE-319 normal caller completion — 2026-09-29

Base: `79497f45`. **Production installed CLI `L` is open** on this branch.

## Repair

The prior capacity-interrupted run's log showed F/receipt/ACK and caller
publication, but State still read `terminal_commit_absent`; other runs left the
launcher pending through owner close. A focused busy four-image case reproduced
the first defect after the launcher exited: State's terminal had
`execution_state="unknown"` instead of `success`.

The normal Runner now commits the root terminal through the original D-bound
`SettleRootTerminal` request after async recipient settlement and before caller
publication. It accepts a lost settlement reply only when exact readback retains
the same handoff, D, session, and parent K admission. Broker's normal and
offline status paths now keep a request pending during the interval after PID1
exit and before owner-close publication when the full physical close proof
revalidates. This addresses an observed `unknown` status from a help launcher
after PID1 had published its terminal. These changes do not mint another K, F,
ACK, L, or owner-close authority.

The featureless test now asserts a successful retained State terminal and ACK
for each async branch. Its focused busy mode skips the unrelated preliminary
sync Bash launch; the default paired mode still runs the complete test. Longer
fixture deadlines accommodate the observed setup and status latency and are not
the production repair. Temporary diagnostic logging was removed.

## Proof

Commands ran in this worktree with `--locked --no-default-features` and the
featureless Bash image at `src-tauri/target/age319-bash-featureless/debug/agent-bash`.
The test copies Runner, Broker, launcher, and Bash into a disposable activated
schema-2 pair under a private user/mount/PID namespace.

```sh
AGE319_FEATURELESS_ONLY_BUSY=1 \
AGE319_FEATURELESS_RUNNER_BIN=$PWD/src-tauri/target/debug/oulipoly-agent-runner \
AGE319_FEATURELESS_BASH_BIN=$PWD/src-tauri/target/age319-bash-featureless/debug/agent-bash \
cargo test --locked --quiet --no-default-features -p oulipoly-kernel-broker \
  --test age319_featureless_installed -- \
  --exact disposable_root_featureless_l_help_lost_reply_and_duplicate --nocapture

AGE319_FEATURELESS_RUNNER_BIN=$PWD/src-tauri/target/debug/oulipoly-agent-runner \
AGE319_FEATURELESS_BASH_BIN=$PWD/src-tauri/target/age319-bash-featureless/debug/agent-bash \
cargo test --locked --quiet --no-default-features -p oulipoly-kernel-broker \
  --test age319_featureless_installed -- \
  --exact disposable_root_featureless_l_help_lost_reply_and_duplicate --nocapture
```

| Check | Result |
|---|---|
| Focused command above before repair | Failed at the new State terminal assertion: `unknown` versus `success` (269.18 s). |
| Same focused command after repair | Passed (265.63 s), including busy W, F/receipt/one-use ACK, caller completion, and retained State terminal. |
| Default paired command above with production guard retained | Passed busy and sleeping branches (276.22 s and 178.44 s). |
| `cargo build --locked --no-default-features -p oulipoly-agent-runner --bin oulipoly-agent-runner -p oulipoly-kernel-broker --bin oulipoly-kernel-broker --bin oulipoly-installed-launcher` after guard removal | Passed. Bash image was already built featureless and its source was unchanged. |
| Default paired command above after removal of the explicit production `L` refusal | Passed busy and sleeping branches on final code (245.48 s and 137.56 s). |
| `cargo fmt --all --check`; `git diff --check` | Passed. |

Both default paired cases exercise exact image/generation refusal before ledger
reservation, normal K/Q and caller publication, owner close, installed status,
Broker restart, and later launch. The busy case also checks duplicate ACK
refusal, receipt tamper refusal, and no repeat effect. The sleeping case checks
the exact successor decision/start/candidate/ACK chain. The final test output
is in ignored `src-tauri/target/age319-paired-l-open.log`.

The proof does not install to fixed host paths or exercise the actual non-root
launcher/root Broker UID split. Host image bytes, activated root State,
`/run/oulipoly-kernel-broker/control.sock` group access, and that UID split
remain for the parent-owned host check. No sudo or host install was run.
