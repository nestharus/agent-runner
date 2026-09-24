//! Ordered schema evolution for the shared PID sidecar.
//! Each migration step names the bounded entity that owns its schema change;
//! fresh construction and installed-version upgrades are separate paths.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub(super) const COMPLETION_PROVENANCE_TRIGGER_SQL: &str =
    include_str!("migrations/0024_completion_mailbox_provenance_trigger.sql");

fn migrate_completed_turn_retention(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("migrations/0020_completed_turn_retention.sql"))
        .map_err(|error| error.to_string())
}

fn migrate_completion_recovery_working_set(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!(
        "migrations/0021_completion_recovery_working_set.sql"
    ))
    .map_err(|error| error.to_string())
}

fn migrate_live_history_barrier(conn: &Connection) -> Result<(), String> {
    let has_retirement_projection: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM pragma_table_info('completion_event_listener')
                WHERE name='retirement_pending')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !has_retirement_projection {
        conn.execute_batch(
            "ALTER TABLE completion_event_listener
             ADD COLUMN retirement_pending INTEGER NOT NULL DEFAULT 1
             CHECK (retirement_pending IN (0, 1));",
        )
        .map_err(|error| error.to_string())?;
    }
    conn.execute_batch(include_str!("migrations/0022_live_history_barrier.sql"))
        .and_then(|_| {
            conn.execute_batch(include_str!(
                "migrations/0022_completion_native_runtime.sql"
            ))
        })
        .map_err(|error| error.to_string())
}

fn migrate_record_timestamp_contract(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!(
        "migrations/0023_record_timestamp_contract.sql"
    ))
    .map_err(|error| error.to_string())
}

fn migrate_completion_mailbox_provenance(conn: &Connection) -> Result<(), String> {
    let present: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('mailbox')
             WHERE name='completion_provenance')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !present {
        conn.execute_batch(include_str!(
            "migrations/0024_completion_mailbox_provenance.sql"
        ))
        .map_err(|error| error.to_string())?;
    }
    // Older test fixtures can carry newer schema objects while presenting an
    // older user_version. Recreate all three triggers without touching rows.
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS mailbox_completion_provenance_insert_valid;
         DROP TRIGGER IF EXISTS mailbox_completion_provenance_update_valid;
         DROP TRIGGER IF EXISTS mailbox_completion_provenance_immutable;",
    )
    .map_err(|error| error.to_string())?;
    // Installed pending rows stay unknown at open. A session-scoped retry
    // reconciles one row at a time, including relational evidence, without
    // walking pending history while holding the schema writer.
    conn.execute_batch(COMPLETION_PROVENANCE_TRIGGER_SQL)
        .map_err(|error| error.to_string())
}

fn migrate_completion_attempt_sources(conn: &Connection) -> Result<(), String> {
    let present: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('completion_continuation_attempt')
         WHERE name='association_completeness')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !present {
        conn.execute_batch(
            "ALTER TABLE completion_continuation_attempt
            ADD COLUMN association_completeness TEXT NOT NULL DEFAULT 'unknown';",
        )
        .map_err(|error| error.to_string())?;
    }
    let source_present: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('completion_continuation_source')
             WHERE name='attempt_association_history')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !source_present {
        // The old source set may have shared activations whose claims are gone.
        // This metadata-only addition avoids a historical scan under the writer.
        conn.execute_batch(
            "ALTER TABLE completion_continuation_source
             ADD COLUMN attempt_association_history TEXT NOT NULL DEFAULT 'unknown';",
        )
        .map_err(|error| error.to_string())?;
    }
    conn.execute_batch(include_str!(
        "migrations/0025_completion_attempt_sources.sql"
    ))
    .map_err(|error| error.to_string())
}

fn migrate_completion_attempt_search_generation(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!(
        "migrations/0026_completion_attempt_search_generation.sql"
    ))
    .map_err(|error| error.to_string())
}

#[derive(Deserialize)]
struct CompletionProtocolSummary {
    #[serde(default, deserialize_with = "present_string")]
    completion_protocol: Option<String>,
    #[serde(default, deserialize_with = "present_u64")]
    schema_version: Option<u64>,
    kind: Option<String>,
    #[serde(default)]
    meta: ObjectField,
    #[serde(default)]
    snapshot: PresentField,
    #[serde(default)]
    outcome: PresentField,
    #[serde(default)]
    source_id: PresentField,
    #[serde(default)]
    registration_id: PresentField,
}

#[derive(Default)]
struct PresentField(bool);

#[derive(Default)]
struct ObjectField(bool);

impl<'de> Deserialize<'de> for ObjectField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = ObjectField;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a legacy completion metadata object")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(ObjectField(true))
            }
        }
        deserializer.deserialize_map(ObjectVisitor)
    }
}

impl<'de> Deserialize<'de> for PresentField {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        IgnoredAny::deserialize(deserializer)?;
        Ok(Self(true))
    }
}

fn present_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

fn present_u64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    u64::deserialize(deserializer).map(Some)
}

pub(super) fn classify_existing_completion_payload(
    conn: &Connection,
    envelope: &str,
    file_path: Option<&str>,
    digest: Option<&str>,
    byte_len: Option<i64>,
    policy: Option<&str>,
    compacted_at: Option<&str>,
) -> Option<&'static str> {
    let summary = if let (Some(path), Some(digest), Some(len), Some(policy)) =
        (file_path, digest, byte_len, policy)
    {
        let root = Path::new(conn.path()?).parent()?;
        let expected = root
            .join(super::MAILBOX_PAYLOAD_DIRECTORY)
            .join(super::MAILBOX_PAYLOAD_ADDRESS_VERSION)
            .join(super::MAILBOX_PAYLOAD_ALGORITHM)
            .join(digest.get(..2)?)
            .join(digest);
        if PathBuf::from(path) != expected {
            return None;
        }
        if policy != super::MAILBOX_PAYLOAD_RETENTION_POLICY {
            return None;
        }
        let metadata = std::fs::symlink_metadata(&expected).ok()?;
        if !metadata.is_file()
            || metadata.permissions().readonly() == false
            || metadata.len() != u64::try_from(len).ok()?
        {
            return None;
        }
        let file = std::fs::File::open(&expected).ok()?;
        let opened = file.metadata().ok()?;
        if !opened.is_file()
            || !opened.permissions().readonly()
            || opened.len() != u64::try_from(len).ok()?
        {
            return None;
        }
        let mut reader = DigestReader {
            inner: std::io::BufReader::new(file),
            digest: Sha256::new(),
            bytes: 0,
        };
        let summary = serde_json::from_reader::<_, CompletionProtocolSummary>(&mut reader).ok()?;
        if reader.bytes != u64::try_from(len).ok()?
            || format!("{:x}", reader.digest.finalize()) != digest
        {
            return None;
        }
        summary
    } else if file_path.is_none()
        && digest.is_none()
        && byte_len.is_none()
        && policy.is_none()
        && compacted_at.is_none()
    {
        serde_json::from_str::<CompletionProtocolSummary>(envelope).ok()?
    } else {
        return None;
    };
    classify_completion_summary(summary)
}

// Parse and hash the same descriptor. A rename or replacement between two
// opens cannot supply a digest from one file and protocol fields from another.
struct DigestReader<R> {
    inner: R,
    digest: Sha256,
    bytes: u64,
}

impl<R: Read> Read for DigestReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.digest.update(&buffer[..read]);
        self.bytes += read as u64;
        Ok(read)
    }
}

pub(super) fn classify_direct_completion_payload_json(payload: &str) -> &'static str {
    let Ok(summary) = serde_json::from_str::<CompletionProtocolSummary>(payload) else {
        return "unclassified";
    };
    if summary.completion_protocol.as_deref() == Some(crate::completion_continuation::PROTOCOL) {
        return "v2";
    }
    if summary.completion_protocol.is_some()
        || summary.snapshot.0
        || summary.outcome.0
        || summary.source_id.0
        || summary.registration_id.0
    {
        return "unclassified";
    }
    // This API is the direct generic enqueue boundary. The route itself is
    // the positive legacy fact for a newly written row; old rows lack it and
    // use the stricter retained-payload classifier above.
    "legacy"
}

fn classify_completion_summary(summary: CompletionProtocolSummary) -> Option<&'static str> {
    match summary.completion_protocol.as_deref() {
        Some(crate::completion_continuation::PROTOCOL) => Some("v2"),
        None if summary.schema_version == Some(2)
            && summary.kind.as_deref() == Some(super::AGENT_BASH_COMPLETE_KIND)
            && summary.meta.0
            && !summary.snapshot.0
            && !summary.outcome.0
            && !summary.source_id.0
            && !summary.registration_id.0 =>
        {
            Some("legacy")
        }
        _ => None,
    }
}

pub(super) const CURRENT_VERSION: i64 = 29;
pub(super) const BROKER_OWNED_VERSION: i64 = 30;
const MAX_SUPPORTED_VERSION: i64 = CURRENT_VERSION;
const SCHEMA_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidecarEntity {
    NamespaceAuthority,
    MailboxDelivery,
    CompletionAuthority,
    RuntimeLifecycle,
    SessionAdmission,
    WakeAndSessionMetadata,
    PayloadRetention,
}

struct MigrationStep {
    target_version: i64,
    owner: SidecarEntity,
    apply: fn(&Connection) -> Result<(), String>,
}

