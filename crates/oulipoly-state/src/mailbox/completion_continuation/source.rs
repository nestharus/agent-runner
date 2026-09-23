use super::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryCursor {
    version: u8,
    domain_id: String,
    session_sha256: String,
    anchor_triggered_at: String,
    anchor_event_id: String,
    after_triggered_at: String,
    after_event_id: String,
}

const RECOVERY_ATTEMPT_PAGE_LIMIT: usize = 128;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryAttemptCursor {
    version: u8,
    domain_id: String,
    registration_sha256: String,
    anchor_attempt_id: String,
    phase: String,
    after_attempt_id: Option<String>,
}

fn recovery_registration_digest(registration_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"completion-recovery-attempt-source-v1\0");
    digest.update(registration_id.as_bytes());
    format!("{:x}", digest.finalize())
}

fn recovery_session_digest(session_id: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"completion-recovery-session-v1\0");
    match session_id {
        Some(id) => {
            digest.update(b"filtered\0");
            digest.update(id.as_bytes());
        }
        None => digest.update(b"all"),
    }
    format!("{:x}", digest.finalize())
}

impl CompletionAuthorityFence<'_> {
    pub(crate) fn require_continuation_binding(
        &self,
        event: &str,
        has_binding: bool,
    ) -> Result<(), String> {
        // Domain capability does not convert legacy admissions into v2 sources.
        // Preserve exact legacy registration/repair; an actually bound source
        // still cannot enter through that lane (State also checks the binding).
        if !has_binding && bound_event(&self.tx, event)? {
            return Err(format!(
                "unsupported_transition_required: v2 source requires exact source admission binding for {event}"
            ));
        }
        Ok(())
    }

    pub(crate) fn preflight_continuation_binding(
        &self,
        binding: &AdmittedSourceBinding,
        repair: bool,
    ) -> Result<(), String> {
        let source = binding.registration()?;
        if domain_on(&self.tx)?.as_deref() != Some(&source.domain_id) {
            return Err(
                "unsupported_transition_required: completion source domain mismatch".into(),
            );
        }
        if repair {
            return Ok(());
        }
        let owners: Option<(String,String)> = self.tx.query_row("SELECT guardian_identity,driver_identity FROM completion_continuation_owner WHERE domain_id=?1 AND phase='running'", [&source.domain_id], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e| e.to_string())?;
        let Some((guardian, driver)) = owners else {
            return Err("completion owner unavailable at admission".into());
        };
        for encoded in [&guardian, &driver] {
            let identity: SourceProcessIdentity =
                serde_json::from_str(encoded).map_err(|e| e.to_string())?;
            let expected = crate::pid_identity::ProcessIdentity {
                os_pid: identity.pid,
                os_boot_id: identity.boot_id,
                os_pid_starttime_ticks: identity.starttime_ticks,
            };
            if crate::pid_identity::read_live_process_identity(expected.os_pid)?.as_ref()
                != Some(&expected)
            {
                return Err("completion owner generation is not live at admission".into());
            }
        }
        Ok(())
    }

    pub(crate) fn materialize_continuation_binding(
        &self,
        binding: &AdmittedSourceBinding,
    ) -> Result<(), String> {
        self.preflight_continuation_binding(binding, true)?;
        let source = binding.registration()?;
        let existing: Option<Vec<u8>> = self
            .tx
            .query_row(
                "SELECT binding FROM completion_continuation_source WHERE registration_id=?1",
                [&source.registration_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let bytes = binding.encoded()?;
        if let Some(existing) = existing {
            return if AdmittedSourceBinding::decode(&existing)?.same_source(binding) {
                Ok(())
            } else {
                Err("immutable completion binding conflict".into())
            };
        }
        let supervisor_authority_id: String = self
            .tx
            .query_row(
                "SELECT supervisor_authority_id
                 FROM completion_continuation_owner
                 WHERE domain_id=?1 AND phase='running'",
                [&source.domain_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        self.tx.execute("INSERT INTO completion_continuation_source(registration_id,domain_id,source_id,event_id,registration_digest,binding,supervisor_authority_id,attempt_association_history) VALUES(?1,?2,?3,?4,?5,?6,?7,'known')", params![source.registration_id,source.domain_id,source.source_id,source.handle,binding.registration_digest(),bytes,supervisor_authority_id]).map_err(|e| e.to_string())?;
        Ok(())
    }
}

impl MailboxDb {
    /// Manual, read-only discovery is anchored in accepted source records, so
    /// response-only listeners are visible even without a mailbox row.
    pub fn completion_recovery_events(
        &self,
        session_id: Option<&str>,
        cursor: Option<&serde_json::Value>,
    ) -> Result<(Vec<serde_json::Value>, Option<serde_json::Value>), String> {
        if session_id == Some("") {
            return Err("session ID is required".into());
        }
        let domain_id = domain_on(&self.conn)?.ok_or("unsupported_transition_required")?;
        let cursor: Option<RecoveryCursor> = cursor
            .map(|value| {
                serde_json::from_value(value.clone())
                    .map_err(|_| "invalid recovery cursor".to_string())
            })
            .transpose()?;
        if let Some(cursor) = &cursor {
            if cursor.version != 1
                || cursor.domain_id != domain_id
                || cursor.session_sha256 != recovery_session_digest(session_id)
                || cursor.anchor_triggered_at.is_empty()
                || cursor.after_triggered_at.is_empty()
                || cursor.anchor_event_id.is_empty()
                || cursor.after_event_id.is_empty()
                || (&cursor.after_triggered_at, &cursor.after_event_id)
                    > (&cursor.anchor_triggered_at, &cursor.anchor_event_id)
            {
                return Err("invalid or stale recovery cursor".into());
            }
            // A repair or retirement of either boundary invalidates the page.
            // A fresh list is the bounded way to restart discovery.
            for (event_id, triggered_at) in [
                (&cursor.anchor_event_id, &cursor.anchor_triggered_at),
                (&cursor.after_event_id, &cursor.after_triggered_at),
            ] {
                let present: bool = self
                    .conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM completion_continuation_source s
                     JOIN completion_event e ON e.event_id=s.event_id
                     WHERE s.event_id=?1 AND e.triggered_at=?2
                       AND s.phase='accepted' AND e.state='triggered'
                       AND (?3 IS NULL OR EXISTS(SELECT 1 FROM completion_event_listener l
                           WHERE l.event_id=s.event_id AND l.session_id=?3)))",
                        params![event_id, triggered_at, session_id],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if !present {
                    return Err("stale recovery cursor; restart the list".into());
                }
            }
        }
        let mut statement = self
            .conn
            .prepare(
                "SELECT s.event_id,s.registration_id,s.domain_id,e.triggered_at
             FROM completion_continuation_source s
             JOIN completion_event e ON e.event_id=s.event_id
             WHERE s.phase='accepted' AND e.state='triggered'
               AND (?1 IS NULL OR EXISTS(SELECT 1 FROM completion_event_listener l
                   WHERE l.event_id=s.event_id AND l.session_id=?1))
               AND (?2 IS NULL OR (e.triggered_at,e.event_id) <= (?2,?3))
               AND (?4 IS NULL OR (e.triggered_at,e.event_id) < (?4,?5))
             ORDER BY e.triggered_at DESC,e.event_id DESC LIMIT 101",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map(
                params![
                    session_id,
                    cursor.as_ref().map(|c| c.anchor_triggered_at.as_str()),
                    cursor.as_ref().map(|c| c.anchor_event_id.as_str()),
                    cursor.as_ref().map(|c| c.after_triggered_at.as_str()),
                    cursor.as_ref().map(|c| c.after_event_id.as_str())
                ],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .map_err(|e| e.to_string())?;
        let mut rows: Vec<_> = rows
            .map(|r| r.map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        let more = rows.len() > 100;
        rows.truncate(100);
        let next_cursor = if more {
            let first = rows
                .first()
                .ok_or("empty recovery page with continuation")?;
            let last = rows.last().ok_or("empty recovery page with continuation")?;
            let cursor = RecoveryCursor {
                version: 1,
                domain_id,
                session_sha256: recovery_session_digest(session_id),
                anchor_triggered_at: cursor
                    .as_ref()
                    .map_or(&first.3, |c| &c.anchor_triggered_at)
                    .clone(),
                anchor_event_id: cursor
                    .as_ref()
                    .map_or(&first.0, |c| &c.anchor_event_id)
                    .clone(),
                after_triggered_at: last.3.clone(),
                after_event_id: last.0.clone(),
            };
            Some(serde_json::to_value(cursor).map_err(|e| e.to_string())?)
        } else {
            None
        };
        Ok((
            rows.into_iter()
                .map(|(event_id, registration_id, domain_id, triggered_at)| {
                    serde_json::json!({"event_id":event_id,"registration_id":registration_id,
                "domain_id":domain_id,"triggered_at":triggered_at})
                })
                .collect(),
            next_cursor,
        ))
    }

    pub fn completion_recovery_record(
        &self,
        event_id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        if domain_on(&self.conn)?.is_none() {
            return Err("unsupported_transition_required".into());
        }
        self.conn.query_row(
            "SELECT s.registration_id,s.domain_id,s.source_id,s.registration_digest,
                    s.snapshot_sha256,s.outcome_sha256,s.payload_sha256,s.payload_byte_len,
                    e.payload_file_path,e.state,e.triggered_at
             FROM completion_continuation_source s
             JOIN completion_event e ON e.event_id=s.event_id
             WHERE s.event_id=?1 AND s.phase='accepted' AND e.state='triggered'",
            [event_id], |r| Ok(serde_json::json!({
                "event_id":event_id,
                "registration_id":r.get::<_,String>(0)?, "domain_id":r.get::<_,String>(1)?,
                "source_id":r.get::<_,String>(2)?, "registration_digest":r.get::<_,String>(3)?,
                "snapshot_sha256":r.get::<_,String>(4)?, "outcome_sha256":r.get::<_,String>(5)?,
                "payload_sha256":r.get::<_,String>(6)?, "payload_byte_len":r.get::<_,i64>(7)?,
                "payload_file_path":r.get::<_,String>(8)?, "event_state":r.get::<_,String>(9)?,
                "triggered_at":r.get::<_,Option<String>>(10)?,
            }))
        ).optional().map_err(|e| e.to_string())
    }

    /// An indexed link page followed by fixed-size pages of the retained
    /// attempt keyspace. Historical scalar associations have no index because
    /// building one during sidecar open would scan the entire live table.
    /// An empty page with a cursor is therefore not evidence of no attempts.
    pub fn completion_recovery_attempts(
        &self,
        registration_id: &str,
        cursor: Option<&serde_json::Value>,
    ) -> Result<(Vec<serde_json::Value>, Option<serde_json::Value>), String> {
        let domain_id: String = self
            .conn
            .query_row(
                "SELECT domain_id FROM completion_continuation_source
             WHERE registration_id=?1 AND phase='accepted'",
                [registration_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let cursor: RecoveryAttemptCursor = match cursor {
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|_| "invalid recovery attempt cursor".to_string())?,
            None => {
                let anchor_attempt_id: Option<String> = self
                    .conn
                    .query_row(
                        "SELECT MAX(attempt_id) FROM completion_continuation_attempt",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                RecoveryAttemptCursor {
                    version: 1,
                    domain_id: domain_id.clone(),
                    registration_sha256: recovery_registration_digest(registration_id),
                    anchor_attempt_id: anchor_attempt_id.unwrap_or_default(),
                    phase: "linked".into(),
                    after_attempt_id: None,
                }
            }
        };
        if cursor.version != 1
            || cursor.domain_id != domain_id
            || cursor.registration_sha256 != recovery_registration_digest(registration_id)
            || !matches!(cursor.phase.as_str(), "linked" | "history")
            || cursor
                .after_attempt_id
                .as_deref()
                .is_some_and(|after| after.is_empty() || after > cursor.anchor_attempt_id.as_str())
        {
            return Err("invalid or stale recovery attempt cursor".into());
        }
        if !cursor.anchor_attempt_id.is_empty() {
            let anchor_exists: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM completion_continuation_attempt WHERE attempt_id=?1)",
                [&cursor.anchor_attempt_id], |r| r.get(0),
            ).map_err(|e| e.to_string())?;
            if !anchor_exists {
                return Err("stale recovery attempt cursor".into());
            }
        }
        if let Some(after) = &cursor.after_attempt_id {
            let boundary_exists: bool = if cursor.phase == "linked" {
                self.conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM completion_continuation_attempt_source
                     WHERE registration_id=?1 AND attempt_id=?2)",
                        params![registration_id, after],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?
            } else {
                self.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM completion_continuation_attempt WHERE attempt_id=?1)",
                    [after], |r| r.get(0),
                ).map_err(|e| e.to_string())?
            };
            if !boundary_exists {
                return Err("stale recovery attempt cursor".into());
            }
        }
        let mut attempts = Vec::new();
        let next = if cursor.phase == "linked" {
            let mut statement = self
                .conn
                .prepare(
                    "SELECT a.attempt_id,a.operation,a.phase,a.integrated,a.drain_receipt,
                        a.association_completeness
                 FROM completion_continuation_attempt_source AS link
                      INDEXED BY completion_continuation_attempt_source_registration
                 JOIN completion_continuation_attempt AS a ON a.attempt_id=link.attempt_id
                 WHERE link.registration_id=?1 AND link.attempt_id<=?2
                   AND link.attempt_id>?3
                 ORDER BY link.attempt_id LIMIT ?4",
                )
                .map_err(|e| e.to_string())?;
            let mut rows = statement
                .query(params![
                    registration_id,
                    cursor.anchor_attempt_id,
                    cursor.after_attempt_id.as_deref().unwrap_or(""),
                    (RECOVERY_ATTEMPT_PAGE_LIMIT + 1) as i64
                ])
                .map_err(|e| e.to_string())?;
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                attempts.push(serde_json::json!({
                    "attempt_id":row.get::<_,String>(0).map_err(|e|e.to_string())?,
                    "operation":row.get::<_,String>(1).map_err(|e|e.to_string())?,
                    "phase":row.get::<_,String>(2).map_err(|e|e.to_string())?,
                    "integrated":row.get::<_,bool>(3).map_err(|e|e.to_string())?,
                    "drain_receipt":row.get::<_,Option<String>>(4).map_err(|e|e.to_string())?,
                    "association_completeness":row.get::<_,String>(5).map_err(|e|e.to_string())?,
                }));
            }
            let more_links = attempts.len() > RECOVERY_ATTEMPT_PAGE_LIMIT;
            attempts.truncate(RECOVERY_ATTEMPT_PAGE_LIMIT);
            let mut next = cursor.clone();
            if more_links {
                next.after_attempt_id = attempts
                    .last()
                    .and_then(|a| a["attempt_id"].as_str())
                    .map(str::to_owned);
            } else {
                next.phase = "history".into();
                next.after_attempt_id = None;
            }
            Some(next)
        } else {
            // This walks at most 129 primary-key entries, including entries
            // with no scalar match. Filtering before LIMIT would reintroduce
            // an unbounded one-call search for a late historical receipt.
            let mut statement = self
                .conn
                .prepare(
                    "SELECT attempt_id,operation,phase,integrated,drain_receipt,
                        association_completeness,source_registration_id
                 FROM completion_continuation_attempt
                 WHERE attempt_id<=?1 AND attempt_id>?2
                 ORDER BY attempt_id LIMIT ?3",
                )
                .map_err(|e| e.to_string())?;
            let mut rows = statement
                .query(params![
                    cursor.anchor_attempt_id,
                    cursor.after_attempt_id.as_deref().unwrap_or(""),
                    (RECOVERY_ATTEMPT_PAGE_LIMIT + 1) as i64
                ])
                .map_err(|e| e.to_string())?;
            let mut scanned = 0;
            let mut last_scanned = None;
            let mut more_history = false;
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                if scanned == RECOVERY_ATTEMPT_PAGE_LIMIT {
                    more_history = true;
                    break;
                }
                scanned += 1;
                let attempt_id: String = row.get(0).map_err(|e| e.to_string())?;
                last_scanned = Some(attempt_id.clone());
                let operation: String = row.get(1).map_err(|e| e.to_string())?;
                let completeness: String = row.get(5).map_err(|e| e.to_string())?;
                let scalar_source: Option<String> = row.get(6).map_err(|e| e.to_string())?;
                if scalar_source.as_deref() != Some(registration_id)
                    || (operation == "activation" && completeness != "unknown")
                {
                    continue;
                }
                let linked: bool = self
                    .conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM completion_continuation_attempt_source
                     WHERE attempt_id=?1 AND registration_id=?2)",
                        params![attempt_id, registration_id],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
                if linked {
                    continue;
                }
                attempts.push(serde_json::json!({
                    "attempt_id":attempt_id, "operation":operation,
                    "phase":row.get::<_,String>(2).map_err(|e|e.to_string())?,
                    "integrated":row.get::<_,bool>(3).map_err(|e|e.to_string())?,
                    "drain_receipt":row.get::<_,Option<String>>(4).map_err(|e|e.to_string())?,
                    "association_completeness":completeness,
                }));
            }
            if more_history {
                let mut next = cursor.clone();
                next.after_attempt_id = last_scanned;
                Some(next)
            } else {
                None
            }
        };
        Ok((
            attempts,
            next.map(|next| serde_json::to_value(next).expect("cursor serializes")),
        ))
    }

    pub(crate) fn has_original_completion_source(
        &self,
        binding: &AdmittedSourceBinding,
    ) -> Result<bool, String> {
        let source = binding.registration()?;
        let retained: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT binding FROM completion_continuation_source
                 WHERE registration_id=?1 AND domain_id=?2",
                params![source.registration_id, source.domain_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        retained
            .map(|bytes| {
                AdmittedSourceBinding::decode(&bytes).map(|original| original.same_source(binding))
            })
            .transpose()
            .map(|matched| matched.unwrap_or(false))
    }

    /// A coherent live observation of exact projection and reconciliation, not
    /// acceptance or mutation authority. A missed/changed row still needs repair.
    pub(crate) fn continuation_projection_matches(
        &self,
        binding: &AdmittedSourceBinding,
        expected_head: &CompletionContinuityHead,
    ) -> Result<bool, String> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        if completion_continuity_head_on(&tx)?.as_ref() != Some(expected_head)
            || sidecar_generation_on(&tx)? != expected_head.sidecar_generation
        {
            return Ok(false);
        }
        projection_matches_on(&tx, binding)
    }

    pub fn completion_continuation_acceptance(
        &self,
        registration_id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        if domain_on(&self.conn)?.is_none() {
            return Err("unsupported_transition_required".into());
        }
        self.conn.query_row("SELECT phase,snapshot_sha256,outcome_sha256,payload_sha256,payload_byte_len FROM completion_continuation_source WHERE registration_id=?1", [registration_id], |r| Ok(serde_json::json!({
            "phase":r.get::<_,String>(0)?, "snapshot_sha256":r.get::<_,Option<String>>(1)?,
            "outcome_sha256":r.get::<_,Option<String>>(2)?, "payload_sha256":r.get::<_,Option<String>>(3)?,
            "payload_byte_len":r.get::<_,Option<i64>>(4)?,
        }))).optional().map_err(|e| e.to_string())
    }
}

