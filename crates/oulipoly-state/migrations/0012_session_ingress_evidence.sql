-- Exact-fenced transport evidence below provider submission/confirmation.

CREATE TABLE session_delivery_evidence (
    evidence_id TEXT PRIMARY KEY CHECK (evidence_id <> ''),
    evidence_kind TEXT NOT NULL CHECK (
        evidence_kind IN ('pty_transport_ack', 'manual_acknowledgement')
    ),
    delivery_id TEXT NOT NULL,
    session_id TEXT NOT NULL CHECK (session_id <> ''),
    turn_generation_id TEXT NOT NULL CHECK (turn_generation_id <> ''),
    observed_at INTEGER NOT NULL,
    FOREIGN KEY (delivery_id) REFERENCES session_delivery_acknowledgements (delivery_id)
);

CREATE INDEX session_delivery_evidence_by_delivery
    ON session_delivery_evidence (delivery_id, observed_at, evidence_id);
