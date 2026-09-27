/// This is a child response reservation, independent of parent Q and root
/// terminal publication. `unknown` means the original caller may have seen
/// zero, some, or all of the response. It is never advanced by a socket write.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshBashSyncPublication {
    pub version: u32,
    pub child: FreshBashChild,
    pub event: FreshBashSourceEvent,
    pub phase: String,
    pub outcome: String,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}

fn fresh_bash_sync_publication_schema_count(state: &Connection) -> Result<i64, String> {
    state
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE
         (type='table' AND name='fresh_bash_sync_publication') OR
         (type='trigger' AND name IN ('fresh_bash_sync_publication_no_update',
          'fresh_bash_sync_publication_no_delete'))",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
}

fn verify_fresh_bash_sync_publication_schema(state: &Connection) -> Result<(), String> {
    if fresh_bash_sync_publication_schema_count(state)? != 3 {
        return Err("fresh Bash sync publication schema incomplete".into());
    }
    verify_fresh_sql_objects(
        state,
        FRESH_BASH_SYNC_PUBLICATION_SCHEMA,
        "fresh_bash_sync_publication",
        &[
            "fresh_bash_sync_publication_no_update",
            "fresh_bash_sync_publication_no_delete",
        ],
    )
}

impl FreshV30Lane {
    fn exact_private_bash_sync_publication(
        &self,
        request_id: &str,
        actor: &FreshRecipientIdentity,
    ) -> Result<FreshBashSyncPublication, String> {
        validate_request_id(request_id)?;
        let mut child = self.read_bash_child(request_id)?.ok_or("sync C absent")?;
        child.session = self
            .read_session(&child.d_key)?
            .ok_or("sync child D absent")?;
        if child.actor != *actor || child.listener_policy != "response_only" {
            return Err("sync response requires original response-only Bash actor".into());
        }
        let (root, root_actor) = self.released_handoff_for_root(&child.root_id)?;
        self.require_bash_child(&child, &root, &root_actor, actor)?;
        self.require_private_bash_listener(&child, Some(FreshBashListenerPolicy::ResponseOnly))?;
        let event_json: String = self
            .state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?
            .query_row(
                "SELECT receipt_json FROM fresh_bash_selected_event WHERE request_id=?1",
                [request_id],
                |r| r.get(0),
            )
            .map_err(|e| format!("sync accepted W absent: {e}"))?;
        let event: FreshBashSourceEvent =
            serde_json::from_str(&event_json).map_err(|e| e.to_string())?;
        self.verify_captured_bash_event(&event)?;
        let (outcome, exit_code, signal) = if event.cancelled {
            ("cancelled", None, None)
        } else if libc::WIFEXITED(event.wait_status) {
            ("exited", Some(libc::WEXITSTATUS(event.wait_status)), None)
        } else if libc::WIFSIGNALED(event.wait_status) {
            ("signaled", None, Some(libc::WTERMSIG(event.wait_status)))
        } else {
            ("unknown", None, None)
        };
        Ok(FreshBashSyncPublication {
            version: 1,
            child,
            event,
            phase: "unknown".into(),
            outcome: outcome.into(),
            exit_code,
            signal,
        })
    }

    /// Returns true only to the transaction that created the reservation.
    /// A lost begin reply can be read back, but cannot authorize reprinting.
    pub fn begin_private_bash_sync_publication(
        &self,
        request_id: &str,
        actor: &FreshRecipientIdentity,
    ) -> Result<(FreshBashSyncPublication, bool), String> {
        let exact = self.exact_private_bash_sync_publication(request_id, actor)?;
        let json = serde_json::to_string(&exact).map_err(|e| e.to_string())?;
        let mut state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state
            .execute_batch("PRAGMA synchronous=FULL")
            .map_err(|e| e.to_string())?;
        let tx = state
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let inserted = tx
            .execute(
                "INSERT OR IGNORE INTO fresh_bash_sync_publication VALUES(?1,?2,?3)",
                params![request_id, json, Utc::now().to_rfc3339()],
            )
            .map_err(|e| e.to_string())?
            == 1;
        let stored: String = tx
            .query_row(
                "SELECT receipt_json FROM fresh_bash_sync_publication WHERE request_id=?1",
                [request_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if stored != json {
            return Err("sync publication replay conflict".into());
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok((exact, inserted))
    }

    pub fn read_private_bash_sync_publication(
        &self,
        request_id: &str,
        actor: &FreshRecipientIdentity,
    ) -> Result<Option<FreshBashSyncPublication>, String> {
        let exact = self.exact_private_bash_sync_publication(request_id, actor)?;
        let stored: Option<String> = self
            .state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?
            .query_row(
                "SELECT receipt_json FROM fresh_bash_sync_publication WHERE request_id=?1",
                [request_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        stored
            .map(|json| {
                let read: FreshBashSyncPublication =
                    serde_json::from_str(&json).map_err(|e| e.to_string())?;
                if read != exact {
                    return Err("sync publication artifact changed".into());
                }
                Ok(read)
            })
            .transpose()
    }
}
