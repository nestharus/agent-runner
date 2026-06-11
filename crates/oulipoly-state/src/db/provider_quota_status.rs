//! ## Declared roles
//!
//! - mutator
//!
//! Role set: { mutator }
//!
//! Provider quota status flag and probe timestamp mutations.

use super::*;

impl StateDb {
    pub fn mark_exhausted(&self, provider_name: &str) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        // Upsert so first-use quota failures land the flag even when the
        // provider has never produced a `provider_quotas` row (e.g.,
        // misconfigured quota_script that only ever fails, or a provider
        // whose first call returns quota_exhausted before any refresh has
        // succeeded). Previously a plain UPDATE silently dropped the write
        // for these cases, leaving the account eligible to be routed to
        // again on the next call.
        self.conn
            .execute(
                "INSERT INTO provider_quotas (provider_name, exhausted_at)
                 VALUES (?1, ?2)
                 ON CONFLICT (provider_name) DO UPDATE SET
                    exhausted_at = excluded.exhausted_at",
                sqlite::params![provider_name, &now],
            )
            .map_err(|e| format!("Failed to mark provider exhausted: {e}"))?;
        Ok(())
    }

    pub fn record_provider_unavailable(
        &self,
        provider_name: &str,
        next_available_at: Option<DateTime<Utc>>,
        failure_class: &str,
    ) -> Result<(), String> {
        let next_at = next_available_at.map(|ts| ts.to_rfc3339());
        self.conn
            .execute(
                "INSERT INTO provider_quotas
                    (provider_name, next_available_at, failure_class)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (provider_name) DO UPDATE SET
                    next_available_at = excluded.next_available_at,
                    failure_class = excluded.failure_class",
                params![provider_name, next_at, failure_class],
            )
            .map_err(|e| format!("Failed to record provider unavailable: {e}"))?;
        Ok(())
    }

    pub fn clear_provider_unavailable(&self, provider_name: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE provider_quotas
                 SET next_available_at = NULL,
                     failure_class = NULL
                 WHERE provider_name = ?1",
                params![provider_name],
            )
            .map_err(|e| format!("Failed to clear provider unavailable: {e}"))?;
        Ok(())
    }

    pub fn touch_provider_refresh(
        &self,
        provider_name: &str,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let ts = now.to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO provider_quotas (provider_name, last_refresh_at)
                 VALUES (?1, ?2)
                 ON CONFLICT (provider_name) DO UPDATE SET
                    last_refresh_at = excluded.last_refresh_at",
                params![provider_name, ts],
            )
            .map_err(|e| format!("Failed to touch provider refresh: {e}"))?;
        Ok(())
    }

    pub fn clear_exhausted(&self, provider_name: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE provider_quotas SET exhausted_at = NULL WHERE provider_name = ?1",
                sqlite::params![provider_name],
            )
            .map_err(|e| format!("Failed to clear provider exhausted flag: {e}"))?;
        Ok(())
    }

    pub fn record_topology_probe(&self, provider_name: &str) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        self.conn
            .execute(
                "INSERT INTO provider_quotas (provider_name, last_topology_probe_at)
                 VALUES (?1, ?2)
                 ON CONFLICT (provider_name) DO UPDATE SET
                    last_topology_probe_at = excluded.last_topology_probe_at",
                sqlite::params![provider_name, &now],
            )
            .map_err(|e| format!("Failed to record topology probe: {e}"))?;
        Ok(())
    }
}
