const FRESH_BASH_SOURCE_VERIFY_BUFFER_BYTES: usize = 64 * 1024;

/// Broker-frozen original tree event for the private Bash child K. The event
/// file and raw output live under the fresh broker root; O is never an input.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshBashSourceEvent {
    pub request_id: String,
    pub source_id: String,
    pub attempt_id: String,
    pub state_admission_id: String,
    pub registration_digest: String,
    pub lane_id: String,
    pub source_generation: String,
    pub session_id: String,
    pub root_id: String,
    pub owner_generation: String,
    pub parent_work_grant_id: String,
    pub parent_work_id: String,
    pub physical_grant_id: String,
    pub physical_work_id: String,
    pub completion_policy: String,
    pub selected_kind: String,
    pub wait_status: i32,
    pub cancelled: bool,
    pub cancel_grant_id: Option<String>,
    pub tree_drained: bool,
    pub output_closed: bool,
    pub stdout_sha256: String,
    pub stdout_len: u64,
    pub stderr_sha256: String,
    pub stderr_len: u64,
}

fn bash_source_registration_digest(child: &FreshBashChild) -> Result<String, String> {
    let bytes = serde_json::to_vec(&(child, "tree")).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn physical_json(directory: &Path, name: &str) -> Result<serde_json::Value, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join(name))
        .map_err(|e| format!("physical {name} lost: {e}"))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.nlink() != 1 {
        return Err(format!("physical {name} is not a retained broker file"));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

fn physical_entry_exists(directory: &Path, name: &str) -> Result<bool, String> {
    match fs::symlink_metadata(directory.join(name)) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn fresh_bash_source_schema_count(state: &Connection) -> Result<i64, String> {
    state.query_row(
        "SELECT count(*) FROM sqlite_master WHERE
         (type='table' AND name IN ('fresh_bash_source_registration','fresh_bash_selected_event')) OR
         (type='trigger' AND name IN ('fresh_bash_source_registration_no_update',
          'fresh_bash_source_registration_no_delete','fresh_bash_selected_event_no_update',
          'fresh_bash_selected_event_no_delete'))",
        [], |r| r.get(0),
    ).map_err(|e| e.to_string())
}

fn verify_fresh_bash_source_schema(state: &Connection) -> Result<(), String> {
    if fresh_bash_source_schema_count(state)? != 6 {
        return Err("fresh Bash source schema is incomplete".into());
    }
    verify_fresh_sql_objects(
        state,
        FRESH_BASH_SOURCE_SCHEMA,
        "fresh_bash_source_registration",
        &[
            "fresh_bash_selected_event",
            "fresh_bash_source_registration_no_update",
            "fresh_bash_source_registration_no_delete",
            "fresh_bash_selected_event_no_update",
            "fresh_bash_selected_event_no_delete",
        ],
    )?;
    Ok(())
}

impl FreshV30Lane {
    pub fn private_bash_source_registration_digest(
        child: &FreshBashChild,
    ) -> Result<String, String> {
        bash_source_registration_digest(child)
    }
    /// C is the registration fence for this fixed private tree source. A
    /// replay can only reproduce the original child and registration digest.
    pub fn register_private_bash_source(&self, child: &FreshBashChild) -> Result<(), String> {
        if self.require_complete_bash_child(&child.request_id)? != *child {
            return Err("fresh Bash source lacks exact C registration".into());
        }
        let digest = bash_source_registration_digest(child)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state
            .execute_batch("PRAGMA synchronous=FULL")
            .map_err(|e| e.to_string())?;
        state.execute(
            "INSERT OR IGNORE INTO fresh_bash_source_registration
             (request_id,source_id,attempt_id,state_admission_id,registration_digest,completion_policy,registered_at)
             VALUES(?1,?2,?3,?4,?5,'tree',?6)",
            params![child.request_id, child.handle, child.invocation_uuid, child.session.allocation_id,
                digest, Utc::now().to_rfc3339()],
        ).map_err(|e| e.to_string())?;
        let found: (String,String,String,String,String) = state.query_row(
            "SELECT source_id,attempt_id,state_admission_id,registration_digest,completion_policy
             FROM fresh_bash_source_registration WHERE request_id=?1",
            [&child.request_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
        ).map_err(|e| e.to_string())?;
        if found
            != (
                child.handle.clone(),
                child.invocation_uuid.clone(),
                child.session.allocation_id.clone(),
                digest,
                "tree".into(),
            )
        {
            return Err("fresh Bash source registration conflict".into());
        }
        Ok(())
    }

    fn verify_captured_bash_event(&self, event: &FreshBashSourceEvent) -> Result<(), String> {
        validate_request_id(&event.request_id)?;
        validate_request_id(&event.physical_grant_id)?;
        let mut child = self
            .read_bash_child(&event.request_id)?
            .ok_or("fresh Bash C absent")?;
        let session = self
            .read_session(&child.d_key)?
            .ok_or("fresh Bash D absent")?;
        self.require_session(&session)?;
        child.session = session.clone();
        let (root, _) = self.released_handoff_for_root(&child.root_id)?;
        let digest = bash_source_registration_digest(&child)?;
        let registration: (String,String,String,String,String) = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?
            .query_row(
                "SELECT source_id,attempt_id,state_admission_id,registration_digest,completion_policy
                 FROM fresh_bash_source_registration WHERE request_id=?1",
                [&event.request_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
            ).map_err(|e| e.to_string())?;
        let checks = [
            (
                "registration",
                registration
                    != (
                        child.handle.clone(),
                        child.invocation_uuid.clone(),
                        child.session.allocation_id.clone(),
                        digest.clone(),
                        "tree".into(),
                    ),
            ),
            ("source", event.source_id != child.handle),
            ("attempt", event.attempt_id != child.invocation_uuid),
            (
                "admission",
                event.state_admission_id != child.session.allocation_id,
            ),
            ("digest", event.registration_digest != digest),
            ("lane", event.lane_id != self.identity.lane_id),
            (
                "generation",
                event.source_generation != self.identity.source_generation,
            ),
            ("session", event.session_id != session.session_id),
            ("root", event.root_id != child.root_id),
            (
                "owner",
                event.owner_generation != root.old_release.prepared.owner_generation,
            ),
            (
                "parent grant",
                event.parent_work_grant_id != child.parent_work_grant_id,
            ),
            ("parent work", event.parent_work_id != child.parent_work_id),
            ("policy", event.completion_policy != "tree"),
            ("drain", !event.tree_drained),
            ("output close", !event.output_closed),
            (
                "selected kind",
                event.selected_kind
                    != if event.cancelled {
                        "cancelled"
                    } else {
                        "tree_drained"
                    },
            ),
            (
                "cancel identity",
                event.cancel_grant_id.as_deref()
                    != if event.cancelled {
                        Some(event.physical_grant_id.as_str())
                    } else {
                        None
                    },
            ),
        ];
        if let Some((name, _)) = checks.into_iter().find(|(_, bad)| *bad) {
            return Err(format!("fresh Bash event {name} conflicts with C/D/root"));
        }
        let directory = self
            .state_path
            .parent()
            .ok_or("fresh State parent absent")?
            .join(FRESH_PROVIDER_DIRECTORY);
        let captured = physical_json(
            &directory,
            &format!("{}.source-event.json", event.physical_grant_id),
        )?;
        let stored: FreshBashSourceEvent =
            serde_json::from_value(captured).map_err(|e| e.to_string())?;
        if stored != *event {
            return Err("captured source receipt changed".into());
        }
        let grant = physical_json(
            &directory,
            &format!("{}.fresh-grant.json", child.request_id),
        )?;
        let consumed = physical_json(
            &directory,
            &format!("{}.consumed.json", event.physical_grant_id),
        )?;
        let attach = physical_json(
            &directory,
            &format!("{}.attach.json", event.physical_grant_id),
        )?;
        let exit = physical_json(
            &directory,
            &format!("{}.exit.json", event.physical_grant_id),
        )?;
        let drain = physical_json(
            &directory,
            &format!("{}.drain.json", event.physical_grant_id),
        )?;
        let wait = physical_json(
            &directory,
            &format!("{}.pid1-wait.json", event.physical_grant_id),
        )?;
        let parent_grant = physical_json(
            &directory,
            &format!("{}.fresh-grant.json", child.root_handoff_id),
        )?;
        let parent_consumed = physical_json(
            &directory,
            &format!("{}.consumed.json", child.parent_work_grant_id),
        )?;
        let parent_attach = physical_json(
            &directory,
            &format!("{}.attach.json", child.parent_work_grant_id),
        )?;
        let b = &grant["binding"];
        let q = &event.physical_grant_id;
        let w = &event.physical_work_id;
        if parent_grant != parent_consumed
            || parent_grant["id"] != child.parent_work_grant_id
            || parent_grant["binding"]["root_id"] != child.root_id
            || parent_grant["binding"]["handoff_id"] != child.root_handoff_id
            || parent_grant["binding"]["invocation_uuid"] != child.parent_invocation_uuid
            || parent_attach["grant_id"] != child.parent_work_grant_id
            || parent_attach["work_id"] != child.parent_work_id
            || grant != consumed
            || grant["id"] != *q
            || grant["version"] != 3
            || b["root_id"] != child.root_id
            || b["handoff_id"] != child.root_handoff_id
            || b["grant_key"] != child.request_id
            || b["invocation_uuid"] != child.invocation_uuid
            || b["session_id"] != session.session_id
            || b["owner_generation"] != root.old_release.prepared.owner_generation
            || b["actor_pid"] != child.actor.host_pid
            || b["actor_starttime"] != child.actor.starttime_ticks
            || b["actor_boot_id"] != child.actor.boot_id
            || b["actor_pidns_dev"] != child.actor.pidns_dev
            || b["actor_pidns_ino"] != child.actor.pidns_ino
            || b["root_pid"] != root.old_release.prepared.root_init.host_pid
            || b["root_starttime"] != root.old_release.prepared.root_init.starttime_ticks
            || b["root_pidns_dev"] != root.old_release.prepared.root_init.pidns_dev
            || b["root_pidns_ino"] != root.old_release.prepared.root_init.pidns_ino
            || b["causal_parent"]["grant_id"] != child.parent_work_grant_id
            || b["causal_parent"]["work_id"] != child.parent_work_id
            || attach["version"] != 1
            || attach["grant_id"] != *q
            || attach["work_id"] != *w
            || exit["version"] != 1
            || exit["grant_id"] != *q
            || exit["work_id"] != *w
            || exit["wait_status"] != event.wait_status
            || exit["provider_local_pid"] != attach["provider_local_pid"]
            || drain["version"] != 1
            || drain["grant_id"] != *q
            || drain["work_id"] != *w
            || drain["cancelled"] != event.cancelled
            || drain["zero_remaining"] != true
            || wait["version"] != 1
            || wait["grant_id"] != *q
            || wait["work_id"] != *w
            || wait["reaped"] != true
            || wait["pid1_parent_namespace_pid"] != attach["pid1_parent_namespace_pid"]
            || wait["wait_status"].as_i64().is_none_or(|status| {
                !libc::WIFEXITED(status as i32) || libc::WEXITSTATUS(status as i32) != 0
            })
        {
            return Err("fresh Bash selected event lacks exact physical K/Q".into());
        }
        if event.cancelled {
            let cancel = physical_json(&directory, &format!("{q}.cancel.json"))?;
            if cancel["grant_id"] != *q || cancel["work_id"] != *w {
                return Err("fresh Bash cancellation intent conflicts with Q".into());
            }
        }
        for (suffix, hash, len) in [
            ("stdout", &event.stdout_sha256, event.stdout_len),
            ("stderr", &event.stderr_sha256, event.stderr_len),
        ] {
            let path = directory.join(format!("{}.{}", event.physical_grant_id, suffix));
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)
                .map_err(|e| format!("original {suffix} lost: {e}"))?;
            let metadata = file.metadata().map_err(|e| e.to_string())?;
            if !metadata.is_file()
                || metadata.uid() != 0
                || metadata.nlink() != 1
                || drain[suffix]["sha256"] != *hash
                || drain[suffix]["bytes"] != len
                || drain[suffix]["device"] != metadata.dev()
                || drain[suffix]["inode"] != metadata.ino()
            {
                return Err(format!("original {suffix} is not regular"));
            }
            let mut reader = file;
            let mut digest = Sha256::new();
            let mut count = 0u64;
            let mut buffer = [0u8; FRESH_BASH_SOURCE_VERIFY_BUFFER_BYTES];
            loop {
                let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
                if read == 0 {
                    break;
                }
                count = count
                    .checked_add(read as u64)
                    .ok_or("source output length overflow")?;
                digest.update(&buffer[..read]);
            }
            if count != len
                || format!("{:x}", digest.finalize()) != *hash
                || reader.metadata().map_err(|e| e.to_string())?.len() != len
            {
                return Err(format!("original {suffix} changed or incomplete"));
            }
        }
        Ok(())
    }

    /// Sidecar capture precedes this single-State transaction. If this call
    /// fails, the immutable receipt remains explicit repair debt. Retrying W
    /// or reopening the broker reuses the same event and never launches K.
    pub fn accept_private_bash_source(&self, event: &FreshBashSourceEvent) -> Result<(), String> {
        self.verify_captured_bash_event(event)?;
        let receipt_json = serde_json::to_string(event).map_err(|e| e.to_string())?;
        let mut state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state
            .execute_batch("PRAGMA synchronous=FULL")
            .map_err(|e| e.to_string())?;
        let tx = state
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR IGNORE INTO fresh_bash_selected_event
             (request_id,source_id,attempt_id,physical_grant_id,receipt_json,selected_at)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                event.request_id,
                event.source_id,
                event.attempt_id,
                event.physical_grant_id,
                receipt_json,
                Utc::now().to_rfc3339()
            ],
        )
        .map_err(|e| e.to_string())?;
        let stored: String = tx
            .query_row(
                "SELECT receipt_json FROM fresh_bash_selected_event WHERE request_id=?1",
                [&event.request_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if stored != receipt_json {
            return Err("fresh selected event replay conflict".into());
        }
        tx.execute(
            "INSERT OR IGNORE INTO fresh_lane_accepted_source
             (source_id,attempt_id,state_admission_id,registration_digest,source_generation,lane_id,root_id,owner_generation,accepted_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![event.source_id,event.attempt_id,event.state_admission_id,event.registration_digest,
                event.source_generation,event.lane_id,event.root_id,event.owner_generation,Utc::now().to_rfc3339()],
        ).map_err(|e| e.to_string())?;
        let accepted: (String,String,String,String,String,String,String) = tx.query_row(
            "SELECT attempt_id,state_admission_id,registration_digest,source_generation,lane_id,root_id,owner_generation
             FROM fresh_lane_accepted_source WHERE source_id=?1",
            [&event.source_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)),
        ).map_err(|e| e.to_string())?;
        if accepted
            != (
                event.attempt_id.clone(),
                event.state_admission_id.clone(),
                event.registration_digest.clone(),
                event.source_generation.clone(),
                event.lane_id.clone(),
                event.root_id.clone(),
                event.owner_generation.clone(),
            )
        {
            return Err("fresh accepted source replay conflict".into());
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Exact repair after broker restart. Only already captured receipts are
    /// reconsidered; absent physical receipts remain unknown and no K is run.
    pub fn repair_captured_private_bash_sources(&self) -> Result<(), String> {
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut statement = state
            .prepare("SELECT request_id FROM fresh_bash_source_registration ORDER BY request_id")
            .map_err(|e| e.to_string())?;
        let requests = statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        drop(statement);
        drop(state);
        let directory = self
            .state_path
            .parent()
            .ok_or("fresh State parent absent")?
            .join(FRESH_PROVIDER_DIRECTORY);
        for request_id in requests {
            let child = self
                .read_bash_child(&request_id)?
                .ok_or("captured Bash C absent")?;
            let physical: Option<String> = self
                .state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?
                .query_row(
                    "SELECT physical_grant_id FROM fresh_bash_selected_event WHERE request_id=?1",
                    [&request_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let selected_in_state = physical.is_some();
            let grant = if let Some(grant) = physical {
                grant
            } else {
                // The grant key is the broker-minted C request, so no path or
                // later event is selected by a caller.
                let name = format!("{}.fresh-grant.json", child.request_id);
                if !physical_entry_exists(&directory, &name)? {
                    continue;
                }
                let value = physical_json(&directory, &name)?;
                value
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or("captured grant id absent")?
                    .to_owned()
            };
            validate_request_id(&grant)?;
            let name = format!("{grant}.source-event.json");
            if !physical_entry_exists(&directory, &name)? {
                if selected_in_state {
                    return Err(format!("selected source event lost: {name}"));
                }
                continue;
            }
            let event: FreshBashSourceEvent =
                serde_json::from_value(physical_json(&directory, &name)?)
                    .map_err(|e| e.to_string())?;
            self.accept_private_bash_source(&event)?;
        }
        Ok(())
    }
}
