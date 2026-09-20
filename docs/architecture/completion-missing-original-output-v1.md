# Canonical missing-original-output wire — revision 5

Runner implements this contract; newest-pair validation remains root-owned. Additive to revision4, whose successful inline/artifact bytes and semantics remain unchanged. Exact fixture: [`age360-missing-output-wire.json`](../../crates/oulipoly-state/tests/fixtures/age360-missing-output-wire.json); the machine-local planning publication is `age360-runner/missing-output-wire.json`. Hash retained UTF-8 bytes, not reserialization. Existing registration, source outcome and snapshot filenames, admission, complete/readback/listen/ACK routes remain unchanged. No new authority table, protocol alias, source replay, or installed-domain transition.

A snapshot may use `output.representation="missing-original-output-v1"`. Its `status` MUST be `original_output_unavailable`. Its rc and original outcome remain the ACTUAL original event evidence (including nullable raw wait, cancellation and readiness); this status is NOT workload failure or successful readiness. No fabricated outcome is permitted. Without a retained genuine original event/outcome, this variant cannot be published: keep pending uncertainty. An unavailable output is neither an empty output nor a complete-body attachment.

All object fields required (nullable fields explicitly null), unknown fields rejected:
- `representation`, `capture_state="irrecoverable"`, `reason`, `detail` (nonblank, <=4096 bytes).
- `producer`: actual observer of permanent selection/capture loss, process identity `{pid,boot_id,starttime_ticks}`. Original observer or exact completion-only successor; never runner-generated or inferred from a read error.
- `original_observer`: exact original outcome observer; `completion_revision` and `outcome_sha256`: exact original outcome linkage (outer snapshot already binds registration/incarnation/digest and outcome bytes).
- `selection`: null or `{device,inode,byte_len}` describing the ORIGINAL retained inode/exclusive prefix; byte_len <=1GiB, inode nonzero. Not mutable-log resampling.
- `observed_byte_len`: null or actual length observed on that SAME selected inode.

Reasons (exhaustive):
1. `original_selection_not_retained`: selection=null, observed_byte_len=null. Producer established original event's selection was never durably retained AND is no longer recoverable from the original owner. Failed open/pin alone while original selection remains recoverable is NOT enough.
2. `selected_storage_lost`: selection nonnull, observed_byte_len=null. Producer established original retained inode lost with no surviving recoverable pin/body. A transient ENOENT, inaccessible mount, EACCES, EIO, owner death or pending recovery alone is NOT proof.
3. `selected_storage_short`: selection nonnull, observed_byte_len nonnull and strictly less than original byte_len. Producer established irreversible shortening of the SAME original inode, with no complete retained body; not an incomplete in-progress copy.

The producer owns causal attestation; runner validates the bounded public representation and all joins, not Bash's private staging schema and not independent filesystem omniscience. Bash must retain attributable actual observations before immutable publication; preserve original header/outcome, selection and failures. Native acceptance retains the proof in the recipient payload. Missing output does NOT imply physical drain, artifact cleanup, cancellation, delivery or ACK.

Recovery reply is `status=source_output_missing`, plus common identity and exact snapshot/outcome hashes, only after public files are durable. `source_ready` MUST NOT describe missing output, and source_output_missing MUST NOT describe successful output. Pending/unavailable responses remain diagnostic only. Native owner accepts through existing authority; accepted registrations stop capture/recovery retries, retain notification for ordinary delivery, late listeners and exact ACK. Outstanding physical attempts/artifact duties remain independent.

## Bash counterpart inputs
Read this contract plus exact fixture. Implement public snapshot producer/recovery status separately; e99b53598788b4de9cc7630e00bdc80f3ce7c3b6 private missing-capture staging is NOT valid public wire. Preserve revision4 successes. Need native paired cases: original header/no recoverable selection after actual owner loss; original pin irreversibly lost/short; transient read failure remains pending; exact recipient missing-output payload, original raw wait/readiness, late listener and ACK; late cancellation/actual drain and retained artifact custody. Root owns dispatch and newest-pair execution. Do not claim e99b535 already emits this capability.

## Runner fixture entry points

`age360_completion_continuation::native_missing_output_rejects_transient_then_delivers_original_wait_without_capture_retries` runs without Bash. Its explicitly runner-only producer owns a real child/wait and observes actual deliberately shortened original storage, then uses public source admission, transient rejection, independent owner acceptance, real recipient readback and exact ACK. This tests native materialization, not the Bash producer. It uses private user/network/PID/mount namespaces.

With `--features age360-fault-fixtures` and explicit `AGE360_AGENT_BASH_BIN`, the same target provides `paired_missing_selection_notifies_original_ready_after_owner_loss_and_cancel`, `paired_missing_pin_notifies_original_ready_after_owner_loss_and_cancel`, and `paired_short_selected_inode_notifies_original_ready_after_owner_loss_and_cancel`. These expect the Bash candidate's existing `selection-error` / `before-output-capture` fault hooks plus its new revision5 public producer. They fault only private selected storage, never write public source evidence, then demand unchanged original ready evidence, actual cancellation drain and recipient missing-output receipt/ACK. They are support for root's pairing, not executed-success claims. Counterpart e99b535 has no public revision5 capability.

The retained immutable payload is the recipient's canonical missing-output evidence. The compact native prompt directs the recipient to distinguish the explicit missing representation from empty output or workload failure; it does not read potentially large bodies to render the prompt. No `output_artifact` is invented. Successful revision4 payload materialization is unchanged.

Fixture metadata uses `fixture_revision=5`; embedded successful-output revision4 bytes are retained unchanged. The test fixture path is `crates/oulipoly-state/tests/fixtures/age360-missing-output-wire.json` (the machine-local planning copy is `age360-runner/missing-output-wire.json`).

Paired permanent-loss controls use a 64KiB log cap and force actual rollover before selected-inode removal/shortening, asserting that the live log has a different inode. Without eliminating that surviving alias, unlinking a pin alone would not establish irrecoverability. The existing logger bounds are unchanged outside these private fixtures.
