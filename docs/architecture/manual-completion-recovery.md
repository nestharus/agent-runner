# Manual selected completion recovery

An interrupted local caller can inspect an accepted `completion-continuation-v2`
event without its Bash handle, latest local receipt, synchronous response, or
mailbox row. This is a read-only local CLI path using normal filesystem access.
It does not activate a listener, notify an agent, acknowledge a mailbox item,
rerun a workload, or record physical drain.

```sh
oulipoly-agent-runner notify agent-bash-recovery-list --session-id SESSION_ID
oulipoly-agent-runner notify agent-bash-recovery-list
oulipoly-agent-runner notify agent-bash-recovery-list --session-id SESSION_ID --offset 100
oulipoly-agent-runner notify agent-bash-recovery-read --event-id EVENT_ID --output /private/recovered.bin
```

List pages contain up to 100 **accepted** event identities, newest first.
Omitting `--session-id` searches all locally retained accepted events under
the same OS permissions. An event ID from a lost response may be used directly.
Read verifies the committed
event JSON against its content-addressed path, byte length and SHA-256. For a
raw artifact it also checks the selected descriptor against the Runner-owned
copy, then streams at most 1 GiB through a 64 KiB buffer and verifies exact EOF
and SHA-256. `--output` publishes only a verified copy and refuses to replace an
existing file. Omitting `--output` verifies and reports metadata without copying.

`selected_output.kind` distinguishes `raw_bytes`, `lossy_inline_legacy`, and
`selected_missing_output`. New Bash snapshots use raw artifacts for every size.
Older inline strings are preserved as text with a digest of their **retained
text**, but cannot recover original invalid UTF-8 bytes; `--output` rejects
them. A validated missing-original-output event has no selected body to copy.

`source_acceptance` is the committed source acceptance. `presentation_and_ack`
reports each registered listener's policy, mailbox placement, transport
evidence and ACK separately, including response-only listeners with no row.
`physical_drain` distinguishes the source's reported tree-drain flag from
Runner attempt phases and exact receipts. No receipt or `drained` phase is
inferred from acceptance, presentation, a missing Bash source,
or the manual read itself.

Accepted v2 event payloads remain protected from terminal payload reclamation
by the accepted-source reference. Runner-owned raw output copies have no
automatic day-30 deletion path. Open/unacknowledged completions therefore do
not silently expire at day 30. Closed-record retirement is a separate policy;
this command does not implement cleanup or delegated cross-session ACK.