const SCHEMA_STEPS: &[MigrationStep] = &[
    MigrationStep {
        target_version: 2,
        owner: SidecarEntity::NamespaceAuthority,
        apply: ensure_namespace_schema,
    },
    MigrationStep {
        target_version: 2,
        owner: SidecarEntity::MailboxDelivery,
        apply: ensure_mailbox_delivery_schema,
    },
    MigrationStep {
        target_version: 2,
        owner: SidecarEntity::CompletionAuthority,
        apply: ensure_completion_authority_schema,
    },
    MigrationStep {
        target_version: 2,
        owner: SidecarEntity::RuntimeLifecycle,
        apply: ensure_runtime_lifecycle_schema,
    },
    MigrationStep {
        target_version: 2,
        owner: SidecarEntity::WakeAndSessionMetadata,
        apply: ensure_wake_and_session_metadata_schema,
    },
    MigrationStep {
        target_version: 2,
        owner: SidecarEntity::PayloadRetention,
        apply: ensure_payload_retention_schema,
    },
    MigrationStep {
        target_version: 3,
        owner: SidecarEntity::WakeAndSessionMetadata,
        apply: ensure_wake_process_identity_schema,
    },
    MigrationStep {
        target_version: 4,
        owner: SidecarEntity::RuntimeLifecycle,
        apply: ensure_runtime_creator_identity_schema,
    },
    MigrationStep {
        target_version: 5,
        owner: SidecarEntity::MailboxDelivery,
        apply: ensure_mailbox_delivery_settlement_schema,
    },
    MigrationStep {
        target_version: 6,
        owner: SidecarEntity::SessionAdmission,
        apply: ensure_session_admission_schema,
    },
    MigrationStep {
        target_version: 7,
        owner: SidecarEntity::SessionAdmission,
        apply: ensure_session_admission_launcher_identity_schema,
    },
    MigrationStep {
        target_version: 8,
        owner: SidecarEntity::WakeAndSessionMetadata,
        apply: ensure_wake_sweep_progress_schema,
    },
    MigrationStep {
        target_version: 9,
        owner: SidecarEntity::SessionAdmission,
        apply: ensure_session_admission_scaling_indexes,
    },
    MigrationStep {
        target_version: 10,
        owner: SidecarEntity::PayloadRetention,
        apply: ensure_terminal_history_retention_schema,
    },
    MigrationStep {
        target_version: 11,
        owner: SidecarEntity::PayloadRetention,
        apply: ensure_terminal_payload_lookup_indexes,
    },
    MigrationStep {
        target_version: 12,
        owner: SidecarEntity::MailboxDelivery,
        apply: ensure_mailbox_delivery_settlement_schema,
    },
    MigrationStep {
        target_version: 13,
        owner: SidecarEntity::MailboxDelivery,
        apply: migrate_headless_observation_fence,
    },
    MigrationStep {
        target_version: 14,
        owner: SidecarEntity::MailboxDelivery,
        apply: ensure_observation_stop_schema,
    },
    MigrationStep {
        target_version: 15,
        owner: SidecarEntity::MailboxDelivery,
        apply: ensure_delivery_finalization_schema,
    },
    MigrationStep {
        target_version: 16,
        owner: SidecarEntity::MailboxDelivery,
        apply: migrate_receipt_scan,
    },
    MigrationStep {
        target_version: 17,
        owner: SidecarEntity::RuntimeLifecycle,
        apply: migrate_starting_custody,
    },
    MigrationStep {
        target_version: 18,
        owner: SidecarEntity::CompletionAuthority,
        apply: migrate_completion_continuation,
    },
    MigrationStep {
        target_version: 19,
        owner: SidecarEntity::CompletionAuthority,
        apply: migrate_notification_settlement,
    },
    MigrationStep {
        target_version: 20,
        owner: SidecarEntity::MailboxDelivery,
        apply: migrate_completed_turn_retention,
    },
    MigrationStep {
        target_version: 21,
        owner: SidecarEntity::CompletionAuthority,
        apply: migrate_completion_recovery_working_set,
    },
    MigrationStep {
        target_version: 22,
        owner: SidecarEntity::MailboxDelivery,
        apply: migrate_live_history_barrier,
    },
    MigrationStep {
        target_version: 23,
        owner: SidecarEntity::PayloadRetention,
        apply: migrate_record_timestamp_contract,
    },
    MigrationStep {
        target_version: 24,
        owner: SidecarEntity::CompletionAuthority,
        apply: migrate_completion_mailbox_provenance,
    },
    MigrationStep {
        target_version: 25,
        owner: SidecarEntity::CompletionAuthority,
        apply: migrate_completion_attempt_sources,
    },
    MigrationStep {
        target_version: 26,
        owner: SidecarEntity::CompletionAuthority,
        apply: migrate_completion_attempt_search_generation,
    },
    MigrationStep {
        target_version: 27,
        owner: SidecarEntity::CompletionAuthority,
        apply: migrate_kernel_root_owner,
    },
    MigrationStep {
        target_version: 28,
        owner: SidecarEntity::CompletionAuthority,
        apply: migrate_native_grant_binding,
    },
    MigrationStep {
        target_version: 29,
        owner: SidecarEntity::CompletionAuthority,
        apply: migrate_native_worker_kernel_q,
    },
];

fn migrate_native_worker_kernel_q(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("migrations/0029_native_worker_kernel_q.sql"))
        .map_err(|error| error.to_string())
}

fn migrate_native_grant_binding(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("migrations/0028_native_grant_binding.sql"))
        .map_err(|error| error.to_string())
}

fn migrate_kernel_root_owner(conn: &Connection) -> Result<(), String> {
    let has_root_column: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('completion_continuation_owner') WHERE name='kernel_root_id')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !has_root_column {
        return conn
            .execute_batch(include_str!("migrations/0027_kernel_root_owner.sql"))
            .map_err(|error| error.to_string());
    }
    // A synthetic downgrade or nonstandard recovery may retain the column
    // while its version is old. Add only the missing index, with the canonical
    // SQL text used by the current-schema fingerprint.
    let has_root_index: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='completion_continuation_owner_kernel_root')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !has_root_index {
        conn.execute_batch(
            "CREATE INDEX completion_continuation_owner_kernel_root
ON completion_continuation_owner(kernel_root_id)
WHERE kernel_root_id IS NOT NULL;",
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn migrate_notification_settlement(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("migrations/0019_notification_settlement.sql"))
        .map_err(|error| error.to_string())
}

fn migrate_completion_continuation(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("migrations/0018_completion_continuation.sql"))
        .map_err(|error| error.to_string())?;
    // Schema, identity and version commit together. Legacy namespace identity,
    // admissions and pending work are not converted into v2 recovery authority.
    conn.execute(
        "INSERT INTO completion_continuation_domain VALUES(1,?1,'main-native-completion-v2')",
        [Uuid::new_v4().to_string()],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn migrate_starting_custody(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("migrations/0017_starting_custody.sql"))
        .map_err(|error| error.to_string())
}

// Version and fingerprint are one WAL read snapshot. The caller retains the
// namespace fence throughout; a committed current schema needs no SQLite writer.
fn observe_valid_current(conn: &Connection) -> Result<bool, String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("Failed to observe sidecar schema: {e}"))?;
    let version = sidecar_version(&tx)?;
    validate_supported_version(version)?;
    if version == MAX_SUPPORTED_VERSION {
        super::completion_continuation::validate_schema_on(&tx)?;
    }
    tx.commit()
        .map_err(|e| format!("Failed to finish sidecar schema observation: {e}"))?;
    Ok(version == MAX_SUPPORTED_VERSION)
}

pub(super) fn ensure(conn: &mut Connection) -> Result<(), String> {
    ensure_with_deadline(conn, None)
}

pub(super) fn ensure_without_wait(conn: &mut Connection) -> Result<(), String> {
    ensure_with_deadline(conn, Some(Instant::now()))
}

fn ensure_with_deadline(conn: &mut Connection, deadline: Option<Instant>) -> Result<(), String> {
    if observe_valid_current(conn)? {
        return Ok(());
    }
    loop {
        // Reobserve on every retry, including after a busy handler used the
        // remaining deadline. Another opener's committed migration is enough.
        if observe_valid_current(conn)? {
            return Ok(());
        }

        #[cfg(test)]
        after_stale_observation();
        let tx = match rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate) {
            Ok(tx) => tx,
            Err(error) if super::sqlite_error_is_contention(&error) => {
                // The failed BEGIN retained no transaction. Reobserve even at
                // the deadline, without extending writer patience.
                if observe_valid_current(conn)? {
                    return Ok(());
                }
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    return Err(format!(
                        "Failed to lock PID mailbox sidecar schema migration: {error}"
                    ));
                }
                std::thread::sleep(SCHEMA_LOCK_RETRY_INTERVAL);
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "Failed to lock PID mailbox sidecar schema migration: {error}"
                ));
            }
        };
        let locked_version = sidecar_version(&tx)?;
        validate_supported_version(locked_version)?;
        if locked_version >= CURRENT_VERSION {
            // Do not serialize fingerprint work under a stale writer lease.
            tx.commit().map_err(|err| {
                format!("Failed to finish PID mailbox sidecar schema check: {err}")
            })?;
            continue;
        }
        if locked_version == 0 {
            create_fresh_schema(&tx)?;
        } else {
            upgrade_installed_schema(&tx, locked_version)?;
        }
        tx.pragma_update(None, "user_version", CURRENT_VERSION)
            .map_err(|err| format!("Failed to record PID mailbox sidecar schema version: {err}"))?;
        // A migration must not commit a current version whose completed
        // provenance, attempt, or kernel-owner schema fails ordinary readback.
        super::completion_continuation::validate_schema_on(&tx)?;
        return tx.commit().map_err(|err| {
            format!("Failed to commit PID mailbox sidecar schema migration: {err}")
        });
    }
}

#[cfg(test)]
thread_local! {
    static STALE_OBSERVATION_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
}
#[cfg(test)]
fn after_stale_observation() {
    STALE_OBSERVATION_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}

fn validate_supported_version(version: i64) -> Result<(), String> {
    if (0..=MAX_SUPPORTED_VERSION).contains(&version) {
        return Ok(());
    }
    Err(format!(
        "Unsupported PID mailbox sidecar schema version {version}; expected 0..={MAX_SUPPORTED_VERSION}"
    ))
}

/// Existing writable authority paths do not run the ordinary migration. They
/// must still refuse a sidecar written by a newer broker-owned protocol.
pub(super) fn validate_existing_writer_version(conn: &Connection) -> Result<(), String> {
    validate_supported_version(sidecar_version(conn)?)
}

pub(super) fn validate_exact_v29(conn: &Connection) -> Result<(), String> {
    if observe_valid_current(conn)? {
        Ok(())
    } else {
        Err("broker cutover requires a complete v29 sidecar".into())
    }
}

pub(super) fn validate_broker_owned(conn: &Connection) -> Result<String, String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|error| format!("Failed to observe broker sidecar: {error}"))?;
    if sidecar_version(&tx)? != BROKER_OWNED_VERSION {
        return Err("broker sidecar requires schema version 30".into());
    }
    super::completion_continuation::validate_broker_schema_on(&tx)?;
    let definition: String = tx
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='broker_sidecar_authority'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("broker authority schema missing: {error}"))?;
    if definition != BROKER_AUTHORITY_SCHEMA {
        return Err("broker authority schema changed".into());
    }
    let owner_definition: String = tx
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='broker_completion_owner'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("broker owner schema missing: {error}"))?;
    if owner_definition != BROKER_OWNER_SCHEMA {
        return Err("broker owner schema changed".into());
    }
    let prepared_definition: String = tx
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='broker_prepared_owner'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("broker prepared owner schema missing: {error}"))?;
    if prepared_definition != BROKER_PREPARED_OWNER_SCHEMA {
        return Err("broker prepared owner schema changed".into());
    }
    for (name, expected) in [
        (
            "broker_prepared_owner_immutable",
            BROKER_PREPARED_OWNER_IMMUTABLE,
        ),
        ("broker_prepared_owner_retain", BROKER_PREPARED_OWNER_RETAIN),
        (
            "broker_prepared_owner_no_running",
            BROKER_PREPARED_OWNER_NO_RUNNING,
        ),
    ] {
        let definition: String = tx
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?1",
                [name],
                |row| row.get(0),
            )
            .map_err(|error| format!("broker prepared owner trigger missing: {error}"))?;
        if definition != expected {
            return Err("broker prepared owner trigger changed".into());
        }
    }
    let generation: String = tx
        .query_row(
            "SELECT source_generation FROM broker_sidecar_authority WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("broker source generation missing: {error}"))?;
    uuid::Uuid::parse_str(&generation)
        .map_err(|_| "broker source generation is invalid".to_string())?;
    tx.commit()
        .map_err(|error| format!("Failed to finish broker sidecar observation: {error}"))?;
    Ok(generation)
}

pub(super) const BROKER_AUTHORITY_SCHEMA: &str = "CREATE TABLE broker_sidecar_authority (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    source_generation TEXT NOT NULL,
    activated_at TEXT NOT NULL
)";

pub(super) const BROKER_OWNER_SCHEMA: &str = "CREATE TABLE broker_completion_owner (
    owner_generation TEXT PRIMARY KEY REFERENCES completion_continuation_owner(generation),
    source_generation TEXT NOT NULL,
    root_id TEXT NOT NULL,
    guardian_identity TEXT NOT NULL,
    driver_identity TEXT NOT NULL
)";

