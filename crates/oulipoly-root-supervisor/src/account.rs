//! Required, low-rate failure observations. This is an account in the
//! existing root store, never a second authority or an output journal.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) const MAX_RECORDS: usize = 64;
pub(crate) const MAX_RECORD_BYTES: usize = 4096;
pub(crate) const MAX_ACCOUNT_BYTES: usize = MAX_RECORDS * MAX_RECORD_BYTES + 8192;

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct Account {
    pub(crate) records: Vec<Value>,
    counts: BTreeMap<String, u64>,
    omitted: u64,
    prior_unavailable: bool,
    persistence_failures: u64,
    #[serde(skip)]
    persistence: Option<&'static str>,
}

impl Account {
    pub(crate) fn load(encoded: Option<String>) -> Self {
        match encoded {
            None => Self::default(),
            Some(encoded) if encoded.len() <= MAX_ACCOUNT_BYTES => {
                match serde_json::from_str::<Self>(&encoded) {
                    Ok(mut account)
                        if account.records.len() <= MAX_RECORDS
                            && account.counts.len() <= 3
                            && account
                                .records
                                .iter()
                                .all(|r| r.to_string().len() <= MAX_RECORD_BYTES) =>
                    {
                        account.persistence = Some("loaded-store-snapshot");
                        account
                    }
                    _ => Self {
                        prior_unavailable: true,
                        ..Self::default()
                    },
                }
            }
            Some(_) => Self {
                prior_unavailable: true,
                ..Self::default()
            },
        }
    }

    pub(crate) fn note(&mut self, kind: &'static str, mut record: Value) {
        // These keys are host-chosen, never endpoint strings.
        assert!(matches!(
            kind,
            "bash-failure" | "endpoint-record-error" | "bash-spawn-record-error"
        ));
        let count = self.counts.entry(kind.into()).or_default();
        *count = count.saturating_add(1);
        record["kind"] = json!(kind);
        if self.records.len() == MAX_RECORDS || record.to_string().len() > MAX_RECORD_BYTES {
            self.omitted = self.omitted.saturating_add(1);
        } else {
            self.records.push(record);
        }
    }

    pub(crate) fn saved(&mut self) {
        self.persistence = Some("stored");
    }
    pub(crate) fn failed(&mut self, label: &'static str) {
        self.persistence = Some(label);
        self.persistence_failures = self.persistence_failures.saturating_add(1);
    }
    pub(crate) fn encoded(&self) -> String {
        serde_json::to_string(self).expect("account JSON")
    }
    pub(crate) fn incomplete(&self) -> bool {
        self.omitted > 0 || self.prior_unavailable
    }
    pub(crate) fn physical_incomplete(&self) -> bool {
        self.prior_unavailable
            || self.counts.get("bash-failure").copied().unwrap_or(0)
                > self
                    .records
                    .iter()
                    .filter(|r| r["kind"] == "bash-failure")
                    .count() as u64
    }
    pub(crate) fn snapshot(&self) -> Value {
        json!({
            "records": self.records, "counts": self.counts,
            "omitted": self.omitted, "prior_unavailable": self.prior_unavailable,
            "details_complete": !self.incomplete(),
            "persistence": self.persistence.unwrap_or("no-observations"),
            "persistence_failures": self.persistence_failures,
            "bounds": { "records": MAX_RECORDS, "record_bytes": MAX_RECORD_BYTES },
            "retry": "not-authorized",
            "meaning": "host-observed failures; persistence is separate; counts include omitted detail; no ACK, wait, control or publication is inferred from absence",
        })
    }
}

/// Endpoint identifiers are opaque provenance, not payload or authority.
pub(crate) fn tag(value: Option<&str>) -> Value {
    value
        .filter(|s| s.len() <= 256 && !s.chars().any(char::is_control))
        .map_or(Value::Null, |s| json!(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overflow_and_failed_persistence_remain_visible_with_bounded_detail() {
        let mut account = Account::default();
        for i in 0..MAX_RECORDS + 17 {
            account.note(
                "endpoint-record-error",
                json!({"work":i,"message_id":"tag"}),
            );
        }
        assert!(
            !account.physical_incomplete(),
            "endpoint detail overflow changes no physical custody"
        );
        account.note(
            "bash-failure",
            json!({"work":999,"detail":"x".repeat(MAX_RECORD_BYTES+1)}),
        );
        account.failed("store-failed");
        assert!(account.physical_incomplete());
        let summary = account.snapshot();
        assert_eq!(summary["records"].as_array().unwrap().len(), MAX_RECORDS);
        assert_eq!(summary["counts"]["endpoint-record-error"], MAX_RECORDS + 17);
        assert_eq!(summary["counts"]["bash-failure"], 1);
        assert_eq!(summary["omitted"], 18);
        assert_eq!(summary["details_complete"], false);
        assert_eq!(summary["persistence_failures"], 1);
        assert!(account.encoded().len() <= MAX_ACCOUNT_BYTES);
        assert_eq!(
            Account::load(Some(account.encoded())).snapshot()["omitted"],
            18
        );
        assert_eq!(
            Account::load(Some("invalid".into())).snapshot()["prior_unavailable"],
            true
        );
        assert_eq!(tag(Some(&"x".repeat(257))), Value::Null);
    }
}
