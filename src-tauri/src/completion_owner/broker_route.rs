//! Exact v30 owner and attempt route over the challenged broker connection.
//! The v30 bootstrap selects this route after held J. Scheduling remains
//! closed until its other State and mailbox clients use the retained source.
use oulipoly_kernel_broker::protocol::{
    self, StateReadSpec, StateRoute, StateWriteAction, StateWriteSpec,
};
use oulipoly_state::mailbox::{
    BrokerContinuationReadback, BrokerRecipientSelection, BrokerReleaseEvidence,
    BrokerRepairReadback, BrokerSourceSelection, CompletionDomainOwner, ContinuationAttempt,
    PreparedBrokerOwner,
};
use std::path::{Path, PathBuf};

pub(crate) struct V30OwnerRoute {
    socket: PathBuf,
    source_generation: String,
    root_id: String,
    domain_id: String,
    supervisor_id: String,
    owner_generation: String,
}

impl V30OwnerRoute {
    pub(crate) fn recipient_selection(
        &self,
        owner: &CompletionDomainOwner,
        completed: &BrokerRepairReadback,
    ) -> Result<BrokerRecipientSelection, String> {
        if completed.has_more || !completed.pending_registration_ids.is_empty() {
            return Err("broker recipient selection before source boundary completed".into());
        }
        self.check_repair_readback(completed)?;
        self.read_running(owner, None)?;
        let selection = protocol::read_bounded_recipient_selection_at(
            &self.socket,
            &self.read_spec("broker-recipient-selection-v30", None),
        )
        .map_err(|e| e.to_string())?;
        #[cfg(feature = "age319-private-broker-fixture")]
        if std::env::var_os("AGE319_PRIVATE_RECIPIENT_SELECTION_CHALLENGE_V1").is_some() {
            // A selection reply may be lost. Re-reading is harmless because
            // this operation has no claim or one-use effect to replay.
            if protocol::read_bounded_recipient_selection_at(
                &self.socket,
                &self.read_spec("broker-recipient-selection-v30", None),
            )
            .map_err(|e| e.to_string())?
                != selection
            {
                return Err("private recipient selection readback changed".into());
            }
            for (field, value) in [
                ("root", uuid::Uuid::new_v4().to_string()),
                ("source", uuid::Uuid::new_v4().to_string()),
                ("owner", uuid::Uuid::new_v4().to_string()),
            ] {
                let mut wrong = self.read_spec("broker-recipient-selection-v30", None);
                match field {
                    "root" => wrong.root_id = value,
                    "source" => wrong.source_generation = value,
                    _ => wrong.owner_generation = value,
                }
                if protocol::read_bounded_recipient_selection_at(&self.socket, &wrong).is_ok() {
                    return Err("private foreign recipient selection accepted".into());
                }
            }
        }
        if selection.source_generation != self.source_generation
            || selection.root_id != self.root_id
            || selection.owner_generation != self.owner_generation
            || selection.authority_ordinal != completed.authority_ordinal
            || selection.candidate.as_ref().is_some_and(|candidate| {
                candidate.session_id.is_empty()
                    || candidate.seq <= 0
                    || candidate.payload_sha256.len() != 64
                    || candidate.payload_byte_len < 0
            })
        {
            return Err("broker recipient selection/repair readback conflict".into());
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        if let Some(expected) = std::env::var_os("AGE319_PRIVATE_EXPECT_RECIPIENT_SHA_V1") {
            let selected = selection
                .candidate
                .as_ref()
                .ok_or("private pending recipient selection absent")?;
            if selected.session_id != "fixture-recipient"
                || selected.handle != "fixture-completion"
                || selected.payload_sha256 != expected.to_string_lossy()
            {
                return Err("private retained recipient identity changed".into());
            }
        }
        Ok(selection)
    }

    pub(crate) fn source_selection(
        &self,
        owner: &CompletionDomainOwner,
        completed: &BrokerRepairReadback,
    ) -> Result<BrokerSourceSelection, String> {
        if completed.has_more {
            return Err("broker source selection before State projection completed".into());
        }
        self.check_repair_readback(completed)?;
        self.read_running(owner, None)?;
        let selection = protocol::read_bounded_source_selection_at(
            &self.socket,
            &self.read_spec("broker-source-selection-v30", None),
        )
        .map_err(|e| e.to_string())?;
        #[cfg(feature = "age319-private-broker-fixture")]
        if std::env::var_os("AGE319_PRIVATE_SOURCE_SELECTION_CHALLENGE_V1").is_some() {
            for (field, value) in [
                ("root", uuid::Uuid::new_v4().to_string()),
                ("source", uuid::Uuid::new_v4().to_string()),
                ("owner", uuid::Uuid::new_v4().to_string()),
            ] {
                let mut wrong = self.read_spec("broker-source-selection-v30", None);
                match field {
                    "root" => wrong.root_id = value,
                    "source" => wrong.source_generation = value,
                    _ => wrong.owner_generation = value,
                }
                if protocol::read_bounded_source_selection_at(&self.socket, &wrong).is_ok() {
                    return Err(
                        "private wrong-root/stale-source/sibling-owner source selection accepted"
                            .into(),
                    );
                }
            }
        }
        if selection.source_generation != self.source_generation
            || selection.root_id != self.root_id
            || selection.owner_generation != self.owner_generation
            || selection.authority_ordinal != completed.authority_ordinal
            || selection
                .candidate
                .as_ref()
                .map(|source| &source.registration_id)
                != completed.pending_registration_ids.first()
        {
            return Err("broker source selection/repair readback conflict".into());
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        if let Some(expected) = std::env::var_os("AGE319_PRIVATE_EXPECT_SOURCE_DIGEST_V1") {
            let source = selection
                .candidate
                .as_ref()
                .ok_or("private expected source selection absent")?;
            if source.registration_digest != expected.to_string_lossy()
                || source.listener_revision == 0
                || source.listener.listener_id.is_empty()
            {
                return Err("private source digest/listener selection conflict".into());
            }
            let denied_attempt = ContinuationAttempt {
                attempt_id: uuid::Uuid::new_v4().to_string(),
                owner_generation: self.owner_generation.clone(),
                operation: "source_recovery".into(),
                request_sha256: source.registration_digest.clone(),
                source_registration_id: Some(source.registration_id.clone()),
                source_listener_revision: Some(source.listener_revision as i64),
                session_id: None,
                claim_token: None,
                result_path: "/private/denied-source-result".into(),
            };
            let bypass = self.write_spec(
                "broker-state-write-v1",
                StateWriteAction::Reserve {
                    attempt: denied_attempt,
                },
            );
            if protocol::write_state_at(&self.socket, &bypass).is_ok()
                || protocol::write_state_at(&self.socket, &bypass).is_ok()
            {
                return Err("private ungranted source reservation/replay accepted".into());
            }
        }
        Ok(selection)
    }

    pub(crate) fn repair_page(
        &self,
        owner: &CompletionDomainOwner,
    ) -> Result<BrokerRepairReadback, String> {
        self.read_running(owner, None)?;
        let read = || {
            protocol::read_bounded_repair_at(
                &self.socket,
                &self.read_spec("broker-repair-read-v30", None),
            )
            .map_err(|error| error.to_string())
        };
        let before = read()?;
        self.check_repair_readback(&before)?;
        #[cfg(feature = "age319-private-broker-fixture")]
        if std::env::var_os("AGE319_PRIVATE_REPAIR_CHALLENGE_V1").is_some() {
            for (field, value) in [
                ("root", uuid::Uuid::new_v4().to_string()),
                ("source", uuid::Uuid::new_v4().to_string()),
                ("owner", uuid::Uuid::new_v4().to_string()),
            ] {
                let mut wrong = self.read_spec("broker-repair-read-v30", None);
                match field {
                    "root" => wrong.root_id = value,
                    "source" => wrong.source_generation = value,
                    _ => wrong.owner_generation = value,
                }
                if protocol::read_bounded_repair_at(&self.socket, &wrong).is_ok() {
                    return Err(
                        "private wrong-root/stale-source/sibling-owner repair read accepted".into(),
                    );
                }
            }
            let wrong_revision = self.write_spec(
                "broker-repair-write-v30",
                StateWriteAction::Repair {
                    expected_ordinal: before.authority_ordinal + 1,
                },
            );
            if protocol::write_bounded_repair_at(&self.socket, &wrong_revision).is_ok() {
                return Err("private stale repair revision accepted".into());
            }
        }
        if !before.has_more {
            return Ok(before);
        }
        let spec = self.write_spec(
            "broker-repair-write-v30",
            StateWriteAction::Repair {
                expected_ordinal: before.authority_ordinal,
            },
        );
        #[cfg(feature = "age319-private-broker-fixture")]
        let written = if std::env::var_os("AGE319_PRIVATE_REPAIR_REPLY_LOSS_V1").is_some() {
            protocol::repair_bounded_drop_reply_at(&self.socket, &spec)
                .map(|()| Err(std::io::Error::other("private repair reply lost")))
                .unwrap_or_else(Err)
        } else {
            protocol::write_bounded_repair_at(&self.socket, &spec)
        };
        #[cfg(not(feature = "age319-private-broker-fixture"))]
        let written = protocol::write_bounded_repair_at(&self.socket, &spec);
        // A lost reply is reconciled by a fresh exact broker cursor read. A
        // stagnant cursor while a suffix existed is uncertainty, not success.
        let after = read().map_err(|error| {
            format!(
                "{}; repair readback: {error}",
                written
                    .as_ref()
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "repair reply received".into())
            )
        })?;
        self.check_repair_readback(&after)?;
        if after.authority_ordinal <= before.authority_ordinal
            || after.authority_ordinal > before.authority_ordinal + 64
            || written.as_ref().is_ok_and(|written| written != &after)
        {
            return Err("broker bounded repair cursor/readback conflict".into());
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        if std::env::var_os("AGE319_PRIVATE_REPAIR_CHALLENGE_V1").is_some()
            && protocol::write_bounded_repair_at(&self.socket, &spec).is_ok()
        {
            return Err("private duplicate repair write accepted".into());
        }
        Ok(after)
    }

    fn check_repair_readback(&self, exact: &BrokerRepairReadback) -> Result<(), String> {
        if exact.source_generation != self.source_generation
            || exact.root_id != self.root_id
            || exact.owner_generation != self.owner_generation
            || exact.authority_ordinal < 0
            || exact.pending_registration_ids.len() > 16
        {
            return Err("broker bounded repair readback owner/source conflict".into());
        }
        Ok(())
    }

    /// The guardian derives the generation from Y after its exact root bind.
    /// I must agree with Y; neither a copied sidecar nor a caller path supplies
    /// a source generation.
    pub(crate) fn guardian(
        socket: &Path,
        root_id: &str,
        domain_id: &str,
        supervisor_id: &str,
        owner_generation: &str,
    ) -> Result<Self, String> {
        let observed = Self::observe(socket, root_id, domain_id, supervisor_id, owner_generation)?;
        let prepared =
            protocol::prepared_generation_at(socket, root_id).map_err(|e| e.to_string())?;
        if prepared != observed.source_generation {
            return Err("broker I/Y source generation changed".into());
        }
        Ok(observed)
    }

    /// A driver has no Y authority. I identifies the retained source and each
    /// R/W request is independently checked against its live process stamp.
    pub(crate) fn driver(
        socket: &Path,
        root_id: &str,
        owner: &CompletionDomainOwner,
    ) -> Result<Self, String> {
        Self::observe(
            socket,
            root_id,
            &owner.domain_id,
            &owner.supervisor_authority_id,
            &owner.owner_generation,
        )
    }

    fn observe(
        socket: &Path,
        root_id: &str,
        domain_id: &str,
        supervisor_id: &str,
        owner_generation: &str,
    ) -> Result<Self, String> {
        let StateRoute::BrokerOwned {
            source_generation,
            domain_id: observed_domain,
        } = protocol::state_route_at(socket).map_err(|e| e.to_string())?
        else {
            return Err("broker v30 source is not active".into());
        };
        if observed_domain != domain_id {
            return Err("broker I domain changed".into());
        }
        for id in [root_id, domain_id, supervisor_id, owner_generation] {
            if uuid::Uuid::parse_str(id)
                .map(|id_| id_.to_string() != id)
                .unwrap_or(true)
            {
                return Err("noncanonical v30 owner selector".into());
            }
        }
        Ok(Self {
            socket: socket.into(),
            source_generation,
            root_id: root_id.into(),
            domain_id: domain_id.into(),
            supervisor_id: supervisor_id.into(),
            owner_generation: owner_generation.into(),
        })
    }

    fn read_spec(&self, protocol_name: &str, attempt_id: Option<&str>) -> StateReadSpec {
        StateReadSpec {
            protocol: protocol_name.into(),
            source_generation: self.source_generation.clone(),
            root_id: self.root_id.clone(),
            owner_generation: self.owner_generation.clone(),
            attempt_id: attempt_id.map(str::to_owned),
        }
    }

    fn write_spec(&self, protocol_name: &str, action: StateWriteAction) -> StateWriteSpec {
        StateWriteSpec {
            protocol: protocol_name.into(),
            source_generation: self.source_generation.clone(),
            root_id: self.root_id.clone(),
            owner_generation: self.owner_generation.clone(),
            action,
        }
    }

    pub(crate) fn prepare(
        &self,
        driver_pid: i32,
        endpoint: &Path,
    ) -> Result<PreparedBrokerOwner, String> {
        let spec = self.write_spec(
            "broker-prepared-write-v30",
            StateWriteAction::Prepare {
                driver_pid,
                endpoint: endpoint.to_str().ok_or("invalid owner endpoint")?.into(),
            },
        );
        #[cfg(feature = "age319-private-broker-fixture")]
        let written = if std::env::var_os("AGE319_PRIVATE_NORMAL_PREPARE_REPLY_LOSS_V1").is_some() {
            protocol::prepare_owner_drop_reply_at(&self.socket, &spec)
                .map(|()| Err(std::io::Error::other("private prepared reply lost")))
                .unwrap_or_else(Err)
        } else {
            protocol::prepare_owner_at(&self.socket, &spec)
        };
        #[cfg(not(feature = "age319-private-broker-fixture"))]
        let written = protocol::prepare_owner_at(&self.socket, &spec);
        // Read once by exact root/owner after any reply, including a lost one.
        // A missing readback is uncertainty, not permission to retry W.
        let exact = self
            .read_prepared(driver_pid, endpoint)
            .map_err(|read_error| {
                format!(
                    "{}; readback: {read_error}",
                    written
                        .as_ref()
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "prepared write reply received".into())
                )
            })?;
        if written.as_ref().is_ok_and(|written| written != &exact) {
            return Err("broker prepared owner write/read conflict".into());
        }
        Ok(exact)
    }

    pub(crate) fn read_prepared(
        &self,
        driver_pid: i32,
        endpoint: &Path,
    ) -> Result<PreparedBrokerOwner, String> {
        let exact = protocol::read_prepared_owner_at(
            &self.socket,
            &self.read_spec("broker-prepared-read-v30", None),
        )
        .map_err(|e| e.to_string())?;
        if exact.source_generation != self.source_generation
            || exact.root_id != self.root_id
            || exact.domain_id != self.domain_id
            || exact.supervisor_authority_id != self.supervisor_id
            || exact.owner_generation != self.owner_generation
            || exact.driver.host_pid != driver_pid
            || exact.endpoint != endpoint.to_string_lossy()
        {
            return Err("broker prepared owner readback conflict".into());
        }
        Ok(exact)
    }

    pub(crate) fn release(
        &self,
        prepared: &PreparedBrokerOwner,
    ) -> Result<BrokerReleaseEvidence, String> {
        if prepared.source_generation != self.source_generation
            || prepared.root_id != self.root_id
            || prepared.owner_generation != self.owner_generation
            || prepared.domain_id != self.domain_id
            || prepared.supervisor_authority_id != self.supervisor_id
        {
            return Err("release preparation changed".into());
        }
        let spec = self.write_spec("broker-held-release-v30", StateWriteAction::Release);
        #[cfg(feature = "age319-private-broker-fixture")]
        let written = if std::env::var_os("AGE319_PRIVATE_NORMAL_RELEASE_REPLY_LOSS_V1").is_some() {
            protocol::release_prepared_owner_drop_reply_at(&self.socket, &spec)
                .map(|()| Err(std::io::Error::other("private release reply lost")))
                .unwrap_or_else(Err)
        } else {
            protocol::release_prepared_owner_at(&self.socket, &spec)
        };
        #[cfg(not(feature = "age319-private-broker-fixture"))]
        let written = protocol::release_prepared_owner_at(&self.socket, &spec);
        let exact = protocol::read_released_owner_at(
            &self.socket,
            &self.read_spec("broker-release-readback-v30", None),
        )
        .map_err(|read_error| {
            format!(
                "{}; readback: {read_error}",
                written
                    .as_ref()
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "release write reply received".into())
            )
        })?;
        if exact.prepared != *prepared || written.as_ref().is_ok_and(|written| written != &exact) {
            return Err("broker release readback conflict".into());
        }
        Ok(exact)
    }