// v30 is not released. Prepared rows deliberately have no FK into the v18
// running-owner table: an owner waiting at J must never enter its election,
// reservation, acceptance, or source predicates.
pub(super) const BROKER_PREPARED_OWNER_SCHEMA: &str = "CREATE TABLE broker_prepared_owner (
    owner_generation TEXT PRIMARY KEY,
    source_generation TEXT NOT NULL,
    root_id TEXT NOT NULL UNIQUE,
    owner_uid INTEGER NOT NULL CHECK(owner_uid>=0 AND owner_uid<=4294967295),
    domain_id TEXT NOT NULL,
    supervisor_authority_id TEXT NOT NULL,
    endpoint TEXT NOT NULL,
    entry_identity TEXT NOT NULL UNIQUE,
    guardian_identity TEXT NOT NULL UNIQUE,
    driver_identity TEXT NOT NULL UNIQUE,
    root_init_identity TEXT NOT NULL UNIQUE,
    joined_child_identity TEXT NOT NULL UNIQUE
)";

pub(super) const BROKER_PREPARED_OWNER_IMMUTABLE: &str =
    "CREATE TRIGGER broker_prepared_owner_immutable
BEFORE UPDATE ON broker_prepared_owner
BEGIN SELECT RAISE(ABORT,'prepared owner is immutable'); END";

pub(super) const BROKER_PREPARED_OWNER_RETAIN: &str = "CREATE TRIGGER broker_prepared_owner_retain
BEFORE DELETE ON broker_prepared_owner
BEGIN SELECT RAISE(ABORT,'prepared owner must be retained'); END";

// A later reviewed release transition must replace this inert boundary with
// an atomic broker-proven running/release commit. No current caller may turn a
// prepared record into the old running owner by direct publication.
pub(super) const BROKER_PREPARED_OWNER_NO_RUNNING: &str =
    "CREATE TRIGGER broker_prepared_owner_no_running
BEFORE INSERT ON completion_continuation_owner
WHEN EXISTS (SELECT 1 FROM broker_prepared_owner WHERE owner_generation=NEW.generation)
BEGIN SELECT RAISE(ABORT,'prepared owner cannot be published as running'); END";

fn create_fresh_schema(conn: &Connection) -> Result<(), String> {
    apply_steps(conn, SCHEMA_STEPS)
}

fn upgrade_installed_schema(conn: &Connection, stored_version: i64) -> Result<(), String> {
    if stored_version == 24 {
        reconcile_alternate_v24(conn)?;
    }
    for target_version in (stored_version + 1)..=CURRENT_VERSION {
        let steps = SCHEMA_STEPS
            .iter()
            .filter(|step| step.target_version == target_version)
            .collect::<Vec<_>>();
        if steps.is_empty() {
            return Err(format!(
                "PID mailbox migration manifest has no steps for version {target_version}"
            ));
        }
        for step in steps {
            (step.apply)(conn).map_err(|err| {
                format!(
                    "PID mailbox {:?} migration to version {} failed: {err}",
                    step.owner, step.target_version
                )
            })?;
        }
    }
    Ok(())
}

// AGE-319's unpublished v24 used this ordinal for the kernel owner column;
// merged main uses v24 for mailbox provenance. The exact alternate shape can
// acquire provenance in this transaction before the published v25/v26 steps.
// An unrecognized v24 must not be advanced under the wrong history.
fn reconcile_alternate_v24(conn: &Connection) -> Result<(), String> {
    let has_provenance: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('mailbox') WHERE name='completion_provenance')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if has_provenance {
        return Ok(());
    }
    let kernel_index: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='index' AND name='completion_continuation_owner_kernel_root'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let has_kernel_column: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('completion_continuation_owner') WHERE name='kernel_root_id' AND type='TEXT')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let has_later_columns: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('completion_continuation_attempt') WHERE name='association_completeness')
               OR EXISTS(SELECT 1 FROM pragma_table_info('completion_continuation_source') WHERE name='attempt_association_history')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if !has_kernel_column
        || kernel_index.as_deref()
            != Some(
                "CREATE INDEX completion_continuation_owner_kernel_root\nON completion_continuation_owner(kernel_root_id)\nWHERE kernel_root_id IS NOT NULL",
            )
        || has_later_columns
    {
        return Err("unsupported_transition_required: unrecognized PID mailbox v24 lineage".into());
    }
    migrate_completion_mailbox_provenance(conn)
}

fn apply_steps(conn: &Connection, steps: &[MigrationStep]) -> Result<(), String> {
    for step in steps {
        (step.apply)(conn).map_err(|err| {
            format!(
                "PID mailbox {:?} fresh-schema step failed: {err}",
                step.owner
            )
        })?;
    }
    Ok(())
}

fn ensure_namespace_schema(conn: &Connection) -> Result<(), String> {
    crate::pid_identity::ensure_identity_schema(conn)?;
    conn.execute_batch(super::mailbox_schema_definition())
        .map_err(|err| format!("Failed to ensure sidecar entity tables: {err}"))?;
    super::ensure_mailbox_sidecar_identity_locked(conn)
}

fn ensure_mailbox_delivery_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_mailbox_columns(conn)?;
    super::ensure_mailbox_target_index(conn)?;
    super::ensure_mailbox_delivery_owner_index(conn)
}

fn ensure_mailbox_delivery_settlement_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_mailbox_delivery_attempt_columns(conn)
}

fn ensure_completion_authority_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "INSERT OR IGNORE INTO completion_authority_materialization_summary (
            invocation_uuid, materialized_count, authority_ordinal,
            sidecar_generation, continuity_digest
         )
         SELECT head.invocation_uuid,
                (SELECT COUNT(*)
                 FROM completion_authority_continuity AS counted
                 WHERE counted.invocation_uuid = head.invocation_uuid),
                head.authority_ordinal, head.sidecar_generation, head.continuity_digest
         FROM completion_authority_continuity AS head
         WHERE head.authority_ordinal = (
            SELECT MAX(candidate.authority_ordinal)
            FROM completion_authority_continuity AS candidate
            WHERE candidate.invocation_uuid = head.invocation_uuid
         )
         AND (
            SELECT COUNT(*)
            FROM completion_authority_continuity AS counted
            WHERE counted.invocation_uuid = head.invocation_uuid
         ) = (
            SELECT COUNT(*)
            FROM completion_authority_continuity AS continuity
            JOIN completion_event AS event
              ON event.event_id = continuity.event_id
             AND event.kind = 'agent_bash_complete'
            JOIN completion_event_listener AS listener
              ON listener.event_id = continuity.event_id
             AND listener.listener_id = continuity.owner_invocation_uuid
             AND listener.owner_invocation_uuid = continuity.owner_invocation_uuid
             AND listener.session_id = continuity.owner_session_id
            WHERE continuity.invocation_uuid = head.invocation_uuid
         );",
    )
    .map_err(|err| format!("Failed to backfill completion materialization summary: {err}"))
}

fn ensure_runtime_lifecycle_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_runtime_generation_columns(conn)?;
    promote_legacy_runtime_authorities(conn)
}

fn ensure_session_admission_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(super::session_admission_schema_definition())
        .map_err(|err| format!("Failed to ensure session admission queue: {err}"))
}

fn ensure_session_admission_launcher_identity_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_session_admission_launcher_identity_schema(conn)
}

fn ensure_session_admission_scaling_indexes(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(super::session_admission_scaling_indexes_definition())
        .map_err(|err| format!("Failed to ensure session admission scaling indexes: {err}"))
}

fn ensure_terminal_history_retention_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_terminal_history_retention_schema(conn)
}

fn ensure_terminal_payload_lookup_indexes(conn: &Connection) -> Result<(), String> {
    super::ensure_terminal_payload_lookup_indexes(conn)
}

fn ensure_wake_and_session_metadata_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_session_runtime_columns(conn)
}

fn ensure_wake_process_identity_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_wake_claim_process_identity_columns(conn)
}

fn ensure_wake_sweep_progress_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(super::wake_sweep_progress_schema_definition())
        .map_err(|err| format!("Failed to ensure wake sweep progress schema: {err}"))
}

fn ensure_runtime_creator_identity_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_runtime_generation_columns(conn)?;
    super::settle_unverifiable_runtime_generations(conn)
}

fn ensure_payload_retention_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_mailbox_compaction_index(conn)
}

#[derive(Debug)]
struct LegacyRuntimeAuthority {
    session_id: String,
    mode: String,
    invocation_uuid: String,
    provider_name: String,
    model_name: Option<String>,
    pty_control_path: Option<String>,
    updated_at: String,
    os_pid: i64,
    os_boot_id: String,
    os_pid_starttime_ticks: i64,
    turn_started_at: Option<String>,
    models_dir: Option<String>,
    effective_cwd: Option<String>,
}

fn promote_legacy_runtime_authorities(conn: &Connection) -> Result<(), String> {
    settle_incomplete_legacy_runtime_authorities(conn)?;
    let rows = read_complete_legacy_runtime_authorities(conn)?;
    for row in rows {
        promote_legacy_runtime_authority(conn, &row)?;
    }
    Ok(())
}

fn settle_incomplete_legacy_runtime_authorities(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "UPDATE session_runtime
         SET run_state = 'idle',
             pty_control_path = NULL,
             running_invocation_uuid = NULL,
             running_os_pid = NULL,
             running_os_boot_id = NULL,
             running_os_pid_starttime_ticks = NULL,
             turn_ended_at = COALESCE(turn_ended_at, updated_at)
         WHERE run_state = 'running'
           AND (running_invocation_uuid IS NULL
             OR running_os_pid IS NULL
             OR running_os_boot_id IS NULL
             OR running_os_pid_starttime_ticks IS NULL)",
        [],
    )
    .map_err(|err| format!("Failed to settle incomplete legacy runtime authority: {err}"))?;
    Ok(())
}

fn read_complete_legacy_runtime_authorities(
    conn: &Connection,
) -> Result<Vec<LegacyRuntimeAuthority>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT session_id, mode, running_invocation_uuid,
                    COALESCE(provider_name, 'legacy-unknown'), model_name,
                    pty_control_path, updated_at, running_os_pid,
                    running_os_boot_id, running_os_pid_starttime_ticks,
                    turn_started_at, models_dir, effective_cwd
             FROM session_runtime
             WHERE run_state = 'running'
               AND running_invocation_uuid IS NOT NULL
               AND running_os_pid IS NOT NULL
               AND running_os_boot_id IS NOT NULL
               AND running_os_pid_starttime_ticks IS NOT NULL",
        )
        .map_err(|err| format!("Failed to prepare legacy runtime promotion: {err}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(LegacyRuntimeAuthority {
                session_id: row.get(0)?,
                mode: row.get(1)?,
                invocation_uuid: row.get(2)?,
                provider_name: row.get(3)?,
                model_name: row.get(4)?,
                pty_control_path: row.get(5)?,
                updated_at: row.get(6)?,
                os_pid: row.get(7)?,
                os_boot_id: row.get(8)?,
                os_pid_starttime_ticks: row.get(9)?,
                turn_started_at: row.get(10)?,
                models_dir: row.get(11)?,
                effective_cwd: row.get(12)?,
            })
        })
        .map_err(|err| format!("Failed to query legacy runtime promotion rows: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read legacy runtime promotion row: {err}"))
}

