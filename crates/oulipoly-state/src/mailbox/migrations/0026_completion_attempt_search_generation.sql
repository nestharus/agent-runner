-- A cursor spans separate read transactions. Record every change to the
-- attempt search or its source links so a later page cannot silently finish
-- against a different keyspace or receipt projection. Existing history is not
-- scanned. The delete triggers also cover a future explicit reaping policy.
CREATE TABLE IF NOT EXISTS completion_continuation_attempt_search_generation (
    id INTEGER PRIMARY KEY CHECK(id=1),
    generation INTEGER NOT NULL CHECK(typeof(generation)='integer' AND generation>=0)
);
INSERT OR IGNORE INTO completion_continuation_attempt_search_generation(id,generation) VALUES(1,0);
CREATE TRIGGER IF NOT EXISTS completion_continuation_attempt_search_insert
AFTER INSERT ON completion_continuation_attempt
BEGIN
    UPDATE completion_continuation_attempt_search_generation
    SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER IF NOT EXISTS completion_continuation_attempt_search_update
AFTER UPDATE ON completion_continuation_attempt
BEGIN
    UPDATE completion_continuation_attempt_search_generation
    SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER IF NOT EXISTS completion_continuation_attempt_search_delete
AFTER DELETE ON completion_continuation_attempt
BEGIN
    UPDATE completion_continuation_attempt_search_generation
    SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER IF NOT EXISTS completion_continuation_attempt_source_search_insert
AFTER INSERT ON completion_continuation_attempt_source
BEGIN
    UPDATE completion_continuation_attempt_search_generation
    SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER IF NOT EXISTS completion_continuation_attempt_source_search_update
AFTER UPDATE ON completion_continuation_attempt_source
BEGIN
    UPDATE completion_continuation_attempt_search_generation
    SET generation=generation+1 WHERE id=1;
END;
CREATE TRIGGER IF NOT EXISTS completion_continuation_attempt_source_search_delete
AFTER DELETE ON completion_continuation_attempt_source
BEGIN
    UPDATE completion_continuation_attempt_search_generation
    SET generation=generation+1 WHERE id=1;
END;
