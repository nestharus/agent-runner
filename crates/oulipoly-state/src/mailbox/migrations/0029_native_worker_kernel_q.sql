-- An attach is preparation before the broker opens the worker's pre-exec gate.
-- Neither this row nor a lost K reply proves that the gate was released.
CREATE TABLE completion_native_worker_attach (
    attempt_id TEXT PRIMARY KEY REFERENCES completion_native_grant_binding(attempt_id),
    grant_id TEXT NOT NULL UNIQUE REFERENCES completion_native_grant_binding(grant_id),
    protocol TEXT NOT NULL CHECK(protocol='native-worker-attach-v1'),
    gate_held_before_release INTEGER NOT NULL CHECK(gate_held_before_release=1),
    kernel_root_id TEXT NOT NULL,
    work_id TEXT NOT NULL CHECK(length(work_id)>0 AND length(work_id)<=256 AND instr(work_id,char(0))=0),
    work_incarnation_id TEXT NOT NULL UNIQUE,
    broker_incarnation_id TEXT NOT NULL,
    worker_entrypoint TEXT NOT NULL CHECK(worker_entrypoint='__completion-root-worker-v1'),
    runner_image_sha256 TEXT NOT NULL CHECK(length(runner_image_sha256)=64),
    worker_identity TEXT NOT NULL UNIQUE,
    pid1_identity TEXT NOT NULL UNIQUE,
    work_pid_namespace_inode INTEGER NOT NULL CHECK(work_pid_namespace_inode>0),
    attach_receipt_sha256 TEXT NOT NULL UNIQUE CHECK(length(attach_receipt_sha256)=64),
    UNIQUE(kernel_root_id,work_id)
);
CREATE TRIGGER completion_native_worker_attach_exact
BEFORE INSERT ON completion_native_worker_attach
WHEN NOT EXISTS (
    SELECT 1 FROM completion_native_grant_binding b
    JOIN completion_continuation_attempt a ON a.attempt_id=b.attempt_id
    WHERE b.attempt_id=NEW.attempt_id AND b.grant_id=NEW.grant_id
      AND b.kernel_root_id=NEW.kernel_root_id
      AND NEW.worker_identity!=NEW.pid1_identity
      AND a.phase='accepted' AND a.revision=2 AND a.integrated=0
      AND a.custodian_identity IS NULL AND a.adopter_identity IS NULL
)
BEGIN SELECT RAISE(ABORT,'native attach requires exact accepted grant'); END;
CREATE TRIGGER completion_native_worker_attach_immutable
BEFORE UPDATE ON completion_native_worker_attach
BEGIN SELECT RAISE(ABORT,'native worker attach is immutable'); END;
CREATE TRIGGER completion_native_worker_attach_retain
BEFORE DELETE ON completion_native_worker_attach
BEGIN SELECT RAISE(ABORT,'native worker attach must be retained'); END;

-- Q is the broker's terminal PID1 and drained-work observation. It is distinct
-- from a worker result, recipient/source ACK, and activation claim integration.
CREATE TABLE completion_native_kernel_q (
    attempt_id TEXT PRIMARY KEY REFERENCES completion_native_worker_attach(attempt_id),
    grant_id TEXT NOT NULL UNIQUE,
    protocol TEXT NOT NULL CHECK(protocol='native-kernel-q-v1'),
    kernel_root_id TEXT NOT NULL,
    work_id TEXT NOT NULL,
    work_incarnation_id TEXT NOT NULL,
    observing_broker_incarnation_id TEXT NOT NULL,
    worker_identity TEXT NOT NULL,
    pid1_identity TEXT NOT NULL,
    work_pid_namespace_inode INTEGER NOT NULL CHECK(work_pid_namespace_inode>0),
    pid1_wait_status INTEGER NOT NULL,
    remaining_work_processes INTEGER NOT NULL CHECK(remaining_work_processes=0),
    pid1_reaped INTEGER NOT NULL CHECK(pid1_reaped=1),
    terminal_receipt_sha256 TEXT NOT NULL UNIQUE CHECK(length(terminal_receipt_sha256)=64)
);
CREATE TRIGGER completion_native_kernel_q_exact
BEFORE INSERT ON completion_native_kernel_q
WHEN NOT EXISTS (
    SELECT 1 FROM completion_native_worker_attach w
    JOIN completion_continuation_attempt a ON a.attempt_id=w.attempt_id
    WHERE w.attempt_id=NEW.attempt_id AND w.grant_id=NEW.grant_id
      AND w.kernel_root_id=NEW.kernel_root_id
      AND w.work_id=NEW.work_id AND w.work_incarnation_id=NEW.work_incarnation_id
      AND w.worker_identity=NEW.worker_identity AND w.pid1_identity=NEW.pid1_identity
      AND w.attach_receipt_sha256!=NEW.terminal_receipt_sha256
      AND w.work_pid_namespace_inode=NEW.work_pid_namespace_inode
      AND a.phase='accepted' AND a.revision=2 AND a.integrated=0
)
BEGIN SELECT RAISE(ABORT,'kernel Q requires exact attached work incarnation'); END;
CREATE TRIGGER completion_native_kernel_q_immutable
BEFORE UPDATE ON completion_native_kernel_q
BEGIN SELECT RAISE(ABORT,'native kernel Q is immutable'); END;
CREATE TRIGGER completion_native_kernel_q_retain
BEFORE DELETE ON completion_native_kernel_q
BEGIN SELECT RAISE(ABORT,'native kernel Q must be retained'); END;

-- Existing guardian ECHILD/unreleased routes cannot settle a bound native
-- grant or release its activation claim. A later integration API must combine
-- independently verified worker result and Q under the exact binding.
CREATE TRIGGER completion_native_grant_no_legacy_terminal
BEFORE UPDATE ON completion_continuation_attempt
WHEN EXISTS (SELECT 1 FROM completion_native_grant_binding b
             WHERE b.attempt_id=OLD.attempt_id)
 AND (NEW.phase IS NOT OLD.phase OR NEW.integrated IS NOT OLD.integrated
      OR NEW.drain_receipt IS NOT OLD.drain_receipt
      OR NEW.custodian_identity IS NOT OLD.custodian_identity
      OR NEW.adopter_identity IS NOT OLD.adopter_identity)
BEGIN SELECT RAISE(ABORT,'bound native attempt requires explicit integration'); END;