fn promote_legacy_runtime_authority(
    conn: &Connection,
    legacy: &LegacyRuntimeAuthority,
) -> Result<(), String> {
    if let Some((generation_uuid, generation_session_id, lifecycle_state, exit_code)) = conn
        .query_row(
            "SELECT generation_uuid, session_id, lifecycle_state, exit_code
             FROM runtime_generation
             WHERE identity_os_pid = ?1
               AND identity_os_boot_id = ?2
               AND identity_os_pid_starttime_ticks = ?3",
            params![
                legacy.os_pid,
                &legacy.os_boot_id,
                legacy.os_pid_starttime_ticks
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i32>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|err| format!("Failed to match promoted runtime identity: {err}"))?
    {
        attach_promoted_runtime_session(conn, &generation_uuid, generation_session_id, legacy)?;
        if lifecycle_state == "exited" {
            settle_legacy_runtime_projection(conn, legacy, exit_code)?;
        }
        return Ok(());
    }
    if session_has_nonterminal_generation(conn, &legacy.session_id)? {
        return Ok(());
    }

    let created_at = legacy
        .turn_started_at
        .as_deref()
        .unwrap_or(&legacy.updated_at);
    conn.execute(
        "INSERT INTO runtime_generation (
            generation_uuid, lifecycle_state, spawn_invocation_uuid, session_id,
            runtime_mode, provider_name, model_name, pty_control_path, models_dir,
            effective_cwd, spawned_os_pid, identity_os_pid, identity_os_boot_id,
            identity_os_pid_starttime_ticks, created_at, running_at
         ) VALUES (?1, 'running', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                   ?10, ?10, ?11, ?12, ?13, ?13)",
        params![
            Uuid::new_v4().to_string(),
            &legacy.invocation_uuid,
            &legacy.session_id,
            &legacy.mode,
            &legacy.provider_name,
            &legacy.model_name,
            &legacy.pty_control_path,
            &legacy.models_dir,
            &legacy.effective_cwd,
            legacy.os_pid,
            &legacy.os_boot_id,
            legacy.os_pid_starttime_ticks,
            created_at,
        ],
    )
    .map_err(|err| format!("Failed to promote legacy runtime authority: {err}"))?;
    Ok(())
}

fn settle_legacy_runtime_projection(
    conn: &Connection,
    legacy: &LegacyRuntimeAuthority,
    exit_code: Option<i32>,
) -> Result<(), String> {
    conn.execute(
        "UPDATE session_runtime
         SET run_state = 'idle',
             pty_control_path = NULL,
             running_invocation_uuid = NULL,
             running_os_pid = NULL,
             running_os_boot_id = NULL,
             running_os_pid_starttime_ticks = NULL,
             turn_ended_at = COALESCE(turn_ended_at, updated_at),
             last_exit_code = ?2
         WHERE session_id = ?1",
        params![&legacy.session_id, exit_code],
    )
    .map_err(|err| format!("Failed to settle promoted terminal runtime projection: {err}"))?;
    Ok(())
}

fn attach_promoted_runtime_session(
    conn: &Connection,
    generation_uuid: &str,
    generation_session_id: Option<String>,
    legacy: &LegacyRuntimeAuthority,
) -> Result<(), String> {
    match generation_session_id {
        Some(session_id) if session_id == legacy.session_id => Ok(()),
        Some(session_id) => Err(format!(
            "Legacy runtime identity for session {} already belongs to session {session_id}",
            legacy.session_id
        )),
        None => {
            conn.execute(
                "UPDATE runtime_generation SET session_id = ?2 WHERE generation_uuid = ?1",
                params![generation_uuid, &legacy.session_id],
            )
            .map_err(|err| format!("Failed to attach promoted runtime session: {err}"))?;
            Ok(())
        }
    }
}

fn session_has_nonterminal_generation(conn: &Connection, session_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM runtime_generation INDEXED BY idx_runtime_generation_live_session
            WHERE session_id = ?1 AND lifecycle_state != 'exited'
         )",
        params![session_id],
        |row| row.get(0),
    )
    .map_err(|err| format!("Failed to inspect promoted runtime session: {err}"))
}

pub(super) fn sidecar_version(conn: &Connection) -> Result<i64, String> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|err| format!("Failed to read PID mailbox sidecar schema version: {err}"))
}

fn migrate_headless_observation_fence(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("0013_headless_observation_fence.sql"))
        .map_err(|err| format!("Failed to migrate headless observation fence: {err}"))
}

fn ensure_observation_stop_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("0014_observation_stop.sql"))
        .map_err(|err| format!("Failed to create observation stop history: {err}"))
}

fn ensure_delivery_finalization_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("0015_delivery_finalization.sql"))
        .map_err(|err| format!("Failed to create delivery finalization references: {err}"))
}

fn migrate_receipt_scan(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("0016_receipt_scan.sql"))
        .map_err(|err| format!("Failed to migrate receipt scan: {err}"))
}

/// Existing older-schema fixtures must remove newer objects as well as lower
/// user_version; this helper is never available to production migration code.
#[cfg(test)]
pub(super) fn remove_continuation_schema_for_legacy_fixture(conn: &Connection) {
    remove_record_timestamp_contract_for_legacy_fixture(conn);
    remove_attempt_search_generation_for_legacy_fixture(conn);
    conn.execute_batch(
        "DROP VIEW mailbox_retained_delivery_finalizers;
        DROP INDEX mailbox_completed_turn_pins_attempt;
        DROP TABLE mailbox_completed_turn_pins;
        DROP TABLE mailbox_completed_turn_tails;
        DROP TRIGGER completion_continuation_notification_ack;
        DROP TABLE completion_continuation_notification;
        DROP TABLE completion_native_kernel_q;
        DROP TABLE completion_native_worker_attach;
        DROP TRIGGER completion_native_grant_no_legacy_terminal;
        DROP TABLE completion_native_grant_binding;
        DROP TABLE completion_continuation_attempt_source;
        DROP TABLE completion_continuation_attempt;
        DROP TABLE completion_continuation_source;
        DROP TABLE completion_continuation_context;
        DROP TABLE completion_continuation_owner;
        DROP TABLE completion_supervisor_inheritance;
        DROP TABLE completion_supervisor_authority;
        DROP TABLE completion_continuation_domain;
        DROP TRIGGER completion_continuation_claim_delete;
        DROP TRIGGER completion_continuation_claim_replace;",
    )
    .unwrap();
}

/// Downgrade-only test support for fixtures that preserve the v18 continuation
/// tables while removing the additive v21 root-supervisor authority layer.
#[cfg(test)]
pub(crate) fn remove_completion_recovery_working_set_for_legacy_fixture(conn: &Connection) {
    remove_record_timestamp_contract_for_legacy_fixture(conn);
    remove_attempt_search_generation_for_legacy_fixture(conn);
    conn.execute_batch(
        "DROP TABLE completion_native_kernel_q;
         DROP TABLE completion_native_worker_attach;
         DROP TRIGGER completion_native_grant_no_legacy_terminal;
         DROP TABLE completion_native_grant_binding;
         DROP INDEX IF EXISTS completion_continuation_owner_kernel_root;
         ALTER TABLE completion_continuation_owner DROP COLUMN kernel_root_id;
         DROP INDEX IF EXISTS idx_mailbox_pending_session_live;
         DROP INDEX IF EXISTS idx_mailbox_pending_target_live;
         DROP INDEX IF EXISTS idx_mailbox_deliverable_session_live;
         DROP INDEX IF EXISTS idx_mailbox_deliverable_target_live;
         DROP INDEX IF EXISTS idx_mailbox_deliverable_global;
         DROP INDEX IF EXISTS idx_mailbox_delivery_attempt_unresolved;
         DROP INDEX IF EXISTS idx_completion_event_listener_session_live;
         DROP INDEX IF EXISTS idx_completion_event_listener_unacknowledged;
         DROP INDEX IF EXISTS idx_completion_event_listener_retirement_pending;
         DROP INDEX IF EXISTS idx_runtime_generation_live_session;
         DROP INDEX IF EXISTS completion_continuation_attempt_native_runtime;
         ALTER TABLE completion_event_listener DROP COLUMN retirement_pending;
         DROP TRIGGER completion_owner_supervisor_authority_insert;
         DROP TRIGGER completion_owner_supervisor_authority_immutable;
         DROP TRIGGER completion_source_supervisor_authority_insert;
         DROP TRIGGER completion_source_supervisor_authority_immutable;
         DROP TRIGGER completion_attempt_supervisor_authority_insert;
         DROP TRIGGER completion_attempt_supervisor_authority_immutable;
         DROP INDEX completion_continuation_attempt_unresolved;
         DROP INDEX completion_continuation_source_unaccepted;
         DROP INDEX completion_supervisor_inheritance_predecessor;
         DROP TABLE completion_supervisor_inheritance;
         DROP TABLE completion_supervisor_authority;
         ALTER TABLE completion_continuation_owner DROP COLUMN supervisor_authority_id;
         ALTER TABLE completion_continuation_source DROP COLUMN supervisor_authority_id;
         ALTER TABLE completion_continuation_attempt DROP COLUMN supervisor_authority_id;",
    )
    .unwrap();
}

#[cfg(test)]
fn remove_attempt_search_generation_for_legacy_fixture(conn: &Connection) {
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS completion_continuation_attempt_search_insert;
         DROP TRIGGER IF EXISTS completion_continuation_attempt_search_update;
         DROP TRIGGER IF EXISTS completion_continuation_attempt_search_delete;
         DROP TRIGGER IF EXISTS completion_continuation_attempt_source_search_insert;
         DROP TRIGGER IF EXISTS completion_continuation_attempt_source_search_update;
         DROP TRIGGER IF EXISTS completion_continuation_attempt_source_search_delete;
         DROP TABLE IF EXISTS completion_continuation_attempt_search_generation;",
    )
    .unwrap();
}