pub(in crate::mailbox) fn accept_on(
    tx: &Transaction<'_>,
    binding: &AdmittedSourceBinding,
    evidence: &crate::completion_continuation::VerifiedCompletion,
    payload: &PublishedMailboxPayload,
) -> Result<(), String> {
    let source = binding.registration()?;
    type AcceptedSourceRow = (
        Vec<u8>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
    );
    let row: AcceptedSourceRow = tx.query_row(
        "SELECT binding,snapshot_sha256,outcome_sha256,payload_sha256,payload_byte_len FROM completion_continuation_source WHERE registration_id=?1",
        [&source.registration_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))
    ).map_err(|e| e.to_string())?;
    let payload_len = i64::try_from(payload.byte_len).map_err(|e| e.to_string())?;
    if !AdmittedSourceBinding::decode(&row.0)?.same_source(binding) {
        return Err("completion acceptance binding conflict".into());
    }
    if let Some(snapshot) = row.1 {
        if snapshot != evidence.snapshot_sha256
            || row.2.as_deref() != Some(&evidence.outcome_sha256)
            || row.3.as_deref() != Some(&payload.sha256)
            || row.4 != Some(payload_len)
        {
            return Err("completion immutable acceptance conflict".into());
        }
        return Ok(());
    }
    tx.execute("UPDATE completion_continuation_source SET phase='accepted',snapshot_sha256=?2,outcome_sha256=?3,payload_sha256=?4,payload_byte_len=?5 WHERE registration_id=?1", params![source.registration_id,evidence.snapshot_sha256,evidence.outcome_sha256,payload.sha256,payload_len]).map_err(|e| e.to_string())?;
    Ok(())
}

