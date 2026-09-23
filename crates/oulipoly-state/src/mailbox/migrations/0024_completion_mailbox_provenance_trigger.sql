CREATE TRIGGER mailbox_completion_provenance_immutable
BEFORE UPDATE OF completion_provenance ON mailbox
WHEN OLD.completion_provenance!='unclassified'
 AND NEW.completion_provenance IS NOT OLD.completion_provenance
BEGIN SELECT RAISE(ABORT,'mailbox completion provenance is immutable'); END;
