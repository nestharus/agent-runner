-- Exact fresh manual assertion. Existing ACKed grants without this row remain
-- unknown to terminal readback; no v29 listener can supply the basis.
CREATE TABLE fresh_recipient_ack_evidence (
 grant_id TEXT PRIMARY KEY REFERENCES fresh_recipient_grant(grant_id),
 delivery_request_id TEXT NOT NULL UNIQUE,
 delivery_token_sha256 TEXT NOT NULL,
 session_id TEXT NOT NULL, seq INTEGER NOT NULL,
 source_id TEXT NOT NULL, attempt_id TEXT NOT NULL,
 recipient_identity TEXT NOT NULL,
 payload_sha256 TEXT NOT NULL, payload_byte_len INTEGER NOT NULL,
 basis TEXT NOT NULL CHECK(basis IN ('manual_ack','delegated_manual_ack')),
 delegation_id TEXT,
 acknowledged_at TEXT NOT NULL,
 UNIQUE(session_id,seq),
 CHECK ((basis='manual_ack' AND delegation_id IS NULL) OR
        (basis='delegated_manual_ack' AND delegation_id IS NOT NULL))
);
CREATE TRIGGER fresh_recipient_ack_evidence_no_update BEFORE UPDATE ON fresh_recipient_ack_evidence
BEGIN SELECT RAISE(ABORT,'fresh ACK evidence immutable'); END;
CREATE TRIGGER fresh_recipient_ack_evidence_no_delete BEFORE DELETE ON fresh_recipient_ack_evidence
BEGIN SELECT RAISE(ABORT,'fresh ACK evidence retained'); END;