pub(in crate::mailbox) fn reject_unbound_v2_trigger(
    tx: &Transaction<'_>,
    event_id: &str,
) -> Result<(), String> {
    if domain_on(tx)?.is_none() {
        return Ok(());
    }
    let bound: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM completion_continuation_source WHERE event_id=?1)",
            [event_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if bound {
        return Err("v2 completion requires exact source outcome and snapshot".into());
    }
    Ok(())
}

pub(in crate::mailbox) fn bound_event(conn: &Connection, event: &str) -> Result<bool, String> {
    if domain_on(conn)?.is_none() {
        return Ok(false);
    }
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM completion_continuation_source WHERE event_id=?1)",
        [event],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}
pub(in crate::mailbox) fn retained_payload(
    conn: &Connection,
    digest: &str,
) -> Result<bool, String> {
    if domain_on(conn)?.is_none() {
        return Ok(false);
    }
    conn.query_row("SELECT EXISTS(SELECT 1 FROM completion_continuation_source WHERE payload_sha256=?1 AND phase='accepted')",[digest],|r|r.get(0)).map_err(|e|e.to_string())
}

fn projection_matches_on(
    conn: &Connection,
    binding: &AdmittedSourceBinding,
) -> Result<bool, String> {
    let source = binding.registration()?;
    if domain_on(conn)?.as_deref() != Some(&source.domain_id) {
        return Ok(false);
    }
    let retained: Option<Vec<u8>> = conn
        .query_row(
            "SELECT binding FROM completion_continuation_source WHERE registration_id=?1",
            [&source.registration_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(retained) = retained else {
        return Ok(false);
    };
    if !AdmittedSourceBinding::decode(&retained)?.same_source(binding) {
        return Ok(false);
    }
    let Some(event) = completion_event_by_id_on(conn, &source.handle)? else {
        return Ok(false);
    };
    let identity = binding.admission_listener()?;
    let paths = source.paths();
    validate_completion_event_registration_replay(
        &event,
        &CompletionEventRegistrationInput {
            event_id: &source.handle,
            delivery_mode: &source.delivery_mode,
            owner_session_id: Some(&identity.session_id),
            owner_invocation_uuid: Some(&identity.owner_invocation_uuid),
            state_dir: &source.handle_dir,
            meta_path: &paths[0],
            log_path: &paths[1],
            rc_path: &paths[2],
        },
    )?;
    let Some(listener) = completion_event_listener_on(conn, &source.handle, &identity.listener_id)?
    else {
        return Ok(false);
    };
    validate_completion_event_listener_replay(
        &listener,
        &identity.session_id,
        &identity.owner_invocation_uuid,
    )?;
    let policy: Option<String> = conn.query_row(
        "SELECT policy FROM completion_continuation_notification WHERE event_id=?1 AND listener_id=?2",
        params![source.handle, identity.listener_id], |r| r.get(0),
    ).optional().map_err(|e| e.to_string())?;
    match policy.as_deref() {
        Some("response_only") => Ok(!listener.active
            || listener.acknowledged_at.is_some()
            || listener.mailbox_seq.is_some()),
        Some("notify") if event.state == "triggered" => Ok(listener.acknowledged_at.is_some()
            || (listener.active && listener.mailbox_seq.is_some())),
        Some("notify") => Ok(true),
        _ => Ok(false),
    }
}
