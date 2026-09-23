//! Fail-closed host entry staging for the unfinished kernel handoff. This
//! branch runs before maintenance, CLI parsing, GUI startup, and owner forks.
use oulipoly_kernel_broker::protocol::{Operation, request};
use std::process::ExitCode;

const REQUIRED_ENV: &str = "OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1";

pub(crate) fn host_entry() -> Option<ExitCode> {
    if std::env::var_os(REQUIRED_ENV).is_none() {
        return None;
    }
    let result = stage_host_entry(
        || {
            let response = request(Operation::ReserveEntry).map_err(|e| e.to_string())?;
            let id = response
                .strip_prefix("reserved ")
                .and_then(|s| s.strip_suffix('\n'))
                .ok_or_else(|| format!("kernel entry reservation refused: {}", response.trim()))?;
            uuid::Uuid::parse_str(id).map_err(|_| "invalid broker root ID".to_owned())?;
            Ok(id.to_owned())
        },
        |root_id| {
            // The reservation is broker-generated. It is never itself a grant.
            // The guardian must bind its pinned host incarnation and domain
            // before it can publish a root authority for this ID.
            unsafe {
                std::env::set_var(
                    crate::completion_owner::KERNEL_ROOT_RESERVATION_ENV,
                    root_id,
                )
            };
            crate::completion_owner::bootstrap_service()?;
            let raw = std::env::var(crate::completion_owner::ROOT_AUTHORITY_ENV)
                .map_err(|_| "host guardian did not publish a root authority".to_owned())?;
            let grant: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|_| "invalid host guardian root authority".to_owned())?;
            grant
                .get("root_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .ok_or_else(|| "host guardian root ID absent".to_owned())
        },
    );
    match result {
        Ok(()) => unreachable!("kernel handoff cannot yet release a child"),
        Err(error) => {
            eprintln!("OULIPOLY_KERNEL_ENTRY_GAP={error}");
            Some(ExitCode::FAILURE)
        }
    }
}

fn stage_host_entry(
    reserve: impl FnOnce() -> Result<String, String>,
    bind: impl FnOnce(&str) -> Result<String, String>,
) -> Result<(), String> {
    let root_id = reserve()?;
    let bound_id = bind(&root_id)?;
    if bound_id != root_id {
        return Err("host guardian root ID differs from broker reservation".into());
    }
    // No CLI/GUI argv, TTY/FD, environment, authenticated child join or nested
    // accepted-work gate is available yet. Stop before any provider dispatch.
    Err("root child handoff is not implemented; no Runner was released".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn production_staging_orders_broker_reservation_before_guardian_and_never_dispatches() {
        let steps = RefCell::new(Vec::new());
        let id = uuid::Uuid::new_v4().to_string();
        let result = stage_host_entry(
            || {
                steps.borrow_mut().push("reserve");
                Ok(id.clone())
            },
            |root| {
                steps.borrow_mut().push("guardian");
                Ok(root.to_owned())
            },
        );
        assert_eq!(steps.into_inner(), ["reserve", "guardian"]);
        assert!(result.unwrap_err().contains("no Runner was released"));
    }

    #[test]
    fn broker_and_guardian_root_mismatch_fails_closed() {
        let result = stage_host_entry(
            || Ok(uuid::Uuid::new_v4().to_string()),
            |_| Ok(uuid::Uuid::new_v4().to_string()),
        );
        assert!(result.unwrap_err().contains("differs"));
    }
}
