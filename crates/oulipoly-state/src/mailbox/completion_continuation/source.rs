use super::*;

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
        self.tx.execute("INSERT INTO completion_continuation_source(registration_id,domain_id,source_id,event_id,registration_digest,binding) VALUES(?1,?2,?3,?4,?5,?6)", params![source.registration_id,source.domain_id,source.source_id,source.handle,binding.registration_digest(),bytes]).map_err(|e| e.to_string())?;
        Ok(())
    }
}

impl MailboxDb {
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