#[cfg(test)]
fn remove_record_timestamp_contract_for_legacy_fixture(conn: &Connection) {
    let present: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master
                           WHERE type='table' AND name='sidecar_timestamp_repairs')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    if !present {
        return;
    }
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS mailbox_timestamp_after_insert;
         DROP TRIGGER IF EXISTS mailbox_timestamp_after_delivery;
         DROP TRIGGER IF EXISTS mailbox_timestamp_after_terminal_error;
         DROP TRIGGER IF EXISTS mailbox_enqueued_at_immutable;
         DROP TRIGGER IF EXISTS mailbox_closed_at_immutable;
         DROP TRIGGER IF EXISTS mailbox_delivered_at_immutable;
         DROP TRIGGER IF EXISTS mailbox_delivery_attempt_timestamp_after_insert;
         DROP TRIGGER IF EXISTS mailbox_delivery_attempt_timestamp_after_update;
         DROP TRIGGER IF EXISTS mailbox_delivery_attempt_created_at_immutable;
         DROP TRIGGER IF EXISTS mailbox_delivery_attempt_resolved_at_immutable;
         DROP TRIGGER IF EXISTS completion_event_timestamp_after_trigger;
         DROP TRIGGER IF EXISTS completion_event_triggered_at_immutable;
         DROP TRIGGER IF EXISTS completion_event_created_at_immutable;
         DROP TRIGGER IF EXISTS completion_event_terminal_reopen_forbidden;
         DROP TRIGGER IF EXISTS completion_listener_timestamp_after_reactivation;
         DROP TRIGGER IF EXISTS completion_listener_timestamp_after_retirement;
         DROP TRIGGER IF EXISTS completion_listener_created_at_immutable;
         DROP TRIGGER IF EXISTS completion_listener_closed_at_immutable;
         DROP TRIGGER IF EXISTS runtime_generation_timestamp_after_transition;
         DROP TRIGGER IF EXISTS runtime_generation_created_at_immutable;
         DROP TRIGGER IF EXISTS runtime_generation_exited_at_immutable;
         DROP TRIGGER IF EXISTS runtime_generation_terminal_reopen_forbidden;
         DROP INDEX IF EXISTS idx_mailbox_retention_eligible_v23;
         DROP INDEX IF EXISTS idx_mailbox_delivery_attempt_retention_v23;
         DROP INDEX IF EXISTS idx_completion_event_retention_v23;
         DROP INDEX IF EXISTS idx_completion_event_listener_retention_v23;
         DROP INDEX IF EXISTS idx_runtime_generation_retention_v23;
         ALTER TABLE mailbox DROP COLUMN retention_status;
         ALTER TABLE mailbox DROP COLUMN retention_eligible_at;
         ALTER TABLE mailbox DROP COLUMN closed_at;
         ALTER TABLE mailbox_delivery_attempts DROP COLUMN retention_status;
         ALTER TABLE mailbox_delivery_attempts DROP COLUMN retention_eligible_at;
         ALTER TABLE mailbox_delivery_attempts DROP COLUMN updated_at;
         ALTER TABLE completion_event DROP COLUMN retention_status;
         ALTER TABLE completion_event DROP COLUMN retention_eligible_at;
         ALTER TABLE completion_event DROP COLUMN closed_at;
         ALTER TABLE completion_event DROP COLUMN updated_at;
         ALTER TABLE completion_event_listener DROP COLUMN retention_status;
         ALTER TABLE completion_event_listener DROP COLUMN retention_eligible_at;
         ALTER TABLE completion_event_listener DROP COLUMN closed_at;
         ALTER TABLE completion_event_listener DROP COLUMN updated_at;
         ALTER TABLE runtime_generation DROP COLUMN retention_status;
         ALTER TABLE runtime_generation DROP COLUMN retention_eligible_at;
         ALTER TABLE runtime_generation DROP COLUMN updated_at;
         DROP TABLE sidecar_timestamp_repairs;",
    )
    .unwrap();
}

