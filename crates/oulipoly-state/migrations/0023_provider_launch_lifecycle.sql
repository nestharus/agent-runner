-- Durable ownership only; runtime custody remains in the existing sidecar.
CREATE TABLE provider_logical_launches (
 logical_launch_id TEXT PRIMARY KEY NOT NULL,
 request_identity_sha256 TEXT NOT NULL,
 model_name TEXT NOT NULL,
 start_mode TEXT NOT NULL CHECK(start_mode IN ('create','resume')),
 expected_provider_session_id TEXT,
 candidate_plan_json TEXT NOT NULL,
 candidate_plan_sha256 TEXT NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('active','transfer_requested','successor_leased','cancelling','succeeded','failed','cancelled','recovery_blocked')),
 current_attempt_id TEXT NOT NULL,
 owner_epoch INTEGER NOT NULL CHECK(owner_epoch > 0),
 cancel_requested_at TEXT,
 terminal_code TEXT,
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL,
 finished_at TEXT,
 FOREIGN KEY(current_attempt_id, logical_launch_id, owner_epoch)
 REFERENCES provider_launch_attempts(attempt_id, logical_launch_id, owner_epoch) DEFERRABLE INITIALLY DEFERRED
);
CREATE TABLE provider_launch_attempts (
 attempt_id TEXT PRIMARY KEY NOT NULL,
 logical_launch_id TEXT NOT NULL REFERENCES provider_logical_launches(logical_launch_id),
 attempt_ordinal INTEGER NOT NULL CHECK(attempt_ordinal >= 0),
 owner_epoch INTEGER NOT NULL CHECK(owner_epoch = attempt_ordinal + 1),
 invocation_id INTEGER NOT NULL UNIQUE REFERENCES invocations(id),
 invocation_uuid TEXT NOT NULL UNIQUE,
 provider_index INTEGER NOT NULL CHECK(provider_index >= 0),
 account_name TEXT NOT NULL,
 endpoint_family TEXT,
 settings_id TEXT,
 provider_instance_id TEXT,
 endpoint_identity_sha256 TEXT,
 runtime_generation_uuid TEXT NOT NULL UNIQUE,
 return_channel_id TEXT NOT NULL UNIQUE,
 status TEXT NOT NULL CHECK(status IN ('leased','active','transfer_requested','effect_incapable','superseded','succeeded','failed','cancelled','recovery_blocked')),
 rotatable_kind TEXT CHECK(rotatable_kind IN ('host_timeout','provider_unavailable','provider_timeout')),
 failure_request_id TEXT,
 failure_code TEXT,
 provider_session_observed INTEGER NOT NULL DEFAULT 0 CHECK(provider_session_observed IN (0,1)),
 prompt_accepted INTEGER NOT NULL DEFAULT 0 CHECK(prompt_accepted IN (0,1)),
 assistant_response_observed INTEGER NOT NULL DEFAULT 0 CHECK(assistant_response_observed IN (0,1)),
 captured_child_count INTEGER NOT NULL DEFAULT 0 CHECK(captured_child_count >= 0),
 returned_artifact_count INTEGER NOT NULL DEFAULT 0 CHECK(returned_artifact_count >= 0),
 mailbox_submission_accepted INTEGER NOT NULL DEFAULT 0 CHECK(mailbox_submission_accepted IN (0,1)),
 actor_custody_state TEXT NOT NULL CHECK(actor_custody_state IN ('not_started','active','effect_incapable','uncertain')),
 actor_settlement_sha256 TEXT,
 runtime_settlement_sha256 TEXT,
 return_channel_state TEXT NOT NULL CHECK(return_channel_state IN ('not_created','open','empty_removed','artifacts_committed','quarantined','cleanup_failed')),
 return_channel_settlement_sha256 TEXT,
 effect_incapable_at TEXT,
 terminal_code TEXT,
 created_at TEXT NOT NULL,
 activated_at TEXT,
 finished_at TEXT,
 UNIQUE(logical_launch_id, attempt_ordinal),
 UNIQUE(logical_launch_id, owner_epoch),
 UNIQUE(logical_launch_id, account_name),
 UNIQUE(attempt_id, logical_launch_id, owner_epoch),
 CHECK((endpoint_family IS NULL AND settings_id IS NULL AND provider_instance_id IS NULL AND endpoint_identity_sha256 IS NULL)
    OR (endpoint_family IS NOT NULL AND settings_id IS NOT NULL AND provider_instance_id IS NOT NULL AND endpoint_identity_sha256 IS NOT NULL)),
 CHECK(effect_incapable_at IS NULL OR
   (provider_session_observed + prompt_accepted + assistant_response_observed + captured_child_count + returned_artifact_count + mailbox_submission_accepted = 0
    AND actor_custody_state = 'effect_incapable' AND actor_settlement_sha256 IS NOT NULL
    AND runtime_settlement_sha256 IS NOT NULL AND return_channel_settlement_sha256 IS NOT NULL
    AND return_channel_state IN ('not_created','empty_removed')))
);
CREATE UNIQUE INDEX provider_launch_failure_request ON provider_launch_attempts(failure_request_id) WHERE failure_request_id IS NOT NULL;
CREATE INDEX provider_launch_nonterminal ON provider_logical_launches(status);
-- Replay records contain request digests and non-secret results, never credentials.
CREATE TABLE provider_launch_transition_replays (
 logical_launch_id TEXT NOT NULL REFERENCES provider_logical_launches(logical_launch_id),
 operation_key TEXT NOT NULL,
 request_sha256 TEXT NOT NULL,
 result_json TEXT NOT NULL,
 PRIMARY KEY(logical_launch_id, operation_key)
);
CREATE TRIGGER provider_launch_attempt_invocation_join BEFORE INSERT ON provider_launch_attempts
WHEN NOT EXISTS (SELECT 1 FROM invocations i WHERE i.id = NEW.invocation_id
 AND i.invocation_uuid = NEW.invocation_uuid AND i.provider_name = NEW.account_name
 AND i.provider_index = NEW.provider_index AND i.status = 'running')
