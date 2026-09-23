CREATE TRIGGER mailbox_completion_provenance_insert_valid
BEFORE INSERT ON mailbox
WHEN NEW.completion_provenance IS NULL
 OR NEW.completion_provenance NOT IN ('unclassified','legacy','v2')
BEGIN SELECT RAISE(ABORT,'invalid mailbox completion provenance'); END;

CREATE TRIGGER mailbox_completion_provenance_update_valid
BEFORE UPDATE ON mailbox
WHEN NEW.completion_provenance IS NULL
 OR NEW.completion_provenance NOT IN ('unclassified','legacy','v2')
BEGIN SELECT RAISE(ABORT,'invalid mailbox completion provenance'); END;

CREATE TRIGGER mailbox_completion_provenance_immutable
BEFORE UPDATE OF completion_provenance ON mailbox
WHEN OLD.completion_provenance!='unclassified'
 AND NEW.completion_provenance IS NOT OLD.completion_provenance
BEGIN SELECT RAISE(ABORT,'mailbox completion provenance is immutable'); END;
