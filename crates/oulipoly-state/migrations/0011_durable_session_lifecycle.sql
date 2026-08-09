-- Durable authority for session-scoped supervisors and provider turns.

CREATE TABLE session_supervisor_leases (
    session_id TEXT PRIMARY KEY CHECK (session_id <> ''),
    supervisor_generation INTEGER NOT NULL CHECK (supervisor_generation > 0),
    lease_token TEXT NOT NULL CHECK (lease_token <> ''),
    supervisor_pid INTEGER NOT NULL CHECK (supervisor_pid > 0),
    boot_id TEXT NOT NULL CHECK (boot_id <> ''),
    start_time_ticks INTEGER NOT NULL CHECK (start_time_ticks > 0),
    acquired_at INTEGER NOT NULL
);

CREATE TABLE provider_turn_generations (
    generation_id TEXT PRIMARY KEY CHECK (generation_id <> ''),
    spawn_invocation_id TEXT NOT NULL UNIQUE CHECK (spawn_invocation_id <> ''),
    session_id TEXT CHECK (session_id IS NULL OR session_id <> ''),
    lifecycle_state TEXT NOT NULL CHECK (
        lifecycle_state IN ('starting', 'running', 'draining', 'exited')
    ),
    child_pid INTEGER NOT NULL CHECK (child_pid > 0),
    child_boot_id TEXT NOT NULL CHECK (child_boot_id <> ''),
    child_start_time_ticks INTEGER NOT NULL CHECK (child_start_time_ticks > 0)
);

CREATE UNIQUE INDEX one_nonterminal_provider_turn_per_session
    ON provider_turn_generations (session_id)
    WHERE session_id IS NOT NULL AND lifecycle_state <> 'exited';

CREATE TABLE session_lifecycle_sequences (
    session_id TEXT PRIMARY KEY CHECK (session_id <> ''),
    last_sequence INTEGER NOT NULL CHECK (last_sequence >= 0)
);

CREATE TABLE session_lifecycle_events (
    event_id TEXT PRIMARY KEY CHECK (event_id <> ''),
    session_id TEXT NOT NULL CHECK (session_id <> ''),
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    event_type TEXT NOT NULL CHECK (event_type <> ''),
    cause_event_id TEXT,
    correlation_id TEXT NOT NULL CHECK (correlation_id <> ''),
    payload TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (session_id, sequence),
    FOREIGN KEY (cause_event_id) REFERENCES session_lifecycle_events (event_id)
);

CREATE INDEX session_lifecycle_events_by_session
    ON session_lifecycle_events (session_id, sequence);

CREATE TABLE session_lifecycle_event_dispositions (
    event_id TEXT NOT NULL,
    consumer_id TEXT NOT NULL CHECK (consumer_id <> ''),
    disposition TEXT NOT NULL CHECK (disposition IN ('applied', 'ignored')),
    disposed_at INTEGER NOT NULL,
    PRIMARY KEY (event_id, consumer_id),
    FOREIGN KEY (event_id) REFERENCES session_lifecycle_events (event_id)
);

CREATE TABLE session_external_ingress (
    session_id TEXT NOT NULL CHECK (session_id <> ''),
    ingress_sequence INTEGER NOT NULL CHECK (ingress_sequence > 0),
    ingress_id TEXT NOT NULL UNIQUE CHECK (ingress_id <> ''),
    payload TEXT NOT NULL,
    PRIMARY KEY (session_id, ingress_sequence)
);

CREATE TABLE session_external_ingress_cursors (
    session_id TEXT PRIMARY KEY CHECK (session_id <> ''),
    last_sequence INTEGER NOT NULL CHECK (last_sequence >= 0)
);

CREATE TABLE session_delivery_acknowledgements (
    delivery_id TEXT PRIMARY KEY CHECK (delivery_id <> ''),
    session_id TEXT NOT NULL CHECK (session_id <> ''),
    turn_generation_id TEXT NOT NULL CHECK (turn_generation_id <> ''),
    accepted_at INTEGER NOT NULL,
    submitted_at INTEGER,
    submitted_evidence TEXT,
    confirmed_at INTEGER,
    confirmed_evidence TEXT,
    CHECK ((submitted_at IS NULL) = (submitted_evidence IS NULL)),
    CHECK ((confirmed_at IS NULL) = (confirmed_evidence IS NULL)),
    CHECK (confirmed_at IS NULL OR submitted_at IS NOT NULL)
);

CREATE INDEX session_delivery_acknowledgements_by_session
    ON session_delivery_acknowledgements (session_id, accepted_at, delivery_id);