    pub(crate) fn read_running(
        &self,
        expected: &CompletionDomainOwner,
        attempt_id: Option<&str>,
    ) -> Result<BrokerContinuationReadback, String> {
        let exact = protocol::read_state_at(
            &self.socket,
            &self.read_spec("broker-state-read-v1", attempt_id),
        )
        .map_err(|e| e.to_string())?;
        if !exact.broker_owned
            || exact.source_generation != self.source_generation
            || exact.root_id != self.root_id
            || exact.owner != *expected
            || exact.owner.domain_id != self.domain_id
            || exact.owner.supervisor_authority_id != self.supervisor_id
            || attempt_id.is_some_and(|id| {
                exact
                    .attempt
                    .as_ref()
                    .is_some_and(|attempt| attempt.attempt_id != id)
            })
        {
            return Err("broker running owner/attempt readback conflict".into());
        }
        Ok(exact)
    }

    pub(crate) fn reserve(
        &self,
        owner: &CompletionDomainOwner,
        attempt: &ContinuationAttempt,
    ) -> Result<BrokerContinuationReadback, String> {
        if attempt.owner_generation != self.owner_generation {
            return Err("reservation owner changed".into());
        }
        self.read_running(owner, None)?;
        let written = protocol::write_state_at(
            &self.socket,
            &self.write_spec(
                "broker-state-write-v1",
                StateWriteAction::Reserve {
                    attempt: attempt.clone(),
                },
            ),
        );
        let exact = self.read_running(owner, Some(&attempt.attempt_id))?;
        if exact.attempt.as_ref() != Some(attempt)
            || exact.phase.as_deref() != Some("reserved")
            || exact.revision != Some(1)
            || (attempt.operation == "activation" && !exact.claim_present)
            || written.as_ref().is_ok_and(|written| written != &exact)
        {
            return Err("broker reservation readback conflict".into());
        }
        Ok(exact)
    }

