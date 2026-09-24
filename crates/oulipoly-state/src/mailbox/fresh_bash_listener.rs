#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreshBashListenerPolicy {
    ResponseOnly,
    Notify,
}

impl FreshBashListenerPolicy {
    fn as_str(self) -> &'static str {
        match self {
            Self::ResponseOnly => "response_only",
            Self::Notify => "notify",
        }
    }
}

fn fresh_bash_listener_schema_count(state: &Connection) -> Result<i64, String> {
    state
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE
         (type='table' AND name='fresh_bash_listener_registration') OR
         (type='trigger' AND name IN ('fresh_bash_listener_registration_no_update',
          'fresh_bash_listener_registration_no_delete'))",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
}

fn verify_fresh_bash_listener_schema(state: &Connection) -> Result<(), String> {
    if fresh_bash_listener_schema_count(state)? != 3 {
        return Err("fresh Bash listener schema is incomplete".into());
    }
    verify_fresh_sql_objects(
        state,
        FRESH_BASH_LISTENER_SCHEMA,
        "fresh_bash_listener_registration",
        &[
            "fresh_bash_listener_registration_no_update",
            "fresh_bash_listener_registration_no_delete",
        ],
    )?;
    Ok(())
}

impl FreshV30Lane {
    /// Called by the broker on original C only, after the pinned Bash actor,
    /// consumed parent work and child D have been verified. A duplicate C may
    /// complete an interrupted insert only with the same immutable policy.
    pub fn register_private_bash_listener(
        &self,
        child: &FreshBashChild,
        policy: FreshBashListenerPolicy,
    ) -> Result<(), String> {
        if self.require_complete_bash_child(&child.request_id)? != *child {
            return Err("fresh Bash listener lacks exact C registration".into());
        }
        if child.listener_policy != policy.as_str() {
            return Err("fresh Bash listener C policy changed".into());
        }
        let (root, root_actor) = self.released_handoff_for_root(&child.root_id)?;
        if root.handoff_id != child.root_handoff_id
            || root.invocation_uuid != child.parent_invocation_uuid
        {
            return Err("fresh Bash listener root changed".into());
        }
        let identity = Self::recipient_identity_json(&root_actor)?;
        let owner_generation = root.old_release.prepared.owner_generation;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state
            .execute_batch("PRAGMA synchronous=FULL")
            .map_err(|e| e.to_string())?;
        state
            .execute(
                "INSERT OR IGNORE INTO fresh_bash_listener_registration
             (request_id,source_id,attempt_id,listener_policy,recipient_identity,
              root_id,owner_generation,registered_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    child.request_id,
                    child.handle,
                    child.invocation_uuid,
                    policy.as_str(),
                    identity,
                    child.root_id,
                    owner_generation,
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(|e| e.to_string())?;
        drop(state);
        self.require_private_bash_listener(child, Some(policy))?;
        Ok(())
    }

    /// Exact readback for c and W. `None` accepts either recorded policy;
    /// a missing policy row never invents notification authority.
    pub fn require_private_bash_listener(
        &self,
        child: &FreshBashChild,
        expected: Option<FreshBashListenerPolicy>,
    ) -> Result<FreshBashListenerPolicy, String> {
        let (root, root_actor) = self.released_handoff_for_root(&child.root_id)?;
        let identity = Self::recipient_identity_json(&root_actor)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let row: Option<(String, String, String, String, String, String)> = state.query_row(
            "SELECT source_id,attempt_id,listener_policy,recipient_identity,root_id,owner_generation
             FROM fresh_bash_listener_registration WHERE request_id=?1",
            [&child.request_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)),
        ).optional().map_err(|e| e.to_string())?;
        let Some((source, attempt, policy, recipient, root_id, generation)) = row else {
            return Err("fresh Bash C listener policy absent or incomplete".into());
        };
        if source != child.handle
            || attempt != child.invocation_uuid
            || policy != child.listener_policy
            || recipient != identity
            || root_id != child.root_id
            || generation != root.old_release.prepared.owner_generation
        {
            return Err("fresh Bash C listener policy binding conflict".into());
        }
        let actual = match policy.as_str() {
            "response_only" => FreshBashListenerPolicy::ResponseOnly,
            "notify" => FreshBashListenerPolicy::Notify,
            _ => return Err("fresh Bash C listener policy invalid".into()),
        };
        if expected.is_some_and(|value| value != actual) {
            return Err("fresh Bash C listener policy replay conflict".into());
        }
        Ok(actual)
    }

    pub fn settle_private_bash_listener(&mut self, request_id: &str) -> Result<(), String> {
        let child = self.require_complete_bash_child(request_id)?;
        if self.require_private_bash_listener(&child, None)? == FreshBashListenerPolicy::Notify {
            let (_, root_actor) = self.released_handoff_for_root(&child.root_id)?;
            self.request_private_bash_notification(request_id, &root_actor)?;
        }
        Ok(())
    }

    /// Startup only revisits accepted W; absent W stays unknown with no F.
    pub fn repair_private_bash_listener_notifications(&mut self) -> Result<(), String> {
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut statement = state
            .prepare(
                "SELECT l.request_id FROM fresh_bash_listener_registration l
             JOIN fresh_bash_selected_event e ON e.request_id=l.request_id
             WHERE l.listener_policy='notify' ORDER BY l.request_id",
            )
            .map_err(|e| e.to_string())?;
        let requests = statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        drop(statement);
        drop(state);
        for request_id in requests {
            self.settle_private_bash_listener(&request_id)?;
        }
        Ok(())
    }
}