BEGIN SELECT RAISE(ABORT, 'provider launch invocation identity mismatch'); END;
CREATE TRIGGER provider_launch_attempt_immutable BEFORE UPDATE ON provider_launch_attempts
WHEN NEW.attempt_id != OLD.attempt_id OR NEW.logical_launch_id != OLD.logical_launch_id
 OR NEW.attempt_ordinal != OLD.attempt_ordinal OR NEW.owner_epoch != OLD.owner_epoch
 OR NEW.invocation_id != OLD.invocation_id OR NEW.invocation_uuid != OLD.invocation_uuid
 OR NEW.provider_index != OLD.provider_index OR NEW.account_name != OLD.account_name
 OR NEW.runtime_generation_uuid != OLD.runtime_generation_uuid OR NEW.return_channel_id != OLD.return_channel_id
 OR (OLD.endpoint_family IS NOT NULL AND (NEW.endpoint_family IS NOT OLD.endpoint_family
 OR NEW.settings_id IS NOT OLD.settings_id OR NEW.provider_instance_id IS NOT OLD.provider_instance_id
 OR NEW.endpoint_identity_sha256 IS NOT OLD.endpoint_identity_sha256))
 OR NEW.provider_session_observed < OLD.provider_session_observed OR NEW.prompt_accepted < OLD.prompt_accepted
 OR NEW.assistant_response_observed < OLD.assistant_response_observed OR NEW.captured_child_count < OLD.captured_child_count
 OR NEW.returned_artifact_count < OLD.returned_artifact_count OR NEW.mailbox_submission_accepted < OLD.mailbox_submission_accepted
BEGIN SELECT RAISE(ABORT, 'immutable provider launch attempt or decreasing promotion'); END;
CREATE TRIGGER provider_logical_launch_immutable BEFORE UPDATE ON provider_logical_launches
WHEN NEW.logical_launch_id != OLD.logical_launch_id OR NEW.request_identity_sha256 != OLD.request_identity_sha256
 OR NEW.model_name != OLD.model_name OR NEW.start_mode != OLD.start_mode
 OR NEW.expected_provider_session_id IS NOT OLD.expected_provider_session_id
 OR NEW.candidate_plan_json != OLD.candidate_plan_json OR NEW.candidate_plan_sha256 != OLD.candidate_plan_sha256
 OR NEW.owner_epoch < OLD.owner_epoch OR NEW.owner_epoch > OLD.owner_epoch + 1
 OR (OLD.cancel_requested_at IS NOT NULL AND NEW.cancel_requested_at IS NOT OLD.cancel_requested_at)
BEGIN SELECT RAISE(ABORT, 'immutable logical launch identity'); END;
