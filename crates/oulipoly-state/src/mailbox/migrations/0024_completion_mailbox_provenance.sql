ALTER TABLE mailbox ADD COLUMN completion_provenance TEXT NOT NULL DEFAULT 'unclassified'
    CHECK(completion_provenance IN ('unclassified','legacy','v2'));

-- A source or classified policy is positive v2 evidence. The subsequent
-- payload pass covers rows whose relational projections have been lost.
UPDATE mailbox AS message SET completion_provenance='v2'
WHERE message.kind='agent_bash_complete' AND message.delivered_at IS NULL
  AND (EXISTS (
      SELECT 1 FROM completion_event_listener AS listener
      JOIN completion_continuation_source AS source ON source.event_id=listener.event_id
      WHERE listener.mailbox_seq=message.seq
  ) OR EXISTS (
      SELECT 1 FROM completion_continuation_source AS source
      WHERE message.handle=source.event_id
         OR (message.owner_invocation_uuid IS NOT NULL
             AND message.handle=source.event_id || ':' || message.owner_invocation_uuid)
  ) OR EXISTS (
      SELECT 1 FROM completion_event_listener AS listener
      JOIN completion_continuation_notification AS notification
        ON notification.event_id=listener.event_id
       AND notification.listener_id=listener.listener_id
      WHERE listener.mailbox_seq=message.seq
        AND notification.policy_origin LIKE 'exact_admitted_binding:%'
  ));