#[cfg(test)]
mod contention_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    #[test]
    fn published_v24_to_v28_upgrade_to_native_worker_ledger_preserves_attempts() {
        for version in 24..=28 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("pid-identity.db");
            let conn = Connection::open(&path).unwrap();
            let steps: Vec<_> = SCHEMA_STEPS
                .iter()
                .filter(|step| step.target_version <= version)
                .map(|step| MigrationStep {
                    target_version: step.target_version,
                    owner: step.owner,
                    apply: step.apply,
                })
                .collect();
            apply_steps(&conn, &steps).unwrap();
            let domain: String = conn
                .query_row(
                    "SELECT domain_id FROM completion_continuation_domain",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            conn.execute(
                "INSERT INTO completion_supervisor_authority
                 (authority_id,domain_id,phase,created_by_generation,guardian_identity)
                 VALUES('older-authority',?1,'active','older-owner','{}')",
                [&domain],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO completion_continuation_owner
                 (generation,domain_id,phase,guardian_identity,driver_identity,
                  endpoint,supervisor_authority_id)
                 VALUES('older-owner',?1,'lost','{}','{}','/older/owner','older-authority')",
                [&domain],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO completion_continuation_attempt
                 (attempt_id,domain_id,owner_generation,operation,request_sha256,
                  phase,revision,result_path)
                 VALUES('older-attempt',?1,'older-owner','transport','digest',
                        'accepted',2,'/older/result')",
                [&domain],
            )
            .unwrap();
            conn.pragma_update(None, "user_version", version).unwrap();
            drop(conn);

            let db = super::super::MailboxDb::open(&path).unwrap();
            assert_eq!(sidecar_version(db.connection()).unwrap(), CURRENT_VERSION);
            let retained: String = db
                .connection()
                .query_row(
                    "SELECT domain_id FROM completion_continuation_domain",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(retained, domain);
            let retained_attempt: (String, i64) = db
                .connection()
                .query_row(
                    "SELECT phase,revision FROM completion_continuation_attempt
                 WHERE attempt_id='older-attempt'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(retained_attempt, ("accepted".into(), 2));
            let grants: i64 = db
                .connection()
                .query_row(
                    "SELECT COUNT(*) FROM completion_native_grant_binding",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(grants, 0);
            assert!(db.connection().query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('completion_continuation_owner') WHERE name='kernel_root_id')",
                [], |row| row.get::<_, bool>(0),
            ).unwrap());
            drop(db);
            super::super::MailboxDb::open(&path).unwrap();
        }
    }

    #[test]
    fn v28_binding_cannot_be_reopened_as_a_synthetic_v27() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        drop(super::super::MailboxDb::open(&path).unwrap());
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 27).unwrap();
        drop(conn);
        let refused = super::super::MailboxDb::open(&path).err().unwrap();
        assert!(
            refused.contains("migration to version 28 failed"),
            "{refused}"
        );
        let conn = Connection::open(&path).unwrap();
        assert_eq!(sidecar_version(&conn).unwrap(), 27);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM completion_native_grant_binding",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn v28_binding_upgrades_to_v29_without_rewriting_accepted_work() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let conn = Connection::open(&path).unwrap();
        let steps: Vec<_> = SCHEMA_STEPS
            .iter()
            .filter(|step| step.target_version <= 28)
            .map(|step| MigrationStep {
                target_version: step.target_version,
                owner: step.owner,
                apply: step.apply,
            })
            .collect();
        apply_steps(&conn, &steps).unwrap();
        let domain: String = conn
            .query_row(
                "SELECT domain_id FROM completion_continuation_domain",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute("INSERT INTO completion_supervisor_authority(authority_id,domain_id,phase,created_by_generation,guardian_identity) VALUES('v28-supervisor',?1,'active','v28-owner','{}')", [&domain]).unwrap();
        conn.execute("INSERT INTO completion_continuation_owner(generation,domain_id,phase,guardian_identity,driver_identity,endpoint,supervisor_authority_id,kernel_root_id) VALUES('v28-owner',?1,'running','{}','{}','/v28','v28-supervisor','v28-root')", [&domain]).unwrap();
        conn.execute("INSERT INTO completion_continuation_attempt(attempt_id,domain_id,owner_generation,operation,request_sha256,phase,revision,result_path) VALUES('v28-attempt',?1,'v28-owner','transport','digest','accepted',2,'/v28/result')", [&domain]).unwrap();
        conn.execute("INSERT INTO completion_native_grant_binding(attempt_id,grant_id,protocol,accepted_revision,domain_id,kernel_root_id,supervisor_authority_id,owner_generation,guardian_identity,accepted_snapshot_sha256,custodian_request_sha256) VALUES('v28-attempt','v28-grant','native-continuation-v1',2,?1,'v28-root','v28-supervisor','v28-owner','{}',?2,?3)", params![domain,"a".repeat(64),"b".repeat(64)]).unwrap();
        conn.pragma_update(None, "user_version", 28).unwrap();
        drop(conn);
        let db = super::super::MailboxDb::open(&path).unwrap();
        assert_eq!(sidecar_version(db.connection()).unwrap(), 29);
        let retained: (String, i64, i64) = db.connection().query_row("SELECT a.phase,a.revision,a.integrated FROM completion_continuation_attempt a JOIN completion_native_grant_binding b ON b.attempt_id=a.attempt_id WHERE b.grant_id='v28-grant'", [], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(retained, ("accepted".into(), 2, 0));
        assert!(db.native_worker_attach("v28-attempt").unwrap().is_none());
        assert!(db.native_kernel_q("v28-attempt").unwrap().is_none());
    }

    #[test]
    fn v29_fingerprint_and_downgrade_refuse_missing_or_older_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        drop(super::super::MailboxDb::open(&path).unwrap());
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("DROP TRIGGER completion_native_kernel_q_exact;")
            .unwrap();
        drop(conn);
        let refused = super::super::MailboxDb::open(&path).err().unwrap();
        assert!(refused.contains("schema lineage differs"), "{refused}");
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 28).unwrap();
        drop(conn);
        let refused = super::super::MailboxDb::open(&path).err().unwrap();
        assert!(
            refused.contains("migration to version 29 failed"),
            "{refused}"
        );
        let conn = Connection::open(&path).unwrap();
        assert_eq!(sidecar_version(&conn).unwrap(), 28);
    }

    #[test]
    fn alternate_kernel_v24_gets_provenance_and_attempt_history_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let conn = Connection::open(&path).unwrap();
        let steps: Vec<_> = SCHEMA_STEPS
            .iter()
            .filter(|step| step.target_version <= 23)
            .map(|step| MigrationStep {
                target_version: step.target_version,
                owner: step.owner,
                apply: step.apply,
            })
            .collect();
        apply_steps(&conn, &steps).unwrap();
        let domain: String = conn
            .query_row(
                "SELECT domain_id FROM completion_continuation_domain",
                [],
                |row| row.get(0),
            )
            .unwrap();
        conn.execute_batch(include_str!("migrations/0027_kernel_root_owner.sql"))
            .unwrap();
        let root = Uuid::new_v4().to_string();
        let authority = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO completion_supervisor_authority(
                authority_id,domain_id,phase,created_by_generation,guardian_identity)
             VALUES(?1,?2,'active','old-generation','old-guardian')",
            params![authority, domain],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO completion_continuation_owner(
                generation,domain_id,phase,guardian_identity,driver_identity,
                endpoint,supervisor_authority_id,kernel_root_id)
             VALUES('old-generation',?1,'lost','old-guardian','old-driver',
                '/private/old-endpoint',?2,?3)",
            params![domain, authority, root],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 24).unwrap();
        drop(conn);

        let db = super::super::MailboxDb::open(&path).unwrap();
        assert_eq!(sidecar_version(db.connection()).unwrap(), CURRENT_VERSION);
        let retained: String = db
            .connection()
            .query_row(
                "SELECT domain_id FROM completion_continuation_domain",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, domain);
        let retained_root: String = db
            .connection()
            .query_row(
                "SELECT kernel_root_id FROM completion_continuation_owner WHERE generation='old-generation'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained_root, root);
        assert!(db.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('mailbox') WHERE name='completion_provenance')",
            [], |row| row.get::<_, bool>(0),
        ).unwrap());
        assert!(db.connection().query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('completion_continuation_attempt') WHERE name='association_completeness')",
            [], |row| row.get::<_, bool>(0),
        ).unwrap());
        drop(db);
        super::super::MailboxDb::open(&path).unwrap();
    }

    #[test]
    fn unrecognized_v24_does_not_advance_or_mutate() {
        let conn = Connection::open_in_memory().unwrap();
        let steps: Vec<_> = SCHEMA_STEPS
            .iter()
            .filter(|step| step.target_version <= 23)
            .map(|step| MigrationStep {
                target_version: step.target_version,
                owner: step.owner,
                apply: step.apply,
            })
            .collect();
        apply_steps(&conn, &steps).unwrap();
        conn.pragma_update(None, "user_version", 24).unwrap();
        let mut conn = conn;
        assert!(
            ensure(&mut conn)
                .unwrap_err()
                .contains("unrecognized PID mailbox v24 lineage")
        );
        assert_eq!(sidecar_version(&conn).unwrap(), 24);
    }

    #[test]
    fn v25_source_history_guard_is_part_of_schema_invariant() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pid-identity.db");
        let db = super::super::MailboxDb::open(&path).unwrap();
        for guard in [
            "completion_source_supervisor_authority_insert",
            "completion_source_supervisor_authority_immutable",
        ] {
            let sql: String = db
                .connection()
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?1",
                    [guard],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(sql.contains("attempt_association_history"));
        }
        db.connection()
            .execute_batch("DROP TRIGGER completion_source_supervisor_authority_immutable;")
            .unwrap();
        drop(db);
        assert!(
            super::super::MailboxDb::open(&path)
                .err()
                .unwrap()
                .contains("completion domain schema lineage differs")
        );
    }

    #[test]
    fn historical_source_status_is_a_point_read_on_large_attempt_history() {
        const ROWS: i64 = 20_000;
        const VM_BUDGET: usize = 5_000;
        let directory = tempfile::tempdir().unwrap();
        let mut db =
            super::super::MailboxDb::open(&directory.path().join("pid-identity.db")).unwrap();
        db.register_completion_event(super::super::CompletionEventRegistrationInput {
            event_id: "old-source-event",
            delivery_mode: "async",
            owner_session_id: Some("old-receiver"),
            owner_invocation_uuid: Some("old-owner"),
            state_dir: "/private/old",
            meta_path: "/private/old/meta",
            log_path: "/private/old/log",
            rc_path: "/private/old/rc",
        })
        .unwrap();
        let domain = db.completion_continuation_domain().unwrap().unwrap();
        let conn = db.connection();
        conn.execute(
            "INSERT INTO completion_continuation_source
             (registration_id,domain_id,source_id,event_id,registration_digest,binding,
              phase,snapshot_sha256,outcome_sha256,payload_sha256,payload_byte_len)
             VALUES('old-source',?1,'old-source','old-source-event','digest',x'00',
                    'accepted','snapshot','outcome','payload',1)",
            [&domain],
        )
        .unwrap();
        // Drained pre-v25 attempts can retain no claim or source-set link.
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        conn.execute_batch(&format!(
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<{ROWS})
             INSERT INTO completion_continuation_attempt
             (attempt_id,domain_id,owner_generation,operation,request_sha256,
              source_registration_id,session_id,claim_token,phase,result_path,
              integrated,drain_receipt)
             SELECT printf('old-attempt-%08d',x),'{domain}','old-owner','activation',
                    'digest','other-source','other-receiver',printf('old-claim-%08d',x),
                    'drained','/private/old/result',1,'old-receipt' FROM n;"
        ))
        .unwrap();
        let old_steps = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&old_steps);
        conn.progress_handler(
            1,
            Some(move || observed.fetch_add(1, Ordering::Relaxed) >= VM_BUDGET),
        )
        .unwrap();
        let old_query = conn.query_row(
            "SELECT EXISTS(
             SELECT 1 FROM completion_continuation_source source
             JOIN completion_continuation_attempt attempt ON attempt.domain_id=source.domain_id
             WHERE source.registration_id=?1 AND attempt.operation='activation'
               AND attempt.association_completeness='unknown'
               AND (attempt.source_registration_id=source.registration_id
                 OR EXISTS (SELECT 1 FROM completion_event_listener listener
                            WHERE listener.event_id=source.event_id
                              AND listener.session_id=attempt.session_id)
                 OR NOT EXISTS (SELECT 1 FROM completion_event_listener listener
                                WHERE listener.event_id=source.event_id)))",
            ["old-source"],
            |row| row.get::<_, bool>(0),
        );
        assert!(matches!(old_query,
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::OperationInterrupted));
        conn.progress_handler(0, None::<fn() -> bool>).unwrap();

        let new_steps = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&new_steps);
        conn.progress_handler(
            1,
            Some(move || observed.fetch_add(1, Ordering::Relaxed) >= VM_BUDGET),
        )
        .unwrap();
        assert_eq!(
            db.continuation_attempt_association_completeness("old-source")
                .unwrap(),
            "unknown"
        );
        let used = new_steps.load(Ordering::Relaxed);
        assert!(used < 500, "historical source status used {used} VM steps");
        conn.progress_handler(0, None::<fn() -> bool>).unwrap();
        let pending_steps = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&pending_steps);
        conn.progress_handler(
            1,
            Some(move || observed.fetch_add(1, Ordering::Relaxed) >= VM_BUDGET),
        )
        .unwrap();
        assert!(
            db.pending_continuation_attempt_ids("old-source")
                .unwrap()
                .is_empty()
        );
        let pending_used = pending_steps.load(Ordering::Relaxed);
        conn.progress_handler(0, None::<fn() -> bool>).unwrap();
        eprintln!(
            "{ROWS} historical attempts: old status interrupted after {} VM steps; point status used {used}",
            old_steps.load(Ordering::Relaxed)
        );
        eprintln!(
            "{ROWS} historical attempts: unresolved source status used {pending_used} VM steps"
        );
        conn.execute_batch(&format!(
            "INSERT INTO completion_continuation_attempt
             (attempt_id,domain_id,owner_generation,operation,request_sha256,
              source_registration_id,session_id,claim_token,phase,result_path,
              integrated,drain_receipt)
             VALUES('old-positive','{domain}','old-owner','activation','digest',
                    'old-source','old-receiver','deleted-claim','drained',
                    '/private/old/result',1,'old-receipt');"
        ))
        .unwrap();
        // A late positive with a deleted claim remains discoverable; an empty
        // bounded page cannot be mistaken for an exhaustive negative answer.
        let claim_retained: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_wake_claim WHERE claim_token='deleted-claim')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!claim_retained);
        conn.execute_batch(&format!(
            "INSERT INTO completion_continuation_attempt
             (attempt_id,domain_id,owner_generation,operation,request_sha256,
              source_registration_id,session_id,claim_token,phase,result_path,
              integrated,drain_receipt)
             VALUES('old-linked','{domain}','old-owner','activation','digest',
                    'old-source','old-receiver','also-deleted','drained',
                    '/private/old/result',1,'linked-receipt');
             INSERT INTO completion_continuation_attempt_source
             (attempt_id,registration_id,listener_revision)
             VALUES('old-linked','old-source',1);"
        ))
        .unwrap();
        let mut next = None;
        let mut found = Vec::new();
        let mut pages = 0;
        let mut max_steps = 0;
        let mut total_steps = 0;
        loop {
            let steps = Arc::new(AtomicUsize::new(0));
            let observed = Arc::clone(&steps);
            conn.progress_handler(
                1,
                Some(move || observed.fetch_add(1, Ordering::Relaxed) >= VM_BUDGET),
            )
            .unwrap();
            let (page, following) = db
                .completion_recovery_attempts("old-source", next.as_ref())
                .unwrap();
            conn.progress_handler(0, None::<fn() -> bool>).unwrap();
            let used = steps.load(Ordering::Relaxed);
            max_steps = max_steps.max(used);
            total_steps += used;
            assert!(used < VM_BUDGET, "recovery page used {used} VM steps");
            pages += 1;
            if pages == 2 {
                assert!(page.is_empty(), "early history page has no scalar match");
                assert!(
                    following.is_some(),
                    "an empty page must expose incompleteness"
                );
            }
            if pages == 1 {
                let mut wrong_source_cursor = following.clone().unwrap();
                wrong_source_cursor["registration_sha256"] = "another-source".into();
                assert!(
                    db.completion_recovery_attempts("old-source", Some(&wrong_source_cursor))
                        .is_err()
                );
            }
            found.extend(page);
            next = following;
            if next.is_none() {
                break;
            }
            assert!(pages < 200);
        }
        assert!(pages > 100, "late positive must require bounded paging");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0]["attempt_id"], "old-linked");
        assert_eq!(found[0]["drain_receipt"], "linked-receipt");
        assert_eq!(found[1]["attempt_id"], "old-positive");
        assert_eq!(found[1]["association_completeness"], "unknown");
        assert_eq!(found[1]["drain_receipt"], "old-receipt");
        assert_eq!(
            db.continuation_attempt_association_completeness("old-source")
                .unwrap(),
            "unknown"
        );
        eprintln!(
            "{ROWS} historical attempts: {pages} bounded recovery pages, max {max_steps} VM steps/page, {total_steps} VM steps total; late receipt retained after claim deletion"
        );
    }

    #[test]
    fn attempt_search_rejects_inter_page_insert_behind_lexical_cursor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pid-identity.db");
        let mut db = super::super::MailboxDb::open(&path).unwrap();
        db.register_completion_event(super::super::CompletionEventRegistrationInput {
            event_id: "one-event",
            delivery_mode: "async",
            owner_session_id: Some("receiver"),
            owner_invocation_uuid: Some("owner"),
            state_dir: "/private/source",
            meta_path: "/private/source/meta",
            log_path: "/private/source/log",
            rc_path: "/private/source/rc",
        })
        .unwrap();
        let domain = db.completion_continuation_domain().unwrap().unwrap();
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        writer
            .execute(
                "INSERT INTO completion_continuation_source
             (registration_id,domain_id,source_id,event_id,registration_digest,binding,
              phase,snapshot_sha256,outcome_sha256,payload_sha256,payload_byte_len)
             VALUES('one-source',?1,'one-source','one-event','digest',x'00',
                    'accepted','snapshot','outcome','payload',1)",
                [&domain],
            )
            .unwrap();
        writer
            .execute_batch(&format!(
                "BEGIN;
             WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<128)
             INSERT INTO completion_continuation_attempt
             (attempt_id,domain_id,owner_generation,operation,request_sha256,
              source_registration_id,phase,result_path,integrated,drain_receipt,
              association_completeness)
             SELECT printf('m%03d',x),'{domain}','owner','source_recovery','digest',
                    'one-source','drained','/private/receipt',1,'initial-receipt','known' FROM n;
             INSERT INTO completion_continuation_attempt_source
             (attempt_id,registration_id,listener_revision)
             SELECT attempt_id,'one-source',1 FROM completion_continuation_attempt;
             COMMIT;"
            ))
            .unwrap();

        let (first, cursor) = db.completion_recovery_attempts("one-source", None).unwrap();
        assert_eq!(first.len(), 128);
        let cursor = cursor.expect("129 linked rows require a second page");
        assert_eq!(cursor["after_attempt_id"], "m127");

        // A later committed attempt is lexically behind the last observed ID.
        // Its exact link and physical receipt commit in the same transaction.
        writer
            .execute_batch(&format!(
                "BEGIN;
             INSERT INTO completion_continuation_attempt
             (attempt_id,domain_id,owner_generation,operation,request_sha256,
              source_registration_id,phase,result_path,integrated,drain_receipt,
              association_completeness)
             VALUES('a000','{domain}','owner','source_recovery','digest',
                    'one-source','drained','/private/new-receipt',1,'late-receipt','known');
             INSERT INTO completion_continuation_attempt_source
             (attempt_id,registration_id,listener_revision)
             VALUES('a000','one-source',1);
             COMMIT;"
            ))
            .unwrap();
        let stale = db.completion_recovery_attempts("one-source", Some(&cursor));
        assert!(stale.unwrap_err().contains("attempt history changed"));

        let mut cursor = None;
        let mut found = Vec::new();
        loop {
            let (page, next) = db
                .completion_recovery_attempts("one-source", cursor.as_ref())
                .unwrap();
            found.extend(page);
            cursor = next;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(found.len(), 130);
        assert_eq!(found[0]["attempt_id"], "a000");
        assert_eq!(found[0]["drain_receipt"], "late-receipt");
        assert_eq!(found[0]["association_completeness"], "known");
    }

    #[test]
    fn v26_attempt_search_generation_migration_does_not_scan_history() {
        const ROWS: i64 = 20_000;
        const VM_BUDGET: usize = 5_000;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE completion_continuation_attempt(attempt_id TEXT PRIMARY KEY);
             CREATE TABLE completion_continuation_attempt_source(
                 attempt_id TEXT, registration_id TEXT);
             WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<{ROWS})
             INSERT INTO completion_continuation_attempt(attempt_id)
             SELECT printf('attempt-%08d',x) FROM n;"
        ))
        .unwrap();
        let steps = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&steps);
        conn.progress_handler(
            1,
            Some(move || observed.fetch_add(1, Ordering::Relaxed) >= VM_BUDGET),
        )
        .unwrap();
        migrate_completion_attempt_search_generation(&conn).unwrap();
        let used = steps.load(Ordering::Relaxed);
        assert!(used < VM_BUDGET, "v26 schema step used {used} VM steps");
        conn.progress_handler(0, None::<fn() -> bool>).unwrap();
        let generation: i64 = conn
            .query_row(
                "SELECT generation FROM completion_continuation_attempt_search_generation",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(generation, 0);
        eprintln!("20,000 historical attempts: v26 step used {used} VM steps");
    }

    #[test]
    fn v25_association_schema_step_does_not_scan_large_attempt_history() {
        const ROWS: i64 = 20_000;
        // The current v29 fingerprint includes two more bounded ledgers and
        // their guards. This remains below a fixed ceiling independent of rows.
        const VM_BUDGET: usize = 8_000;
        fn history() -> Connection {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(&format!(
                "CREATE TABLE completion_continuation_attempt(attempt_id TEXT PRIMARY KEY);
                 CREATE TABLE completion_continuation_source(
                     registration_id TEXT PRIMARY KEY, domain_id TEXT,
                     supervisor_authority_id TEXT);
                 CREATE TABLE completion_supervisor_authority(authority_id TEXT,domain_id TEXT);
                 CREATE TRIGGER completion_source_supervisor_authority_insert
                     BEFORE INSERT ON completion_continuation_source BEGIN SELECT 1; END;
                 CREATE TRIGGER completion_source_supervisor_authority_immutable
                     BEFORE UPDATE ON completion_continuation_source BEGIN SELECT 1; END;
                 WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<{ROWS})
                 INSERT INTO completion_continuation_attempt(attempt_id) SELECT printf('attempt-%08d',x) FROM n;
                 WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<{ROWS})
                 INSERT INTO completion_continuation_source(registration_id) SELECT printf('source-%08d',x) FROM n;"
            )).unwrap();
            conn
        }
        fn budget(conn: &Connection) -> Arc<AtomicUsize> {
            let steps = Arc::new(AtomicUsize::new(0));
            let observed = Arc::clone(&steps);
            conn.progress_handler(
                1,
                Some(move || observed.fetch_add(1, Ordering::Relaxed) >= VM_BUDGET),
            )
            .unwrap();
            steps
        }
        let old = history();
        let old_steps = budget(&old);
        let old_result = old.execute_batch(
            "CREATE INDEX hypothetical_historical_backfill ON completion_continuation_attempt(attempt_id);"
        );
        assert!(matches!(old_result,
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::OperationInterrupted));
        assert!(old_steps.load(Ordering::Relaxed) > VM_BUDGET);

        let corrected = history();
        let corrected_steps = budget(&corrected);
        migrate_completion_attempt_sources(&corrected).unwrap();
        let used = corrected_steps.load(Ordering::Relaxed);
        assert!(used < VM_BUDGET, "v25 schema step used {used} VM steps");
        corrected.progress_handler(0, None::<fn() -> bool>).unwrap();
        let (count, unknown): (i64, i64) = corrected.query_row(
            "SELECT COUNT(*), SUM(association_completeness='unknown') FROM completion_continuation_attempt",
            [], |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!((count, unknown), (ROWS, ROWS));
        let (sources, unknown_sources): (i64, i64) = corrected
            .query_row(
                "SELECT COUNT(*), SUM(attempt_association_history='unknown')
             FROM completion_continuation_source",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((sources, unknown_sources), (ROWS, ROWS));
        eprintln!(
            "20,000 historical attempts: indexing history interrupted after {} VM steps; v25 step used {used}",
            old_steps.load(Ordering::Relaxed)
        );

        let full = Connection::open_in_memory().unwrap();
        create_fresh_schema(&full).unwrap();
        full.pragma_update(None, "user_version", CURRENT_VERSION)
            .unwrap();
        let domain: String = full
            .query_row(
                "SELECT domain_id FROM completion_continuation_domain",
                [],
                |row| row.get(0),
            )
            .unwrap();
        full.execute(
            "INSERT INTO completion_supervisor_authority
             (authority_id,domain_id,phase,created_by_generation,guardian_identity)
             VALUES('history-auth',?1,'active','history-owner','{}')",
            [&domain],
        )
        .unwrap();
        full.execute(
            "INSERT INTO completion_continuation_owner
             (generation,domain_id,phase,guardian_identity,driver_identity,endpoint,supervisor_authority_id)
             VALUES('history-owner',?1,'lost','{}','{}','/private/owner','history-auth')",
            [&domain],
        ).unwrap();
        full.execute_batch(&format!(
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<{ROWS})
             INSERT INTO completion_continuation_attempt
             (attempt_id,domain_id,owner_generation,operation,request_sha256,
              phase,result_path,integrated,drain_receipt)
             SELECT printf('old-%08d',x),'{domain}','history-owner','source_recovery',
                    'digest','drained','/private/old-result',1,'old-receipt' FROM n;"
        ))
        .unwrap();
        let open_steps = budget(&full);
        assert!(observe_valid_current(&full).unwrap());
        let open_used = open_steps.load(Ordering::Relaxed);
        assert!(
            open_used < VM_BUDGET,
            "current-schema ordinary open check used {open_used} VM steps"
        );
        eprintln!("20,000-row v25 ordinary-open schema check used {open_used} VM steps");
    }

    #[test]
    fn v23_provenance_schema_step_stays_below_large_mailbox_vm_budget() {
        const ROWS: i64 = 20_000;
        const VM_BUDGET: usize = 5_000;

        fn populated_v23_mailbox(rows: i64) -> Connection {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(&format!(
                "CREATE TABLE mailbox(seq INTEGER PRIMARY KEY, payload_json TEXT NOT NULL);
                 WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<{rows})
                 INSERT INTO mailbox(seq,payload_json) SELECT x,'{{}}' FROM n;"
            ))
            .unwrap();
            conn
        }

        fn limit_vm_steps(conn: &Connection, budget: usize) -> Arc<AtomicUsize> {
            let steps = Arc::new(AtomicUsize::new(0));
            let observed = Arc::clone(&steps);
            conn.progress_handler(
                1,
                Some(move || observed.fetch_add(1, Ordering::Relaxed) >= budget),
            )
            .unwrap();
            steps
        }

        // The former CHECK-bearing statement is the discriminating control:
        // SQLite must visit preexisting rows and the same budget interrupts it.
        let old = populated_v23_mailbox(ROWS);
        let old_steps = limit_vm_steps(&old, VM_BUDGET);
        let old_result = old.execute_batch(
            "ALTER TABLE mailbox ADD COLUMN completion_provenance TEXT NOT NULL
             DEFAULT 'unclassified'
             CHECK(completion_provenance IN ('unclassified','legacy','v2'));",
        );
        assert!(
            matches!(
                old_result,
                Err(rusqlite::Error::SqliteFailure(error, _))
                    if error.code == rusqlite::ErrorCode::OperationInterrupted
            ),
            "CHECK-bearing ADD COLUMN was not interrupted by its row scan"
        );
        assert!(old_steps.load(Ordering::Relaxed) > VM_BUDGET);
        old.progress_handler(0, None::<fn() -> bool>).unwrap();
        let old_column_present: bool = old
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('mailbox')
                 WHERE name='completion_provenance')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !old_column_present,
            "interrupted CHECK addition changed the schema"
        );

        let corrected = populated_v23_mailbox(ROWS);
        let corrected_steps = limit_vm_steps(&corrected, VM_BUDGET);
        migrate_completion_mailbox_provenance(&corrected).unwrap();
        let used = corrected_steps.load(Ordering::Relaxed);
        assert!(used < VM_BUDGET, "v24 schema step used {used} VM steps");
        eprintln!(
            "20,000-row v23 mailbox: former CHECK ADD COLUMN interrupted after {} VM steps; corrected v24 step used {used} VM steps",
            old_steps.load(Ordering::Relaxed)
        );
        corrected.progress_handler(0, None::<fn() -> bool>).unwrap();
        let (count, first, last): (i64, String, String) = corrected
            .query_row(
                "SELECT COUNT(*),
                    (SELECT completion_provenance FROM mailbox WHERE seq=1),
                    (SELECT completion_provenance FROM mailbox WHERE seq=?1)
                 FROM mailbox",
                [ROWS],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (count, first.as_str(), last.as_str()),
            (ROWS, "unclassified", "unclassified")
        );
    }

    #[test]
    fn fresh_and_same_column_upgrade_enforce_provenance_on_direct_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pid-identity.db");
        let mailbox = super::super::MailboxDb::open(&path).unwrap();
        let insert = "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
            state_dir,meta_path,log_path,rc_path,rc,completion_provenance)
            VALUES('session','input',?1,'{}','2026-09-23T00:00:00Z',
                '/state','/meta','/log','/rc',0,?2)";
        assert!(
            mailbox
                .connection()
                .execute(insert, params!["bad-insert", "unknown"])
                .is_err()
        );
        mailbox
            .connection()
            .execute(
                "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
                    state_dir,meta_path,log_path,rc_path,rc)
                 VALUES('session','input','default','{}','2026-09-23T00:00:00Z',
                    '/state','/meta','/log','/rc',0)",
                [],
            )
            .unwrap();
        let initial: String = mailbox
            .connection()
            .query_row(
                "SELECT completion_provenance FROM mailbox WHERE handle='default'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(initial, "unclassified");
        assert!(
            mailbox
                .connection()
                .execute(
                    "UPDATE mailbox SET completion_provenance='unknown' WHERE handle='default'",
                    [],
                )
                .is_err()
        );
        mailbox
            .connection()
            .execute(
                "UPDATE mailbox SET completion_provenance='legacy' WHERE handle='default'",
                [],
            )
            .unwrap();
        assert!(
            mailbox
                .connection()
                .execute(
                    "UPDATE mailbox SET completion_provenance='v2' WHERE handle='default'",
                    [],
                )
                .is_err()
        );
        mailbox
            .connection()
            .execute(
                "UPDATE mailbox SET payload_json='[]' WHERE handle='default'",
                [],
            )
            .unwrap();
        drop(mailbox);

        // A fixture may carry the v24 column and triggers while its recorded
        // version is 23. Reapplying the step must keep classified rows intact.
        let fixture = Connection::open(&path).unwrap();
        fixture
            .execute_batch(
                "DROP TABLE completion_native_kernel_q;
                DROP TABLE completion_native_worker_attach;
                DROP TRIGGER completion_native_grant_no_legacy_terminal;
                DROP TABLE completion_native_grant_binding;",
            )
            .unwrap();
        fixture.pragma_update(None, "user_version", 23).unwrap();
        drop(fixture);
        let reopened = super::super::MailboxDb::open(&path).unwrap();
        let provenance: String = reopened
            .connection()
            .query_row(
                "SELECT completion_provenance FROM mailbox WHERE handle='default'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(provenance, "legacy");
        assert!(
            reopened
                .connection()
                .execute(insert, params!["bad-again", "unknown"])
                .is_err()
        );
    }

    #[test]
    fn v23_upgrade_defers_pending_history_and_reconciles_exact_rows() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pid-identity.db");
        let mut mailbox = super::super::MailboxDb::open(&path).unwrap();
        mailbox
            .register_completion_event(super::super::CompletionEventRegistrationInput {
                event_id: "old-v2",
                delivery_mode: "async",
                owner_session_id: Some("receiver"),
                owner_invocation_uuid: Some("listener"),
                state_dir: "/private/source",
                meta_path: "/private/source/meta.json",
                log_path: "/private/source/log",
                rc_path: "/private/source/rc",
            })
            .unwrap();
        let domain = mailbox.completion_continuation_domain().unwrap().unwrap();
        for (handle, payload) in [
            (
                "old-v2",
                r#"{"completion_protocol":"completion-continuation-v2","source":true}"#,
            ),
            (
                "old-v2-lost-source",
                r#"{"completion_protocol":"completion-continuation-v2"}"#,
            ),
            ("old-null-marker", r#"{"completion_protocol":null}"#),
            (
                "old-legacy",
                r#"{"schema_version":2,"kind":"agent_bash_complete","meta":{}}"#,
            ),
            (
                "old-markerless-v2",
                r#"{"schema_version":2,"kind":"agent_bash_complete","snapshot":{},"outcome":{}}"#,
            ),
        ] {
            mailbox
                .enqueue_agent_bash_complete(&super::super::AgentBashCompleteEnqueue {
                    session_id: handle,
                    handle,
                    payload_json: payload,
                    owner_invocation_uuid: None,
                    matched_os_pid: None,
                    matched_os_boot_id: None,
                    matched_os_pid_starttime_ticks: None,
                    matched_chain_index: None,
                    state_dir: "/private/source",
                    meta_path: "/private/source/meta.json",
                    log_path: "/private/source/log",
                    rc_path: "/private/source/rc",
                    rc: 0,
                })
                .unwrap();
        }
        let direct_markerless: String = mailbox
            .connection()
            .query_row(
                "SELECT completion_provenance FROM mailbox WHERE handle='old-markerless-v2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(direct_markerless, "unclassified");
        let source_payload_path: String = mailbox
            .connection()
            .query_row(
                "SELECT payload_file_path FROM mailbox WHERE handle='old-v2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        std::fs::remove_file(source_payload_path).unwrap();
        mailbox
            .connection()
            .execute(
                "INSERT INTO completion_continuation_source
                 (registration_id,domain_id,source_id,event_id,registration_digest,binding)
             VALUES('old-v2',?1,'old-v2','old-v2','digest',x'00')",
                [domain],
            )
            .unwrap();
        mailbox
            .connection()
            .execute_batch(
                "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<256)
             INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
                 state_dir,meta_path,log_path,rc_path,rc)
             SELECT 'backlog','agent_bash_complete',printf('pending-%d',x),
                 '{\"schema_version\":1}','2026-09-23T00:00:00Z',
                 '/private/source','/private/source/meta.json',
                 '/private/source/log','/private/source/rc',0 FROM n;",
            )
            .unwrap();
        mailbox
            .connection()
            .execute(
                "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
                 state_dir,meta_path,log_path,rc_path,rc)
             VALUES('old-ambiguous','agent_bash_complete','old-ambiguous',
                 '{\"schema_version\":1}','2026-09-23T00:00:00Z',
                 '/private/source','/private/source/meta.json',
                 '/private/source/log','/private/source/rc',0)",
                [],
            )
            .unwrap();
        mailbox
            .connection()
            .execute_batch(
                "DROP TABLE completion_native_kernel_q;
             DROP TABLE completion_native_worker_attach;
             DROP TRIGGER completion_native_grant_no_legacy_terminal;
             DROP TABLE completion_native_grant_binding;
             DROP TRIGGER mailbox_completion_provenance_insert_valid;
             DROP TRIGGER mailbox_completion_provenance_update_valid;
             DROP TRIGGER mailbox_completion_provenance_immutable;
             ALTER TABLE mailbox DROP COLUMN completion_provenance;
             PRAGMA user_version = 23;",
            )
            .unwrap();
        drop(mailbox);

        let mut reopened = super::super::MailboxDb::open(&path).unwrap();
        assert_eq!(
            sidecar_version(reopened.connection()).unwrap(),
            CURRENT_VERSION
        );
        let pending_backlog: i64 = reopened
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM mailbox WHERE session_id='backlog'
             AND completion_provenance='unclassified'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_backlog, 256);
        for handle in [
            "old-v2",
            "old-v2-lost-source",
            "old-null-marker",
            "old-legacy",
            "old-markerless-v2",
            "old-ambiguous",
        ] {
            let provenance: String = reopened
                .connection()
                .query_row(
                    "SELECT completion_provenance FROM mailbox WHERE handle=?1",
                    [handle],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                provenance, "unclassified",
                "upgrade must defer all pending history"
            );
        }
        for (handle, expected) in [
            ("old-v2", "v2"),
            ("old-v2-lost-source", "v2"),
            ("old-null-marker", "unclassified"),
            ("old-legacy", "legacy"),
            ("old-markerless-v2", "unclassified"),
            ("old-ambiguous", "unclassified"),
        ] {
            super::super::completion_continuation::classify_one_pending_completion(
                &mut reopened.conn,
                handle,
                None,
            )
            .unwrap();
            let provenance: String = reopened
                .connection()
                .query_row(
                    "SELECT completion_provenance FROM mailbox WHERE handle=?1",
                    [handle],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(provenance, expected, "incorrect backfill for {handle}");
        }
        assert!(
            reopened
                .connection()
                .execute(
                    "UPDATE mailbox SET completion_provenance='legacy' WHERE handle='old-v2'",
                    [],
                )
                .is_err(),
            "classified v2 provenance must be immutable"
        );
        reopened
            .connection()
            .execute_batch("DROP TRIGGER mailbox_completion_provenance_immutable;")
            .unwrap();
        drop(reopened);
        assert!(
            super::super::MailboxDb::open(&path)
                .err()
                .unwrap()
                .contains("completion domain schema lineage differs")
        );
    }

    #[test]
    fn populated_v19_fixture_migrates_without_newer_schema_objects() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut mailbox = super::super::MailboxDb::open(&path).unwrap();
        mailbox
            .enqueue_agent_bash_complete(&super::super::AgentBashCompleteEnqueue {
                session_id: "historical-session",
                handle: "historical-handle",
                payload_json: "{}",
                owner_invocation_uuid: Some("historical-owner"),
                matched_os_pid: Some(1),
                matched_os_boot_id: Some("boot"),
                matched_os_pid_starttime_ticks: Some(1),
                matched_chain_index: Some(0),
                state_dir: "/private/state",
                meta_path: "/private/meta",
                log_path: "/private/log",
                rc_path: "/private/rc",
                rc: 0,
            })
            .unwrap();
        mailbox
            .register_completion_event(super::super::CompletionEventRegistrationInput {
                event_id: "terminal-history-event",
                delivery_mode: "async",
                owner_session_id: Some("terminal-history-session"),
                owner_invocation_uuid: Some("terminal-history-owner"),
                state_dir: "/private/state",
                meta_path: "/private/meta",
                log_path: "/private/log",
                rc_path: "/private/rc",
            })
            .unwrap();
        let terminal_seq = mailbox
            .trigger_completion_event(super::super::CompletionEventTriggerInput {
                event_id: "terminal-history-event",
                payload_json: "{}",
                state_dir: "/private/state",
                meta_path: "/private/meta",
                log_path: "/private/log",
                rc_path: "/private/rc",
                rc: 0,
            })
            .unwrap()
            .mailbox_rows[0]
            .seq;
        drop(mailbox);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "DROP VIEW mailbox_retained_delivery_finalizers;
                 DROP INDEX mailbox_completed_turn_pins_attempt;
                 DROP TABLE mailbox_completed_turn_pins;
                 DROP TABLE mailbox_completed_turn_tails;",
            )
            .unwrap();
        remove_completion_recovery_working_set_for_legacy_fixture(&connection);
        connection
            .execute(
                "UPDATE mailbox SET delivery_error='mailbox_ingress_expired' WHERE seq=?1",
                [terminal_seq],
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 19).unwrap();
        drop(connection);
        let migrated = super::super::MailboxDb::open(&path).unwrap();
        assert_eq!(
            migrated
                .list_mailbox_all("historical-session")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            sidecar_version(migrated.connection()).unwrap(),
            CURRENT_VERSION
        );
        let terminal_listener: (bool, bool) = migrated
            .connection()
            .query_row(
                "SELECT active,retirement_pending FROM completion_event_listener
                 WHERE event_id='terminal-history-event'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(terminal_listener, (false, false));
        for object in [
            "mailbox_completed_turn_pins",
            "mailbox_completed_turn_tails",
        ] {
            let present: bool = migrated
                .connection()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name=?1)",
                    [object],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(present, "missing migrated object {object}");
        }
    }

    #[test]
    fn stale_reader_validates_committed_schema_with_unrelated_writer_held() {
        stale_reader_case("current");
    }

    #[test]
    fn stale_reader_rejects_uncommitted_corrupt_and_unsupported_schema() {
        for case in ["uncommitted", "corrupt", "unsupported"] {
            stale_reader_case(case);
        }
    }

    fn stale_reader_case(case: &str) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let reader_path = path.clone();
        let (observed_tx, observed_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            STALE_OBSERVATION_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    observed_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                }));
            });
            // Real opener retains its ordinary shared namespace fence.
            let result = super::super::MailboxDb::open(&reader_path);
            done_tx.send(result.map(|_| ())).unwrap();
        });
        observed_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        let mut writer = Connection::open(&path).unwrap();
        if case != "uncommitted" {
            let migrated = super::super::MailboxDb::open(&path).unwrap();
            drop(migrated);
            match case {
                "corrupt" => writer
                    .execute_batch("DROP TABLE completion_continuation_notification")
                    .unwrap(),
                "unsupported" => writer
                    .pragma_update(None, "user_version", CURRENT_VERSION + 1)
                    .unwrap(),
                _ => (),
            }
        }
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        if case == "uncommitted" {
            create_fresh_schema(&tx).unwrap();
            tx.pragma_update(None, "user_version", CURRENT_VERSION)
                .unwrap();
        }
        resume_tx.send(()).unwrap();
        if case == "uncommitted" {
            // A live writer with invisible uncommitted schema is not proof of
            // deadlock. The opener remains pending rather than fabricating a
            // five-second startup failure; rollback releases SQLite custody.
            assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err());
            tx.rollback().unwrap();
            done_rx
                .recv_timeout(Duration::from_secs(20))
                .unwrap()
                .unwrap();
            reader.join().unwrap();
            return;
        }
        // Committed unsupported/corrupt schema is readable evidence and can be
        // rejected without waiting for the unrelated writer to release.
        let result = done_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        match case {
            "current" => result.unwrap(),
            "unsupported" => assert!(
                result
                    .unwrap_err()
                    .contains("Unsupported PID mailbox sidecar schema version")
            ),
            "corrupt" => assert!(result.is_err(), "invalid fingerprint was accepted"),
            _ => unreachable!(),
        }
        tx.rollback().unwrap();
        reader.join().unwrap();
    }
}
