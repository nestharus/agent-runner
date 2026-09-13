use super::*;

impl MailboxDb {
    pub fn reserve_continuation_attempt(
        &mut self,
        request: &ContinuationAttempt,
    ) -> Result<(), String> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        reserve_on(&tx, request)?;
        tx.commit().map_err(|e| e.to_string())
    }

    /// The same live driver can revoke an unaccepted reservation after request
    /// I/O failure. Accepted gates and predecessor obligations are never freed.
    pub fn revoke_unaccepted_continuation_attempt(
        &mut self,
        attempt: &ContinuationAttempt,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
            .ok_or("driver process disappeared")?;
        let identity = SourceProcessIdentity {
            pid: live.os_pid,
            boot_id: live.os_boot_id,
            starttime_ticks: live.os_pid_starttime_ticks,
        };
        let encoded = serde_json::to_string(&identity).map_err(|e| e.to_string())?;
        let changed = self.conn.execute(
            "UPDATE completion_continuation_attempt SET phase='never_started',revision=revision+1,integrated=1,drain_receipt=?3 WHERE attempt_id=?1 AND owner_generation=?2 AND phase='reserved' AND revision=1 AND custodian_identity IS NULL AND EXISTS(SELECT 1 FROM completion_continuation_owner WHERE generation=?2 AND phase='running' AND driver_identity=?4)",
            params![attempt.attempt_id, attempt.owner_generation,
                serde_json::json!({"attempt_id":attempt.attempt_id,"driver":identity,"gate":"unaccepted_reservation_revoked"}).to_string(), encoded],
        ).map_err(|e| e.to_string())?;
        if changed != 1 {
            return Err("reservation no longer unaccepted under this driver".into());
        }
        Ok(())
    }

    pub fn advance_continuation_attempt(
        &mut self,
        attempt: &ContinuationAttempt,
        revision: i64,
        from: &str,
        to: &str,
        custodian: &SourceProcessIdentity,
    ) -> Result<i64, String> {
        require_exact_attempt(&self.conn, attempt)?;
        if !matches!((from, to), ("accepted", "starting")) {
            return Err("invalid continuation attempt transition".into());
        }
        let changed = self.conn.execute("UPDATE completion_continuation_attempt SET phase=?4,revision=revision+1,custodian_identity=?5 WHERE attempt_id=?1 AND revision=?2 AND phase=?3 AND custodian_identity=?5 AND owner_generation=?6 AND EXISTS(SELECT 1 FROM completion_continuation_owner WHERE generation=?6 AND phase='running')", params![attempt.attempt_id,revision,from,to,serde_json::to_string(custodian).map_err(|e| e.to_string())?,attempt.owner_generation]).map_err(|e| e.to_string())?;
        if changed != 1 {
            return Err("stale continuation attempt owner/revision".into());
        }
        Ok(revision + 1)
    }

    /// Exact original custodian integration after its actual ECHILD observation.
    /// This operation intentionally has no listener ACK argument.
    pub fn discharge_continuation_attempt(
        &mut self,
        attempt: &ContinuationAttempt,
        custodian: &SourceProcessIdentity,
        receipt: &str,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        if receipt.is_empty() {
            return Err("missing physical drain receipt".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let changed = tx.execute("UPDATE completion_continuation_attempt SET phase='drained',revision=revision+1,integrated=1,drain_receipt=?3 WHERE attempt_id=?1 AND custodian_identity=?2 AND phase IN ('starting','running','unknown_custody')", params![attempt.attempt_id,serde_json::to_string(custodian).map_err(|e| e.to_string())?,receipt]).map_err(|e| e.to_string())?;
        if changed != 1 {
            let retained:Option<String>=tx.query_row("SELECT drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1 AND custodian_identity=?2 AND phase='drained'",params![attempt.attempt_id,serde_json::to_string(custodian).map_err(|e|e.to_string())?],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
            if retained.as_deref() != Some(receipt) {
                return Err("completion custodian mismatch or not started".into());
            }
        }
        if attempt.operation == "activation" {
            tx.execute(
                "DELETE FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2",
                params![attempt.session_id, attempt.claim_token],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn pending_continuation_attempt_ids(
        &self,
        registration_id: &str,
    ) -> Result<Vec<String>, String> {
        let mut statement = self.conn.prepare("SELECT attempt_id FROM completion_continuation_attempt WHERE source_registration_id=?1 AND phase NOT IN ('drained','never_started') ORDER BY attempt_id").map_err(|e| e.to_string())?;
        statement
            .query_map([registration_id], |r| r.get(0))
            .map_err(|e| e.to_string())?
            .map(|r| r.map_err(|e| e.to_string()))
            .collect()
    }
}

fn reserve_on(tx: &Transaction<'_>, request: &ContinuationAttempt) -> Result<(), String> {
    let domain: String = tx.query_row("SELECT domain_id FROM completion_continuation_owner WHERE generation=?1 AND phase='running'", [&request.owner_generation], |r| r.get(0)).map_err(|e| e.to_string())?;
    if !crate::completion_continuation::is_sha256(&request.request_sha256) {
        return Err("invalid continuation request digest".into());
    }
    if request.operation == "source_recovery" {
        let registration = request
            .source_registration_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or("source recovery requires registration")?;
        let blocked: bool = tx.query_row(
            "SELECT (SELECT COUNT(*) FROM completion_continuation_attempt WHERE domain_id=?1 AND operation='source_recovery' AND phase NOT IN ('drained','never_started')) >= 4 OR EXISTS(SELECT 1 FROM completion_continuation_attempt WHERE domain_id=?1 AND operation='source_recovery' AND source_registration_id=?2 AND phase NOT IN ('drained','never_started'))",
            params![domain, registration], |r| r.get(0)).map_err(|e| e.to_string())?;
        if blocked {
            return Err("source recovery retained custody bound".into());
        }
    }
    if request.operation == "activation" {
        let claim: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2)", params![request.session_id,request.claim_token], |r| r.get(0)).map_err(|e| e.to_string())?;
        if !claim {
            return Err("activation requires exact current wake claim".into());
        }
    }
    tx.execute("INSERT INTO completion_continuation_attempt(attempt_id,domain_id,owner_generation,operation,request_sha256,source_registration_id,source_listener_revision,session_id,claim_token,phase,result_path) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'reserved',?10)", params![request.attempt_id,domain,request.owner_generation,request.operation,request.request_sha256,request.source_registration_id,request.source_listener_revision,request.session_id,request.claim_token,request.result_path]).map_err(|e| e.to_string())?;
    Ok(())
}

impl MailboxDb {
    pub fn cancel_unreleased_continuation_gate(
        &mut self,
        attempt: &ContinuationAttempt,
        custodian: &SourceProcessIdentity,
        receipt: &str,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        if receipt.is_empty() {
            return Err("missing cancelled gate proof".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let identity = serde_json::to_string(custodian).map_err(|e| e.to_string())?;
        let changed=tx.execute("UPDATE completion_continuation_attempt SET phase='never_started',revision=revision+1,custodian_identity=?2,integrated=1,drain_receipt=?3 WHERE attempt_id=?1 AND phase IN ('reserved','accepted','starting','unknown_custody') AND (custodian_identity IS NULL OR custodian_identity=?2)",params![attempt.attempt_id,identity,receipt]).map_err(|e|e.to_string())?;
        if changed != 1 {
            let retained:Option<String>=tx.query_row("SELECT drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1 AND custodian_identity=?2 AND phase='never_started'",params![attempt.attempt_id,identity],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
            if retained.as_deref() != Some(receipt) {
                return Err("cancelled gate does not match current attempt custody".into());
            }
        }
        if attempt.operation == "activation" {
            tx.execute(
                "DELETE FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2",
                params![attempt.session_id, attempt.claim_token],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
}

pub(in crate::mailbox) fn reserve_activation_on(
    tx: &Transaction<'_>,
    input: WakeClaimRequest<'_>,
) -> Result<(), String> {
    let Some(domain) = domain_on(tx)? else {
        return Ok(());
    };
    let (generation,driver):(String,String)=tx.query_row("SELECT generation,driver_identity FROM completion_continuation_owner WHERE domain_id=?1 AND phase='running'",[&domain],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|e|e.to_string())?;
    let driver: SourceProcessIdentity = serde_json::from_str(&driver).map_err(|e| e.to_string())?;
    let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
        .ok_or("driver identity unavailable")?;
    if driver.pid != live.os_pid
        || driver.boot_id != live.os_boot_id
        || driver.starttime_ticks != live.os_pid_starttime_ticks
    {
        return Err("completion activation must be admitted by independent driver".into());
    }
    let metadata = session_metadata_row(tx, input.session_id)?;
    let source:Option<String>=tx.query_row("SELECT s.registration_id FROM completion_continuation_source s JOIN completion_event_listener l ON l.event_id=s.event_id WHERE l.session_id=?1 AND l.acknowledged_at IS NULL ORDER BY s.registration_id LIMIT 1",[input.session_id],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    let request = ContinuationAttempt {
        attempt_id: uuid::Uuid::new_v4().to_string(),
        owner_generation: generation,
        operation: "activation".into(),
        request_sha256: super::activation_request_sha256(
            input.session_id,
            metadata.as_ref(),
            input.claim_token,
            input.auto_wake_count,
        ),
        source_registration_id: source.clone(),
        source_listener_revision: source.as_ref().map(|id| tx.query_row("SELECT COUNT(*) FROM completion_event_listener l JOIN completion_continuation_source s ON s.event_id=l.event_id WHERE s.registration_id=?1",[id],|r|r.get::<_,i64>(0)).map_err(|e|e.to_string())).transpose()?,
        session_id: Some(input.session_id.into()),
        claim_token: Some(input.claim_token.into()),
        result_path: crate::paths::data_dir()?
            .join("completion-continuation")
            .join(&domain)
            .join("attempts")
            .join(input.claim_token)
            .join("result.json")
            .to_string_lossy()
            .into_owned(),
    };
    reserve_on(tx, &request)
}

impl MailboxDb {
    pub fn accept_continuation_attempt(
        &mut self,
        attempt: &ContinuationAttempt,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        let changed=self.conn.execute("UPDATE completion_continuation_attempt SET phase='accepted',revision=revision+1 WHERE attempt_id=?1 AND owner_generation=?2 AND phase='reserved' AND revision=1 AND EXISTS(SELECT 1 FROM completion_continuation_owner WHERE generation=?2 AND phase='running')",params![attempt.attempt_id,attempt.owner_generation]).map_err(|e|e.to_string())?;
        if changed != 1 {
            return Err("continuation acceptance lost current reservation".into());
        }
        Ok(())
    }
    pub fn attach_continuation_custodian(
        &mut self,
        attempt: &ContinuationAttempt,
        custodian: &SourceProcessIdentity,
    ) -> Result<(), String> {
        self.attach_continuation_custodian_with_adopter(attempt, custodian, None)
    }
    pub fn attach_continuation_custodian_with_adopter(
        &mut self,
        attempt: &ContinuationAttempt,
        custodian: &SourceProcessIdentity,
        adopter: Option<&SourceProcessIdentity>,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        let changed=self.conn.execute("UPDATE completion_continuation_attempt SET revision=revision+1,custodian_identity=?3,adopter_identity=?4 WHERE attempt_id=?1 AND owner_generation=?2 AND phase='accepted' AND revision=2 AND custodian_identity IS NULL AND EXISTS(SELECT 1 FROM completion_continuation_owner WHERE generation=?2 AND phase='running')",params![attempt.attempt_id,attempt.owner_generation,serde_json::to_string(custodian).map_err(|e|e.to_string())?,adopter.map(serde_json::to_string).transpose().map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        if changed != 1 {
            return Err("continuation custodian publication lost current reservation".into());
        }
        Ok(())
    }
    /// Retained birth testimony from the SAME original driver, not a successor's
    /// inferred identity. This records custody only and moves an unfinished
    /// starting gate to unknown custody, never restoring execution authority.
    /// The immutable actor attachment and revision schema remain enforced.
    pub fn attach_original_continuation_custody(
        &mut self,
        attempt: &ContinuationAttempt,
        driver: &SourceProcessIdentity,
        custodian: &SourceProcessIdentity,
        adopter: &SourceProcessIdentity,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
            .ok_or("original driver absent")?;
        if driver.pid != live.os_pid
            || driver.boot_id != live.os_boot_id
            || driver.starttime_ticks != live.os_pid_starttime_ticks
        {
            return Err("original birth driver identity conflict".into());
        }
        let driver = serde_json::to_string(driver).map_err(|e| e.to_string())?;
        let custodian = serde_json::to_string(custodian).map_err(|e| e.to_string())?;
        let adopter = serde_json::to_string(adopter).map_err(|e| e.to_string())?;
        let recorded_driver: String = self
            .conn
            .query_row(
                "SELECT driver_identity FROM completion_continuation_owner WHERE generation=?1",
                [&attempt.owner_generation],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if recorded_driver != driver {
            return Err("original birth driver generation conflict".into());
        }
        let (old_custodian, old_adopter, phase): (Option<String>, Option<String>, String) = self.conn.query_row(
            "SELECT custodian_identity,adopter_identity,phase FROM completion_continuation_attempt WHERE attempt_id=?1", [&attempt.attempt_id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|e|e.to_string())?;
        if old_custodian.as_ref().is_some_and(|v| v != &custodian)
            || old_adopter.as_ref().is_some_and(|v| v != &adopter)
            || !matches!(
                phase.as_str(),
                "accepted" | "starting" | "unknown_custody" | "drained" | "never_started"
            )
        {
            return Err("original birth attachment fence conflict".into());
        }
        if old_custodian.is_some()
            && old_adopter.is_some()
            && !matches!(phase.as_str(), "accepted" | "starting")
        {
            return Ok(());
        }
        let changed = self.conn.execute("UPDATE completion_continuation_attempt SET revision=revision+1,custodian_identity=?3,adopter_identity=?4,phase=CASE WHEN phase IN ('accepted','starting') THEN 'unknown_custody' ELSE phase END WHERE attempt_id=?1 AND owner_generation=?2 AND phase IN ('accepted','starting','unknown_custody','drained','never_started') AND (custodian_identity IS NULL OR custodian_identity=?3) AND (adopter_identity IS NULL OR adopter_identity=?4)", params![attempt.attempt_id,attempt.owner_generation,custodian,adopter]).map_err(|e| e.to_string())?;
        if changed != 1 {
            return Err("original birth attachment fence conflict".into());
        }
        Ok(())
    }
    pub fn continuation_activation(
        &self,
        session_id: &str,
        token: &str,
    ) -> Result<Option<ContinuationAttempt>, String> {
        if domain_on(&self.conn)?.is_none() {
            return Ok(None);
        }
        self.conn.query_row("SELECT attempt_id,owner_generation,operation,request_sha256,source_registration_id,source_listener_revision,session_id,claim_token,result_path FROM completion_continuation_attempt WHERE session_id=?1 AND claim_token=?2 AND operation='activation' AND phase NOT IN ('drained','never_started')",params![session_id,token],|r|Ok(ContinuationAttempt{
            attempt_id:r.get(0)?,owner_generation:r.get(1)?,operation:r.get(2)?,request_sha256:r.get(3)?,source_registration_id:r.get(4)?,source_listener_revision:r.get(5)?,session_id:r.get(6)?,claim_token:r.get(7)?,result_path:r.get(8)?,
        })).optional().map_err(|e|e.to_string())
    }
}

pub(in crate::mailbox) fn cancel_unaccepted_activation_on(
    tx: &Transaction<'_>,
    session: &str,
    token: &str,
) -> Result<(), String> {
    if domain_on(tx)?.is_none() {
        return Ok(());
    }
    // Accepted is persisted before any custodian fork. Thus a current reserved
    // row is conclusive unspent launch authority, unlike a dead accepted owner.
    tx.execute("UPDATE completion_continuation_attempt SET phase='never_started',revision=revision+1,integrated=1,drain_receipt='cancelled_unaccepted_reservation' WHERE session_id=?1 AND claim_token=?2 AND operation='activation' AND phase='reserved' AND revision=1 AND custodian_identity IS NULL",params![session,token]).map_err(|e|e.to_string())?;
    Ok(())
}

pub(in crate::mailbox) fn admit_launcher_on(
    tx: &Transaction<'_>,
    session: &str,
    token: &str,
    child: &ProcessIdentity,
) -> Result<(), String> {
    if domain_on(tx)?.is_none() {
        return Ok(());
    }
    let (attempt,custodian,existing):(String,String,Option<String>)=tx.query_row("SELECT attempt_id,custodian_identity,launcher_identity FROM completion_continuation_attempt WHERE session_id=?1 AND claim_token=?2 AND operation='activation' AND phase IN ('starting','running','unknown_custody')",params![session,token],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|e|e.to_string())?;
    let custodian: SourceProcessIdentity =
        serde_json::from_str(&custodian).map_err(|e| e.to_string())?;
    if child.os_pid != i64::from(std::process::id()) {
        return Err("activation launcher must present its own exact process identity".into());
    }
    #[cfg(target_os = "linux")]
    {
        let parent = unsafe { libc::getppid() };
        let expected = crate::pid_identity::read_live_process_identity(i64::from(parent))?
            .ok_or("activation custodian disappeared")?;
        if custodian.pid != expected.os_pid
            || custodian.boot_id != expected.os_boot_id
            || custodian.starttime_ticks != expected.os_pid_starttime_ticks
        {
            return Err("activation launcher is not beneath its exact custodian".into());
        }
    }
    let identity = serde_json::to_string(&SourceProcessIdentity {
        pid: child.os_pid,
        boot_id: child.os_boot_id.clone(),
        starttime_ticks: child.os_pid_starttime_ticks,
    })
    .map_err(|e| e.to_string())?;
    if let Some(existing) = existing {
        return if existing == identity {
            Ok(())
        } else {
            Err("activation launcher identity conflict".into())
        };
    }
    tx.execute("UPDATE completion_continuation_attempt SET launcher_identity=?2,revision=revision+1 WHERE attempt_id=?1 AND launcher_identity IS NULL",params![attempt,identity]).map_err(|e|e.to_string())?;
    Ok(())
}

pub(in crate::mailbox) fn bind_generation_on(
    tx: &Transaction<'_>,
    request: CreateRuntimeGeneration<'_>,
    creator: &ProcessIdentity,
) -> Result<(), String> {
    if domain_on(tx)?.is_none() {
        return Ok(());
    }
    let Some(session) = request.session_id else {
        return Ok(());
    };
    let active:Option<(String,Option<String>,Option<String>)>=tx.query_row("SELECT attempt_id,launcher_identity,runtime_generation_uuid FROM completion_continuation_attempt WHERE session_id=?1 AND operation='activation' AND phase NOT IN ('drained','never_started')",[session],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?;
    let Some((attempt, launcher, generation)) = active else {
        return Ok(());
    };
    let identity = serde_json::to_string(&SourceProcessIdentity {
        pid: creator.os_pid,
        boot_id: creator.os_boot_id.clone(),
        starttime_ticks: creator.os_pid_starttime_ticks,
    })
    .map_err(|e| e.to_string())?;
    if launcher.as_deref() != Some(&identity)
        || generation
            .as_ref()
            .is_some_and(|id| *id != request.generation_id.to_string())
    {
        return Err(
            "runtime creation conflicts with current session activation reservation".into(),
        );
    }
    tx.execute("UPDATE completion_continuation_attempt SET spawn_invocation_uuid=?2,runtime_generation_uuid=?3,revision=revision+1 WHERE attempt_id=?1",params![attempt,request.spawn_invocation_uuid,request.generation_id.to_string()]).map_err(|e|e.to_string())?;
    Ok(())
}

fn require_exact_attempt(conn: &Connection, request: &ContinuationAttempt) -> Result<(), String> {
    let retained=conn.query_row("SELECT attempt_id,owner_generation,operation,request_sha256,source_registration_id,source_listener_revision,session_id,claim_token,result_path FROM completion_continuation_attempt WHERE attempt_id=?1",[&request.attempt_id],|r|Ok(ContinuationAttempt{attempt_id:r.get(0)?,owner_generation:r.get(1)?,operation:r.get(2)?,request_sha256:r.get(3)?,source_registration_id:r.get(4)?,source_listener_revision:r.get(5)?,session_id:r.get(6)?,claim_token:r.get(7)?,result_path:r.get(8)?})).map_err(|e|e.to_string())?;
    if &retained != request {
        return Err("immutable continuation request conflict".into());
    }
    Ok(())
}

impl MailboxDb {
    /// Called only by the actual recorded driver after a local gate/fork syscall
    /// conclusively failed before any custodian existed. Announcement EOF uses
    /// a separate no-effect classification, not a claim that no AC was forked.
    /// Owner absence or an empty successor is never sufficient for this transition.
    pub fn record_continuation_never_forked(
        &mut self,
        attempt: &ContinuationAttempt,
        reason: &str,
    ) -> Result<(), String> {
        self.record_driver_unreleased(attempt, reason, "no_custodian_created")
    }

    /// Original driver observed announcement EOF while still holding the unsent
    /// execution grant, then waited its exact adopter. This says no effects were
    /// released, not that no AC was created. No replacement emptiness qualifies.
    pub fn record_continuation_unreleased_before_announcement(
        &mut self,
        attempt: &ContinuationAttempt,
        reason: &str,
    ) -> Result<(), String> {
        self.record_driver_unreleased(attempt, reason, "unreleased_announcement_eof")
    }

    fn record_driver_unreleased(
        &mut self,
        attempt: &ContinuationAttempt,
        reason: &str,
        gate: &str,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
            .ok_or("driver process disappeared")?;
        let identity = SourceProcessIdentity {
            pid: live.os_pid,
            boot_id: live.os_boot_id,
            starttime_ticks: live.os_pid_starttime_ticks,
        };
        let encoded = serde_json::to_string(&identity).map_err(|e| e.to_string())?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let driver: String = tx
            .query_row(
                "SELECT driver_identity FROM completion_continuation_owner WHERE generation=?1",
                [&attempt.owner_generation],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if driver != encoded || reason.is_empty() {
            return Err("unreleased receipt is not from exact driver".into());
        }
        let receipt=serde_json::json!({"attempt_id":attempt.attempt_id,"driver":identity,"gate":gate,"reason":reason}).to_string();
        let changed=tx.execute("UPDATE completion_continuation_attempt SET phase='never_started',revision=revision+1,integrated=1,drain_receipt=?2 WHERE attempt_id=?1 AND phase IN ('accepted','unknown_custody') AND custodian_identity IS NULL",params![attempt.attempt_id,receipt]).map_err(|e|e.to_string())?;
        if changed != 1 {
            let retained:Option<String>=tx.query_row("SELECT drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1 AND phase='never_started' AND custodian_identity IS NULL",[&attempt.attempt_id],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
            if retained.as_deref() != Some(&receipt) {
                return Err("receipt no longer matches original unreleased gate".into());
            }
        }
        if attempt.operation == "activation" {
            tx.execute(
                "DELETE FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2",
                params![attempt.session_id, attempt.claim_token],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
}

impl MailboxDb {
    /// Re-executed custodian validates the exact admitted gate before any effect.
    pub fn require_continuation_launch_gate(
        &self,
        attempt: &ContinuationAttempt,
        custodian: &SourceProcessIdentity,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        let live = crate::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
            .ok_or("custodian disappeared")?;
        if custodian.pid != live.os_pid
            || custodian.boot_id != live.os_boot_id
            || custodian.starttime_ticks != live.os_pid_starttime_ticks
        {
            return Err("custodian gate caller mismatch".into());
        }
        let ready:bool=self.conn.query_row("SELECT EXISTS(SELECT 1 FROM completion_continuation_attempt a JOIN completion_continuation_owner o ON o.generation=a.owner_generation WHERE a.attempt_id=?1 AND a.phase='starting' AND a.custodian_identity=?2 AND o.phase='running')",params![attempt.attempt_id,serde_json::to_string(custodian).map_err(|e|e.to_string())?],|r|r.get(0)).map_err(|e|e.to_string())?;
        if !ready {
            return Err("custodian launch gate is not current and released".into());
        }
        Ok(())
    }
}

impl MailboxDb {
    pub fn continuation_domain_drained_runtime(
        &self,
        domain: &str,
        generation: &str,
        invocation: &str,
    ) -> Result<bool, String> {
        self.conn.query_row("SELECT EXISTS(SELECT 1 FROM completion_continuation_attempt
            WHERE domain_id=?1 AND operation='activation' AND phase='drained' AND integrated=1 AND runtime_generation_uuid=?2 AND spawn_invocation_uuid=?3)",
            params![domain, generation, invocation], |r| r.get(0)).map_err(|e| e.to_string())
    }

    pub fn continuation_runtime_identity(
        &self,
        attempt: &ContinuationAttempt,
    ) -> Result<Option<(String, String)>, String> {
        require_exact_attempt(&self.conn, attempt)?;
        self.conn.query_row("SELECT runtime_generation_uuid,spawn_invocation_uuid FROM completion_continuation_attempt WHERE attempt_id=?1 AND runtime_generation_uuid IS NOT NULL AND spawn_invocation_uuid IS NOT NULL",[&attempt.attempt_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())
    }
}

impl MailboxDb {
    /// Continuing obligations, not a process-local driver population.
    pub fn pending_continuation_attempts(&self) -> Result<Vec<ContinuationAttempt>, String> {
        let mut statement = self.conn.prepare("SELECT attempt_id,owner_generation,operation,request_sha256,source_registration_id,source_listener_revision,session_id,claim_token,result_path FROM completion_continuation_attempt WHERE phase NOT IN ('drained','never_started')").map_err(|e| e.to_string())?;
        statement
            .query_map([], |r| {
                Ok(ContinuationAttempt {
                    attempt_id: r.get(0)?,
                    owner_generation: r.get(1)?,
                    operation: r.get(2)?,
                    request_sha256: r.get(3)?,
                    source_registration_id: r.get(4)?,
                    source_listener_revision: r.get(5)?,
                    session_id: r.get(6)?,
                    claim_token: r.get(7)?,
                    result_path: r.get(8)?,
                })
            })
            .map_err(|e| e.to_string())?
            .map(|r| r.map_err(|e| e.to_string()))
            .collect()
    }

    /// Only the retained *original adopting boundary* may certify its drain.
    /// The caller may replay that immutable receipt after producer loss. A new
    /// owner, missing PID, ACK, or an empty unrelated tree grants no discharge.
    pub fn discharge_adopted_continuation_attempt(
        &mut self,
        attempt: &ContinuationAttempt,
        receipt: &str,
    ) -> Result<(), String> {
        require_exact_attempt(&self.conn, attempt)?;
        let value: serde_json::Value = serde_json::from_str(receipt).map_err(|e| e.to_string())?;
        let (custodian, adopter): (String, String) = self.conn.query_row("SELECT custodian_identity,adopter_identity FROM completion_continuation_attempt WHERE attempt_id=?1", [&attempt.attempt_id], |r| Ok((r.get(0)?,r.get(1)?))).map_err(|e| e.to_string())?;
        if value["attempt_id"] != attempt.attempt_id
            || value["owned_children"] != "ECHILD"
            || value["custodian"]
                != serde_json::from_str::<serde_json::Value>(&custodian)
                    .map_err(|e| e.to_string())?
            || value["adopter"]
                != serde_json::from_str::<serde_json::Value>(&adopter).map_err(|e| e.to_string())?
            || value["custodian_wait_status"].as_i64().is_none()
        {
            return Err("adopting drain receipt identity/evidence conflict".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let changed = tx.execute("UPDATE completion_continuation_attempt SET phase='drained',revision=revision+1,integrated=1,drain_receipt=?2 WHERE attempt_id=?1 AND phase IN ('accepted','starting','running','unknown_custody')", params![attempt.attempt_id,receipt]).map_err(|e| e.to_string())?;
        if changed != 1 {
            let retained: Option<String> = tx.query_row("SELECT drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1 AND phase='drained'", [&attempt.attempt_id], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
            if retained.as_deref() != Some(receipt) {
                return Err("adopting result conflicts with terminal evidence".into());
            }
        }
        if attempt.operation == "activation" {
            tx.execute(
                "DELETE FROM session_wake_claim WHERE session_id=?1 AND claim_token=?2",
                params![attempt.session_id, attempt.claim_token],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
}

impl MailboxDb {
    pub fn continuation_launcher_identity(
        &self,
        attempt: &ContinuationAttempt,
    ) -> Result<Option<SourceProcessIdentity>, String> {
        require_exact_attempt(&self.conn, attempt)?;
        let identity: Option<String> = self
            .conn
            .query_row(
                "SELECT launcher_identity FROM completion_continuation_attempt WHERE attempt_id=?1",
                [&attempt.attempt_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        identity
            .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
            .transpose()
    }
}

impl MailboxDb {
    /// The immutable native association remains an obligation during both the
    /// running and drained phases; retirement must not race the drain write.
    pub fn native_runtime_in_domain(
        &self,
        domain: &str,
        generation: &str,
        invocation: &str,
    ) -> Result<bool, String> {
        if self.completion_continuation_domain()?.is_none() {
            return Ok(false);
        }
        self.conn.query_row("SELECT EXISTS(SELECT 1 FROM completion_continuation_attempt WHERE operation='activation' AND domain_id=?1 AND runtime_generation_uuid=?2 AND spawn_invocation_uuid=?3)",
            params![domain,generation,invocation], |r|r.get(0)).map_err(|e|e.to_string())
    }
    /// Exact original enclosing boundary already integrated its drain. This is
    /// attribution/physical evidence, not actor receipts or channel settlement.
    pub fn native_original_drain(
        &self,
        generation: &str,
        invocation: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        if self.completion_continuation_domain()?.is_none() {
            return Ok(None);
        }
        let row: Option<(String, String, String, String, String, String)> = self.conn.query_row(
            "SELECT attempt_id,result_path,custodian_identity,adopter_identity,drain_receipt,domain_id FROM completion_continuation_attempt WHERE operation='activation' AND phase='drained' AND integrated=1 AND runtime_generation_uuid=?1 AND spawn_invocation_uuid=?2",
            params![generation, invocation], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(|e|e.to_string())?;
        row.map(|(attempt, path, ac, adopter, receipt, domain)| -> Result<_, String> {
            Ok(serde_json::json!({"attempt_id":attempt, "result_path":path,"domain_id":domain,
                "custodian":serde_json::from_str::<serde_json::Value>(&ac).map_err(|e|e.to_string())?,
                "adopter":serde_json::from_str::<serde_json::Value>(&adopter).map_err(|e|e.to_string())?,
                "receipt":serde_json::from_str::<serde_json::Value>(&receipt).map_err(|e|e.to_string())?}))
        }).transpose()
    }
}

impl MailboxDb {
    /// Original launched-runtime exit projection, independent of late cancellation.
    /// The runtime producer supplies its retained original attempt; State joins the
    /// exact recorded process and integrated original activation drain. No grant,
    /// transfer claim, or existing terminal history is replaced.
    pub fn exit_native_launched_after_original_drain(
        &mut self,
        generation: &str,
        invocation: &str,
        process: &SourceProcessIdentity,
        reason: RuntimeTerminalReason,
        exit_code: Option<i32>,
        drain_request: Option<&str>,
    ) -> Result<(), String> {
        self.native_original_drain(generation, invocation)?
            .ok_or("native_original_drain_absent")?;
        if !matches!(
            reason,
            RuntimeTerminalReason::OrderlyCompletion | RuntimeTerminalReason::AbnormalTermination
        ) {
            return Err("native_original_process_outcome_absent".into());
        }
        let id = RuntimeGenerationId::parse(generation).map_err(|e| e.to_string())?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let before = runtime_generation_by_id_on(&tx, &id)
            .map_err(|e| e.to_string())?
            .ok_or("native_runtime_absent")?;
        let exact = matches!(&before.exact_process_evidence, ExactProcessEvidence::Recorded(p)
            if p.os_pid == process.pid && p.os_boot_id == process.boot_id && p.os_pid_starttime_ticks == process.starttime_ticks);
        if before.spawn_invocation_uuid != invocation
            || before.spawned_os_pid != Some(process.pid)
            || !exact
        {
            return Err("native_original_process_identity_conflict".into());
        }
        if before.active_delivery_claim_id.is_some()
            || !before.active_delivery_seqs.is_empty()
            || before.active_delivery_claimed_at.is_some()
        {
            return Err("native_delivery_claim_unsettled".into());
        }
        if before.lifecycle_state == RuntimeLifecycleState::Exited {
            return Ok(());
        }
        // Physical drain is not new lifecycle authority. Keep the same
        // predecessor distinction as finish-drain and non-orderly exit.
        match reason {
            RuntimeTerminalReason::OrderlyCompletion
                if before.lifecycle_state == RuntimeLifecycleState::Draining
                    && drain_request.is_some()
                    && before
                        .drain_request_id
                        .as_ref()
                        .map(ToString::to_string)
                        .as_deref()
                        == drain_request => {}
            RuntimeTerminalReason::AbnormalTermination
                if matches!(
                    before.lifecycle_state,
                    RuntimeLifecycleState::Starting | RuntimeLifecycleState::Running
                ) => {}
            _ => return Err("native_original_runtime_illegal_predecessor".into()),
        }
        let reason = match reason {
            RuntimeTerminalReason::OrderlyCompletion => "orderly_completion",
            _ => "abnormal_termination",
        };
        let now = now_rfc3339();
        tx.execute("UPDATE runtime_generation SET lifecycle_state='exited',exited_at=?3,terminal_reason=?4,exit_code=?5 WHERE generation_uuid=?1 AND spawn_invocation_uuid=?2", params![generation,invocation,now,reason,exit_code]).map_err(|e| e.to_string())?;
        settle_runtime_generation_admission_on(&tx, &id).map_err(|e| e.to_string())?;
        let row = runtime_generation_by_id_on(&tx, &id)
            .map_err(|e| e.to_string())?
            .ok_or("native_runtime_absent_after_exit")?;
        project_exited_generation_on(&tx, &row, &now, row.exit_code).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    /// Cancellation-only runtime projection from its original activation drain.
    /// The Starting monitor's lost Q is not rewritten. Generic recovery and
    /// successor-transfer custody fences continue to require their original proof.
    pub fn exit_native_cancelled_after_original_drain(
        &mut self,
        generation: &str,
        invocation: &str,
        launch_never_invoked: bool,
    ) -> Result<(), String> {
        let drain = self
            .native_original_drain(generation, invocation)?
            .ok_or("native_original_drain_absent")?;
        // An explicitly never-invoked Launch projects startup failure from its
        // original physical drain, not from a claim of earlier cancellation.
        // Launched work still requires original cancellation evidence.
        if !launch_never_invoked && !drain["receipt"]["accepted_cancellation"].is_string() {
            return Err("original_native_cancellation_absent".into());
        }
        let id = RuntimeGenerationId::parse(generation).map_err(|e| e.to_string())?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let before = runtime_generation_by_id_on(&tx, &id)
            .map_err(|e| e.to_string())?
            .ok_or("native_runtime_absent")?;
        if before.spawn_invocation_uuid != invocation {
            return Err("native_runtime_fence_conflict".into());
        }
        if before.lifecycle_state == RuntimeLifecycleState::Exited {
            return Ok(());
        }
        // No delivery claim is silently discarded by physical cancellation.
        if before.active_delivery_claim_id.is_some()
            || !before.active_delivery_seqs.is_empty()
            || before.active_delivery_claimed_at.is_some()
        {
            return Err("native_delivery_claim_unsettled".into());
        }
        let reason = if launch_never_invoked {
            if before.spawned_os_pid.is_some()
                || !matches!(
                    before.exact_process_evidence,
                    ExactProcessEvidence::NotRecorded
                )
            {
                return Err("native_never_invoked_launch_has_runtime_process".into());
            }
            "startup_failed"
        } else {
            "cancelled"
        };
        let now = now_rfc3339();
        tx.execute("UPDATE runtime_generation SET lifecycle_state='exited',exited_at=?3,terminal_reason=?4,exit_code=NULL WHERE generation_uuid=?1 AND spawn_invocation_uuid=?2",
            params![generation,invocation,now,reason]).map_err(|e|e.to_string())?;
        settle_runtime_generation_admission_on(&tx, &id).map_err(|e| e.to_string())?;
        let row = runtime_generation_by_id_on(&tx, &id)
            .map_err(|e| e.to_string())?
            .ok_or("native_runtime_absent_after_exit")?;
        project_exited_generation_on(&tx, &row, &now, None).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
}