    pub(crate) fn accept(
        &self,
        owner: &CompletionDomainOwner,
        attempt: &ContinuationAttempt,
    ) -> Result<BrokerContinuationReadback, String> {
        let before = self.read_running(owner, Some(&attempt.attempt_id))?;
        if before.attempt.as_ref() != Some(attempt)
            || before.phase.as_deref() != Some("reserved")
            || before.revision != Some(1)
        {
            return Err("broker acceptance reservation absent".into());
        }
        let written = protocol::write_state_at(
            &self.socket,
            &self.write_spec(
                "broker-state-write-v1",
                StateWriteAction::Accept {
                    attempt_id: attempt.attempt_id.clone(),
                },
            ),
        );
        let exact = self.read_running(owner, Some(&attempt.attempt_id))?;
        if exact.attempt.as_ref() != Some(attempt)
            || exact.phase.as_deref() != Some("accepted")
            || exact.revision != Some(2)
            || written.as_ref().is_ok_and(|written| written != &exact)
        {
            return Err("broker acceptance readback conflict".into());
        }
        Ok(exact)
    }

    pub(crate) fn revoke(
        &self,
        owner: &CompletionDomainOwner,
        attempt: &ContinuationAttempt,
    ) -> Result<BrokerContinuationReadback, String> {
        let before = self.read_running(owner, Some(&attempt.attempt_id))?;
        if before.attempt.as_ref() != Some(attempt)
            || before.phase.as_deref() != Some("reserved")
            || before.revision != Some(1)
        {
            return Err("broker withdrawal reservation absent".into());
        }
        let written = protocol::write_state_at(
            &self.socket,
            &self.write_spec(
                "broker-state-write-v1",
                StateWriteAction::Revoke {
                    attempt: attempt.clone(),
                },
            ),
        );
        let exact = self.read_running(owner, Some(&attempt.attempt_id))?;
        if exact.attempt.as_ref() != Some(attempt)
            || exact.phase.as_deref() != Some("never_started")
            || exact.revision != Some(2)
            || exact.claim_present
            || written.as_ref().is_ok_and(|written| written != &exact)
        {
            return Err("broker withdrawal readback conflict".into());
        }
        Ok(exact)
    }
}
