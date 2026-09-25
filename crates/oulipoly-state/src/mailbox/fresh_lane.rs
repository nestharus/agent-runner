//! Fresh, independent v30 storage. Publication never reads or copies a v29 file.
//! A failed prepublication build leaves only an inert hidden staging directory;
//! a lost publication reply reopens the same immutable identity.
use super::broker_authority::{
    BoundStateSource, activate_with_owner, require_root_owned_ancestors,
};
use super::*;
use crate::StateDb;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

include!("fresh_recipient.rs");
include!("fresh_bash_child.rs");
include!("fresh_bash_source.rs");
include!("fresh_bash_notify.rs");
include!("fresh_bash_listener.rs");
include!("fresh_root_terminal.rs");
include!("fresh_bash_sync_publication.rs");

const LANE_DIRECTORY: &str = "v30";
const FRESH_PROVIDER_DIRECTORY: &str = "fresh-provider";
const LANE_PROTOCOL: &str = "fresh-v30-lane-v1";
const FRESH_SCHEMA: &str = include_str!("migrations/0030_fresh_lane.sql");
const FRESH_STATE_SCHEMA: &str = include_str!("migrations/0030_fresh_state_identity.sql");
const FRESH_CHILD_REQUEST_SCHEMA: &str = include_str!("migrations/0030_fresh_child_request.sql");
const FRESH_HANDOFF_SCHEMA: &str = include_str!("migrations/0031_fresh_released_handoff.sql");
const FRESH_ROOT_EFFECT_SCHEMA: &str = include_str!("migrations/0032_fresh_root_effect.sql");
const FRESH_BASH_CHILD_SCHEMA: &str = include_str!("migrations/0033_fresh_bash_child.sql");
const FRESH_BASH_SOURCE_SCHEMA: &str = include_str!("migrations/0035_fresh_bash_source.sql");
const FRESH_BASH_NOTIFY_SCHEMA: &str = include_str!("migrations/0036_fresh_bash_notify.sql");
const FRESH_BASH_LISTENER_SCHEMA: &str = include_str!("migrations/0037_fresh_bash_listener.sql");
const FRESH_ROOT_TERMINAL_SCHEMA: &str = include_str!("migrations/0038_fresh_root_terminal.sql");
const FRESH_BASH_SYNC_PUBLICATION_SCHEMA: &str =
    include_str!("migrations/0040_fresh_bash_sync_publication.sql");
const FRESH_NORMAL_WORK_SCHEMA: &str = include_str!("migrations/0034_fresh_normal_work.sql");
const FRESH_RECIPIENT_SCHEMA: &str = include_str!("migrations/0030_fresh_recipient.sql");
const FRESH_RECIPIENT_ACK_SCHEMA: &str = include_str!("migrations/0039_fresh_recipient_ack.sql");
const FRESH_RECIPIENT_STATE_SCHEMA: &str =
    include_str!("migrations/0030_fresh_recipient_state.sql");

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FreshV30LaneIdentity {
    pub lane_id: String,
    pub domain_id: String,
    pub source_generation: String,
}

/// The old release authority records the exact root entry it has already
/// placed. This describes the root invocation; a later Bash child needs its
/// own separately admitted handle and registration.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    content = "arguments",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FreshRootWorkIntent {
    CliHelp(Vec<String>),
    CliDiagnostics(Vec<String>),
    NormalCli(Vec<String>),
    PrivateProbe(Vec<String>),
}

impl FreshRootWorkIntent {
    fn kind(&self) -> &'static str {
        match self {
            Self::CliHelp(_) => "cli_help",
            Self::CliDiagnostics(_) => "cli_diagnostics",
            Self::NormalCli(_) => "normal_cli",
            Self::PrivateProbe(_) => "private_probe",
        }
    }

    fn valid(&self) -> bool {
        match self {
            Self::CliHelp(args) => {
                matches!(args.as_slice(), [only] if only == "--help" || only == "-h")
            }
            Self::CliDiagnostics(args) => args.first().is_some_and(|first| first == "diagnostics"),
            Self::NormalCli(args) => normal_root_arguments(args),
            Self::PrivateProbe(args) => {
                cfg!(feature = "age319-private-broker-fixture")
                    && matches!(args.as_slice(), [only] if only == "__age319-private-root-handoff-v1")
            }
        }
    }

    pub fn arguments(&self) -> &[String] {
        match self {
            Self::CliHelp(args)
            | Self::CliDiagnostics(args)
            | Self::NormalCli(args)
            | Self::PrivateProbe(args) => args,
        }
    }

    /// Offline CLI roots return through the broker result transition. Normal
    /// provider/recovery roots still need their own launch and result authority.
    pub fn returnable_entry(&self) -> bool {
        self.valid()
            && match self {
                Self::CliHelp(_) | Self::CliDiagnostics(_) => true,
                Self::NormalCli(_) => false,
                Self::PrivateProbe(_) => cfg!(feature = "age319-private-broker-fixture"),
            }
    }
}

/// Conservative syntax for a held normal root. This classification does not
/// authorize CLI dispatch or provider spawn. The Runner's Clap parser remains
/// authoritative once a separate provider K/Q route exists.
pub fn normal_root_arguments(args: &[String]) -> bool {
    match args {
        [flag, model, ..]
            if (flag == "--model" || flag == "-m")
                && !model.is_empty()
                && !model.starts_with('-') =>
        {
            true
        }
        [flag, session, ..]
            if flag == "--resume" && !session.is_empty() && !session.starts_with('-') =>
        {
            true
        }
        [first, ..] if first.starts_with("--model=") && first.len() > "--model=".len() => true,
        [first, ..] if first.starts_with("--resume=") && first.len() > "--resume=".len() => true,
        [first, ..]
            if matches!(
                first.as_str(),
                "--new" | "-n" | "--file" | "-f" | "--agent-file" | "-a"
            ) =>
        {
            true
        }
        [first, ..] if !first.starts_with('-') => true,
        _ => false,
    }
}

/// A durable, exact normal-root boundary. `held` carries no provider launch
/// authority: there is deliberately no started/success transition here.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshNormalWorkPreparation {
    pub handoff_id: String,
    pub invocation_uuid: String,
    pub session_id: String,
    pub actor: FreshRecipientIdentity,
    pub intent: FreshRootWorkIntent,
    pub state: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshRootEffectState {
    Started,
    ReturnedSuccess,
    ReturnedFailure,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshRootEffect {
    pub handoff_id: String,
    pub invocation_uuid: String,
    pub session_id: String,
    pub actor: FreshRecipientIdentity,
    pub intent: FreshRootWorkIntent,
    pub state: FreshRootEffectState,
}

/// Minted by the old release authority while its physical gate is retained.
/// U/D create a root invocation and session, not a Bash descendant.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshReleasedHandoff {
    pub handoff_id: String,
    pub d_key: String,
    pub invocation_uuid: String,
    pub root_work_intent: FreshRootWorkIntent,
    pub broker_incarnation: String,
    pub runner_image_device: u64,
    pub runner_image_inode: u64,
    pub old_release: super::BrokerReleaseEvidence,
    pub fresh_lane: FreshV30LaneIdentity,
    pub registration_authority: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FreshV30Session {
    pub lane_id: String,
    pub source_generation: String,
    pub session_id: String,
    pub request_id: String,
    pub allocation_id: String,
}

/// Read-only join of a consumed source with this lane's original session.
/// This is a prerequisite for a future broker admission writer, not an
/// acceptance or release grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshV30SourceProvenance {
    pub lane_id: String,
    pub allocation_id: String,
    pub session_id: String,
    pub source_generation: String,
    pub state_admission_id: String,
    pub registration_id: String,
    pub registration_digest: String,
    pub root_id: String,
    pub owner_generation: String,
}

/// A broker-retained connection; no caller supplies a ledger path to an
/// operation. The public path argument is an installation root for bootstrap
/// and private fixtures only, never a wire selector.
pub struct FreshV30Lane {
    sidecar: BrokerSidecar,
    identity: FreshV30LaneIdentity,
    state_path: PathBuf,
}

impl FreshV30Lane {
    /// Reserved namespace for broker-minted session IDs. Legacy unqualified
    /// commands must refuse it even if an old row has the same spelling.
    pub fn is_reserved_session_id(session_id: &str) -> bool {
        let mut parts = session_id.split(':');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some("v30"), Some(lane), Some(session), None) => [lane, session]
                .into_iter()
                .all(|part| Uuid::parse_str(part).is_ok_and(|id| id.to_string() == part)),
            _ => false,
        }
    }

    pub fn initialize_at(broker_root: &Path) -> Result<FreshV30LaneIdentity, String> {
        require_root()?;
        // The installed broker is a dedicated root process. SQLite may create
        // WAL/SHM while opening, so its process umask must protect those too.
        unsafe { libc::umask(0o077) };
        require_broker_root(broker_root)?;
        let final_root = broker_root.join(LANE_DIRECTORY);
        if final_root.exists() {
            return Ok(Self::open_at(broker_root)?.identity);
        }
        let stage_root = broker_root.join(format!(".v30-fresh-{}", Uuid::new_v4().simple()));
        fs::create_dir(&stage_root).map_err(|e| e.to_string())?;
        fs::set_permissions(&stage_root, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        let sidecar_root = stage_root.join("sidecar");
        fs::create_dir(&sidecar_root).map_err(|e| e.to_string())?;
        fs::set_permissions(&sidecar_root, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        let provider_root = stage_root.join(FRESH_PROVIDER_DIRECTORY);
        fs::create_dir(&provider_root).map_err(|e| e.to_string())?;
        fs::set_permissions(&provider_root, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        let state_path = stage_root.join("state.db");
        let mailbox_path = sidecar_root.join("pid-identity.db");
        // Both constructors start from absent files. No old writer or WAL is
        // consulted, and no quiescence proof is relevant to this lane.
        drop(StateDb::open_historical_without_observation(&state_path)?);
        let mailbox = MailboxDb::open_historical_without_observation(&mailbox_path)?;
        let domain_id = mailbox
            .completion_continuation_domain()?
            .ok_or("fresh mailbox domain absent")?;
        drop(mailbox);
        let source_generation = activate_with_owner(&mailbox_path, 0, &stage_root)?;
        let lane_id = Uuid::new_v4().to_string();
        let identity = FreshV30LaneIdentity {
            lane_id,
            domain_id,
            source_generation,
        };
        let state_meta = fs::symlink_metadata(&state_path).map_err(|e| e.to_string())?;
        if !state_meta.is_file()
            || state_meta.is_symlink()
            || state_meta.uid() != 0
            || state_meta.nlink() != 1
        {
            return Err("fresh State file has wrong physical owner".into());
        }
        let sidecar = BrokerSidecar::open_existing(&mailbox_path, &stage_root)?;
        sidecar
            .mailbox()
            .conn
            .execute_batch(&format!(
                "{FRESH_SCHEMA}\n{FRESH_RECIPIENT_SCHEMA}\n{FRESH_RECIPIENT_ACK_SCHEMA}"
            ))
            .map_err(|e| e.to_string())?;
        sidecar
            .mailbox()
            .conn
            .execute(
                "INSERT INTO fresh_lane_identity VALUES(1,?1,?2,?3,?4,?5,?6)",
                params![
                    LANE_PROTOCOL,
                    identity.lane_id,
                    identity.domain_id,
                    identity.source_generation,
                    state_meta.dev() as i64,
                    state_meta.ino() as i64
                ],
            )
            .map_err(|e| e.to_string())?;
        drop(sidecar);
        let state_conn = Connection::open(&state_path).map_err(|e| e.to_string())?;
        state_conn
            .execute_batch(&format!(
                "{FRESH_STATE_SCHEMA}\n{FRESH_RECIPIENT_STATE_SCHEMA}\n{FRESH_CHILD_REQUEST_SCHEMA}\n{FRESH_HANDOFF_SCHEMA}\n{FRESH_ROOT_EFFECT_SCHEMA}\n{FRESH_BASH_CHILD_SCHEMA}\n{FRESH_BASH_SOURCE_SCHEMA}\n{FRESH_BASH_NOTIFY_SCHEMA}\n{FRESH_ROOT_TERMINAL_SCHEMA}\n{FRESH_BASH_SYNC_PUBLICATION_SCHEMA}"
            ))
            .map_err(|e| e.to_string())?;
        state_conn
            .execute(
                "INSERT INTO fresh_lane_state_identity VALUES(1,?1,?2,?3,?4)",
                params![
                    LANE_PROTOCOL,
                    identity.lane_id,
                    identity.domain_id,
                    identity.source_generation
                ],
            )
            .map_err(|e| e.to_string())?;
        drop(state_conn);
        // The binding names the *published* path while recording the staged
        // inode. It becomes valid only at the atomic no-replace rename.
        let projected = BoundStateSource {
            path: final_root.join("state.db"),
            owner: 0,
            device: state_meta.dev(),
            inode: state_meta.ino(),
        };
        let mut binding = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(sidecar_root.join("state-source.json"))
            .map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut binding, &projected).map_err(|e| e.to_string())?;
        binding.sync_all().map_err(|e| e.to_string())?;
        harden_tree(&stage_root)?;
        File::open(&sidecar_root)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        File::open(&stage_root)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        rename_noreplace(&stage_root, &final_root)?;
        File::open(broker_root)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(Self::open_at(broker_root)?.identity)
    }

    pub fn open_at(broker_root: &Path) -> Result<Self, String> {
        require_root()?;
        unsafe { libc::umask(0o077) };
        require_broker_root(broker_root)?;
        let lane_root = broker_root.join(LANE_DIRECTORY);
        let provider_meta = fs::symlink_metadata(lane_root.join(FRESH_PROVIDER_DIRECTORY))
            .map_err(|e| format!("fresh provider ledger directory absent: {e}"))?;
        if !provider_meta.is_dir()
            || provider_meta.file_type().is_symlink()
            || provider_meta.uid() != 0
            || provider_meta.mode() & 0o777 != 0o700
        {
            return Err("fresh provider ledger directory changed".into());
        }
        let sidecar =
            BrokerSidecar::open_existing(&lane_root.join("sidecar/pid-identity.db"), &lane_root)?;
        let state = sidecar.bound_state()?;
        let state_meta =
            fs::symlink_metadata(lane_root.join("state.db")).map_err(|e| e.to_string())?;
        if state_meta.uid() != 0 || state_meta.mode() & 0o077 != 0 || state_meta.nlink() != 1 {
            return Err("fresh State storage is not root-only".into());
        }
        let row = sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT protocol,lane_id,domain_id,source_generation,state_device,state_inode
             FROM fresh_lane_identity WHERE singleton=1",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                    ))
                },
            )
            .map_err(|e| format!("fresh lane identity absent: {e}"))?;
        let protected_schema_count: i64 = sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE
             (type='table' AND name IN ('fresh_lane_identity','fresh_lane_session')) OR
             (type='trigger' AND name IN ('fresh_lane_identity_no_update',
              'fresh_lane_identity_no_delete','fresh_lane_session_no_update',
              'fresh_lane_session_no_delete'))",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if protected_schema_count != 6 {
            return Err("fresh lane immutable schema is incomplete".into());
        }
        let recipient_schema_count: i64 = sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE
             (type='table' AND name IN ('fresh_recipient_source','fresh_recipient_binding',
              'fresh_recipient_row_source','fresh_recipient_grant',
              'fresh_recipient_ack_delegation','fresh_recipient_ack_delegation_item')) OR
             (type='trigger' AND name IN ('fresh_recipient_source_no_update',
              'fresh_recipient_source_no_delete','fresh_recipient_binding_no_update',
              'fresh_recipient_binding_no_delete','fresh_recipient_row_source_no_update',
              'fresh_recipient_row_source_no_delete','fresh_recipient_grant_no_delete',
              'fresh_recipient_grant_update_guard',
              'fresh_recipient_ack_delegation_item_no_update',
              'fresh_recipient_ack_delegation_item_no_delete'))",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if recipient_schema_count != 16 {
            return Err("fresh recipient authority schema is incomplete".into());
        }
        let ack_schema_count: i64 = sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE
             (type='table' AND name='fresh_recipient_ack_evidence') OR
             (type='trigger' AND name IN ('fresh_recipient_ack_evidence_no_update',
              'fresh_recipient_ack_evidence_no_delete'))",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        match ack_schema_count {
            0 => sidecar
                .mailbox()
                .conn
                .execute_batch(FRESH_RECIPIENT_ACK_SCHEMA)
                .map_err(|e| e.to_string())?,
            3 => {}
            _ => return Err("fresh recipient ACK schema is incomplete".into()),
        }
        verify_fresh_sql_objects(
            &sidecar.mailbox().conn,
            FRESH_RECIPIENT_ACK_SCHEMA,
            "fresh_recipient_ack_evidence",
            &[
                "fresh_recipient_ack_evidence_no_update",
                "fresh_recipient_ack_evidence_no_delete",
            ],
        )?;
        let request_key_columns: i64 = sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('fresh_lane_session') WHERE name='request_id' AND \"notnull\"=1",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if request_key_columns != 1 {
            return Err("fresh lane session request identity is absent".into());
        }
        let unique_request_indexes: i64 = sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT count(*) FROM pragma_index_list('fresh_lane_session') AS idx
                 WHERE idx.\"unique\"=1
                   AND (SELECT count(*) FROM pragma_index_info(idx.name))=1
                   AND (SELECT name FROM pragma_index_info(idx.name))='request_id'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if unique_request_indexes != 1 {
            return Err("fresh lane session request identity is not unique".into());
        }
        let identity = FreshV30LaneIdentity {
            lane_id: row.1,
            domain_id: row.2,
            source_generation: row.3,
        };
        for id in [
            &identity.lane_id,
            &identity.domain_id,
            &identity.source_generation,
        ] {
            let parsed = Uuid::parse_str(id).map_err(|_| "invalid fresh lane UUID")?;
            if parsed.to_string() != *id {
                return Err("noncanonical fresh lane UUID".into());
            }
        }
        if row.0 != LANE_PROTOCOL
            || identity.domain_id != sidecar.domain_id()?
            || identity.source_generation != sidecar.source_generation()
            || row.4 != state_meta.dev() as i64
            || row.5 != state_meta.ino() as i64
            || state.path() != lane_root.join("state.db")
        {
            return Err("fresh lane State/mailbox identity mismatch".into());
        }
        let state_conn = Connection::open_with_flags(
            lane_root.join("state.db"),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .map_err(|e| e.to_string())?;
        let state_identity = state_conn
            .query_row(
                "SELECT protocol,lane_id,domain_id,source_generation
             FROM fresh_lane_state_identity WHERE singleton=1",
                [],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .map_err(|e| format!("fresh State identity absent: {e}"))?;
        let state_trigger_count: i64 = state_conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name IN
             ('fresh_lane_state_identity_no_update','fresh_lane_state_identity_no_delete')",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if state_identity
            != (
                LANE_PROTOCOL.to_owned(),
                identity.lane_id.clone(),
                identity.domain_id.clone(),
                identity.source_generation.clone(),
            )
            || state_trigger_count != 2
        {
            return Err("fresh State and mailbox lane identities differ".into());
        }
        let admission_schema_count: i64 = state_conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE
                 (type='table' AND name='fresh_lane_session_admission') OR
                 (type='trigger' AND name IN
                 ('fresh_lane_session_admission_no_update','fresh_lane_session_admission_no_delete'))",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if admission_schema_count != 3 {
            return Err("fresh State session admission schema is incomplete".into());
        }
        let accepted_schema_count: i64 = state_conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE
             (type='table' AND name IN ('fresh_lane_accepted_source',
              'fresh_lane_recipient_attachment')) OR
             (type='trigger' AND name IN ('fresh_lane_accepted_source_no_update',
              'fresh_lane_accepted_source_no_delete',
              'fresh_lane_recipient_attachment_no_update',
              'fresh_lane_recipient_attachment_no_delete'))",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if accepted_schema_count != 6 {
            return Err("fresh accepted source schema is incomplete".into());
        }
        // This embedded SQL is the additive upgrade for a previously
        // published empty v30 lane. Identity and all pre-existing fresh
        // schemas are checked before any write. The immediate transaction
        // makes concurrent broker opens see either old or complete new shape.
        state_conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        match fresh_child_request_schema_count(&state_conn)? {
            0 => state_conn
                .execute_batch(FRESH_CHILD_REQUEST_SCHEMA)
                .map_err(|e| e.to_string())?,
            3 => {}
            _ => return Err("fresh child request schema is incomplete".into()),
        }
        if fresh_child_request_schema_count(&state_conn)? != 3 {
            return Err("fresh child request schema is incomplete".into());
        }
        match fresh_handoff_schema_count(&state_conn)? {
            0 => state_conn
                .execute_batch(FRESH_HANDOFF_SCHEMA)
                .map_err(|e| e.to_string())?,
            6 => {}
            _ => return Err("fresh handoff schema is incomplete".into()),
        }
        if fresh_handoff_schema_count(&state_conn)? != 6 {
            return Err("fresh handoff schema is incomplete".into());
        }
        let root_intent_columns: i64 = state_conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('fresh_released_handoff')
             WHERE name='root_intent_kind' AND \"notnull\"=1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let old_bash_columns: i64 = state_conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('fresh_released_handoff')
             WHERE name='bash_handle'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if root_intent_columns != 1 || old_bash_columns != 0 {
            return Err("fresh root handoff schema conflicts with Bash placeholder schema".into());
        }
        verify_fresh_sql_objects(
            &state_conn,
            FRESH_HANDOFF_SCHEMA,
            "fresh_released_handoff",
            &[
                "fresh_released_handoff_no_update",
                "fresh_released_handoff_no_delete",
                "fresh_released_owner",
                "fresh_released_owner_no_update",
                "fresh_released_owner_no_delete",
            ],
        )?;
        match fresh_root_effect_schema_count(&state_conn)? {
            0 => state_conn
                .execute_batch(FRESH_ROOT_EFFECT_SCHEMA)
                .map_err(|e| e.to_string())?,
            3 => {}
            _ => return Err("fresh root effect schema is incomplete".into()),
        }
        if fresh_root_effect_schema_count(&state_conn)? != 3 {
            return Err("fresh root effect schema is incomplete".into());
        }
        verify_fresh_root_effect_schema(&state_conn)?;
        match fresh_bash_child_schema_count(&state_conn)? {
            0 => state_conn
                .execute_batch(FRESH_BASH_CHILD_SCHEMA)
                .map_err(|e| e.to_string())?,
            9 => {}
            _ => return Err("fresh Bash child schema is incomplete".into()),
        }
        if fresh_bash_child_schema_count(&state_conn)? != 9 {
            return Err("fresh Bash child schema is incomplete".into());
        }
        verify_fresh_bash_child_schema(&state_conn)?;
        match fresh_bash_source_schema_count(&state_conn)? {
            0 => state_conn
                .execute_batch(FRESH_BASH_SOURCE_SCHEMA)
                .map_err(|e| e.to_string())?,
            6 => {}
            _ => return Err("fresh Bash source schema is incomplete".into()),
        }
        verify_fresh_bash_source_schema(&state_conn)?;
        match fresh_bash_notify_schema_count(&state_conn)? {
            0 => state_conn
                .execute_batch(FRESH_BASH_NOTIFY_SCHEMA)
                .map_err(|e| e.to_string())?,
            3 => {}
            _ => return Err("fresh Bash notification schema is incomplete".into()),
        }
        verify_fresh_bash_notify_schema(&state_conn)?;
        match fresh_bash_listener_schema_count(&state_conn)? {
            0 => state_conn
                .execute_batch(FRESH_BASH_LISTENER_SCHEMA)
                .map_err(|e| e.to_string())?,
            3 => {}
            _ => return Err("fresh Bash listener schema is incomplete".into()),
        }
        verify_fresh_bash_listener_schema(&state_conn)?;
        match fresh_root_terminal_schema_count(&state_conn)? {
            0 => state_conn
                .execute_batch(FRESH_ROOT_TERMINAL_SCHEMA)
                .map_err(|e| e.to_string())?,
            6 => {}
            _ => return Err("fresh root terminal schema incomplete".into()),
        }
        verify_fresh_root_terminal_schema(&state_conn)?;
        match fresh_bash_sync_publication_schema_count(&state_conn)? {
            0 => state_conn
                .execute_batch(FRESH_BASH_SYNC_PUBLICATION_SCHEMA)
                .map_err(|e| e.to_string())?,
            3 => {}
            _ => return Err("fresh Bash sync publication schema incomplete".into()),
        }
        verify_fresh_bash_sync_publication_schema(&state_conn)?;
        match fresh_normal_work_schema_count(&state_conn)? {
            0 => state_conn
                .execute_batch(FRESH_NORMAL_WORK_SCHEMA)
                .map_err(|e| e.to_string())?,
            3 => {}
            _ => return Err("fresh normal work schema is incomplete".into()),
        }
        if fresh_normal_work_schema_count(&state_conn)? != 3 {
            return Err("fresh normal work schema is incomplete".into());
        }
        verify_fresh_sql_objects(
            &state_conn,
            FRESH_NORMAL_WORK_SCHEMA,
            "fresh_normal_work_preparation",
            &[
                "fresh_normal_work_preparation_no_update",
                "fresh_normal_work_preparation_no_delete",
            ],
        )?;
        let normal_trigger_count: i64 = state_conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='trigger' AND tbl_name='fresh_normal_work_preparation'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if normal_trigger_count != 2 {
            return Err("fresh normal work schema differs from embedded SQL".into());
        }
        state_conn
            .execute_batch("COMMIT")
            .map_err(|e| e.to_string())?;
        Ok(Self {
            sidecar,
            identity,
            state_path: lane_root.join("state.db"),
        })
    }

    pub fn identity(&self) -> &FreshV30LaneIdentity {
        &self.identity
    }

    /// Copy only an old-authority receipt delivered over the broker's typed
    /// in-process channel. The caller cannot insert one through the socket.
    /// A committed old receipt may remain debt if this write is interrupted.
    pub fn bind_released_handoff(
        &self,
        receipt: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
    ) -> Result<(), String> {
        self.validate_released_handoff(receipt, actor)?;
        let json = serde_json::to_string(receipt).map_err(|e| e.to_string())?;
        let actor_json = serde_json::to_string(actor).map_err(|e| e.to_string())?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state
            .execute_batch("PRAGMA synchronous=FULL")
            .map_err(|e| e.to_string())?;
        state.execute(
            "INSERT INTO fresh_released_handoff
             (handoff_id,d_key,invocation_uuid,root_intent_kind,root_id,actor_identity,receipt_json,bound_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT DO NOTHING",
            params![receipt.handoff_id, receipt.d_key, receipt.invocation_uuid,
                receipt.root_work_intent.kind(), receipt.old_release.prepared.root_id,
                actor_json, json, Utc::now().to_rfc3339()],
        ).map_err(|e| e.to_string())?;
        drop(state);
        self.require_released_handoff(&receipt.d_key, receipt, actor)
    }

    pub fn require_released_handoff(
        &self,
        d_key: &str,
        receipt: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
    ) -> Result<(), String> {
        self.validate_released_handoff(receipt, actor)?;
        if d_key != receipt.d_key {
            return Err("fresh handoff D key conflict".into());
        }
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let row: Option<(String, String)> = state
            .query_row(
                "SELECT receipt_json,actor_identity FROM fresh_released_handoff WHERE d_key=?1",
                [d_key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((json, actor_json)) = row else {
            return Err("fresh released handoff absent".into());
        };
        let stored: FreshReleasedHandoff =
            read_bash_json(&json, "fresh_released_handoff.receipt_json")?;
        let stored_actor: FreshRecipientIdentity =
            read_bash_json(&actor_json, "fresh_released_handoff.actor_identity")?;
        if stored != *receipt || stored_actor != *actor {
            return Err("fresh released handoff changed or belongs to another child".into());
        }
        Ok(())
    }

    pub fn released_handoff_for_child(
        &self,
        d_key: &str,
        actor: &FreshRecipientIdentity,
    ) -> Result<FreshReleasedHandoff, String> {
        validate_request_id(d_key)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let json: String = state
            .query_row(
                "SELECT receipt_json FROM fresh_released_handoff WHERE d_key=?1",
                [d_key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .ok_or("fresh released handoff absent before D")?;
        let receipt: FreshReleasedHandoff =
            read_bash_json(&json, "fresh_released_handoff.receipt_json")?;
        self.require_released_handoff(d_key, &receipt, actor)?;
        Ok(receipt)
    }

    fn validate_released_handoff(
        &self,
        receipt: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
    ) -> Result<(), String> {
        for id in [
            &receipt.handoff_id,
            &receipt.d_key,
            &receipt.invocation_uuid,
            &receipt.broker_incarnation,
        ] {
            validate_request_id(id)?;
        }
        let child = &receipt.old_release.prepared.joined_child;
        if receipt.fresh_lane != self.identity
            || receipt.old_release.prepared.source_generation == self.identity.source_generation
            || receipt.old_release.prepared.domain_id == self.identity.domain_id
            || !receipt.root_work_intent.valid()
            || receipt.runner_image_device == 0
            || receipt.runner_image_inode == 0
            || receipt.old_release.release_id.is_empty()
            || actor.host_pid != child.host_pid
            || actor.boot_id != child.boot_id
            || actor.starttime_ticks != child.starttime_ticks
            || actor.pidns_dev != child.pidns_dev
            || actor.pidns_ino != child.pidns_ino
            || crate::CompletionRegistrationAuthority::from_process_environment_value(
                receipt.registration_authority.clone(),
            )
            .is_err()
        {
            return Err("fresh released handoff identity conflict".into());
        }
        Ok(())
    }

    /// D's State invocation and owner association are recoverable under the
    /// same broker-minted UUID and registration secret. A partial write is
    /// debt; no different handoff can adopt it.
    pub fn ensure_released_invocation(
        &self,
        receipt: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
    ) -> Result<(), String> {
        self.require_released_handoff(&receipt.d_key, receipt, actor)?;
        self.require_session(session)?;
        if session.request_id != receipt.d_key {
            return Err("fresh handoff session D key conflict".into());
        }
        let authority = crate::CompletionRegistrationAuthority::from_process_environment_value(
            receipt.registration_authority.clone(),
        )?;
        let state = StateDb::open(&self.state_path)?;
        if state
            .get_invocation_by_uuid(&receipt.invocation_uuid)?
            .is_none()
        {
            state.start_invocation_with_prepared_completion_registration_authority(
                &crate::InvocationStart {
                    invocation_uuid: receipt.invocation_uuid.clone(),
                    model_name: "agent-runner-root".into(),
                    provider_name: "agent-runner".into(),
                    provider_index: 0,
                    parent_invocation_id: None,
                },
                &authority,
            )?;
        }
        let row = state
            .get_invocation_by_uuid(&receipt.invocation_uuid)?
            .ok_or("fresh handoff invocation absent after start")?;
        if row.model_name != "agent-runner-root"
            || row.provider_name.as_deref() != Some("agent-runner")
            || row.provider_index != 0
            || row.parent_invocation_id.is_some()
            || row
                .session_id
                .as_deref()
                .is_some_and(|id| id != session.session_id)
        {
            return Err("fresh handoff invocation row conflict".into());
        }
        let read = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let digest: Option<String> = read.query_row(
            "SELECT completion_registration_capability_digest FROM invocations WHERE invocation_uuid=?1",
            [&receipt.invocation_uuid],
            |r| r.get(0),
        ).map_err(|e| e.to_string())?;
        if digest.as_deref() != Some(authority.digest().as_str()) {
            return Err("fresh handoff registration authority conflict".into());
        }
        state.bind_invocation_provider_session_start(
            crate::InvocationMutationAuthority::Standalone,
            row.id,
            &crate::ProviderSessionBinding {
                provider_session_id: session.session_id.clone(),
                capture_method: "broker-released-root-v30",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )?;
        let owner = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        owner
            .execute_batch("PRAGMA synchronous=FULL")
            .map_err(|e| e.to_string())?;
        owner
            .execute(
                "INSERT INTO fresh_released_owner
             (handoff_id,invocation_uuid,session_id,actor_identity,bound_at)
             VALUES(?1,?2,?3,?4,?5) ON CONFLICT DO NOTHING",
                params![
                    receipt.handoff_id,
                    receipt.invocation_uuid,
                    session.session_id,
                    serde_json::to_string(actor).map_err(|e| e.to_string())?,
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(|e| e.to_string())?;
        let rebound = state
            .get_invocation_by_uuid(&receipt.invocation_uuid)?
            .ok_or("fresh handoff invocation lost after owner binding")?;
        if rebound.provider_session_id.as_deref() != Some(session.session_id.as_str()) {
            return Err("fresh handoff invocation/session/owner readback conflict".into());
        }
        self.require_released_invocation(receipt, actor, session)
    }

    pub fn require_released_invocation(
        &self,
        receipt: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
    ) -> Result<(), String> {
        self.require_released_handoff(&receipt.d_key, receipt, actor)?;
        self.require_session(session)?;
        let authority = crate::CompletionRegistrationAuthority::from_process_environment_value(
            receipt.registration_authority.clone(),
        )?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let row: Option<(
            String,
            String,
            i64,
            Option<i64>,
            Option<String>,
            Option<String>,
        )> = state
            .query_row(
                "SELECT model_name,provider_name,provider_index,parent_invocation_id,
                 provider_session_id,completion_registration_capability_digest
                 FROM invocations WHERE invocation_uuid=?1",
                [&receipt.invocation_uuid],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if row
            != Some((
                "agent-runner-root".into(),
                "agent-runner".into(),
                0,
                None,
                Some(session.session_id.clone()),
                Some(authority.digest()),
            ))
        {
            return Err("fresh invocation/session/registration readback conflict".into());
        }
        let owner: Option<(String, String, String)> = state
            .query_row(
                "SELECT invocation_uuid,session_id,actor_identity FROM fresh_released_owner
             WHERE handoff_id=?1",
                [&receipt.handoff_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if owner
            != Some((
                receipt.invocation_uuid.clone(),
                session.session_id.clone(),
                serde_json::to_string(actor).map_err(|e| e.to_string())?,
            ))
        {
            return Err("fresh child owner association changed".into());
        }
        Ok(())
    }

    /// The broker must call this only after reattesting the live old release.
    /// A lost reply leaves a durable started/unknown effect; it cannot be
    /// replayed to execute the same root a second time.
    pub fn begin_root_effect(
        &self,
        receipt: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
    ) -> Result<FreshRootEffect, String> {
        self.require_released_invocation(receipt, actor, session)?;
        if !receipt.root_work_intent.returnable_entry() {
            return Err("root intent has no returnable effect entry".into());
        }
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state
            .execute_batch("PRAGMA synchronous=FULL; BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        state
            .execute(
                "INSERT INTO fresh_root_effect
             (handoff_id,invocation_uuid,session_id,actor_identity,intent_json,state,started_at)
             VALUES(?1,?2,?3,?4,?5,'started',?6)",
                params![
                    receipt.handoff_id,
                    receipt.invocation_uuid,
                    session.session_id,
                    serde_json::to_string(actor).map_err(|e| e.to_string())?,
                    serde_json::to_string(&receipt.root_work_intent).map_err(|e| e.to_string())?,
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(|e| e.to_string())?;
        state.execute_batch("COMMIT").map_err(|e| e.to_string())?;
        self.read_root_effect(receipt, actor, session)?
            .ok_or_else(|| "root effect start lost its durable row".into())
    }

    pub fn read_root_effect(
        &self,
        receipt: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
    ) -> Result<Option<FreshRootEffect>, String> {
        self.require_released_invocation(receipt, actor, session)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let row: Option<(String, String, String, String, String, String)> = state
            .query_row(
                "SELECT handoff_id,invocation_uuid,session_id,actor_identity,intent_json,state
             FROM fresh_root_effect WHERE handoff_id=?1",
                [&receipt.handoff_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((handoff_id, invocation_uuid, session_id, actor_json, intent_json, status)) = row
        else {
            return Ok(None);
        };
        if handoff_id != receipt.handoff_id
            || invocation_uuid != receipt.invocation_uuid
            || session_id != session.session_id
            || read_bash_json::<FreshRecipientIdentity>(
                &actor_json,
                "fresh_root_effect.actor_identity",
            )? != *actor
            || read_bash_json::<FreshRootWorkIntent>(&intent_json, "fresh_root_effect.intent_json")?
                != receipt.root_work_intent
        {
            return Err("root effect identity readback conflict".into());
        }
        let state = match status.as_str() {
            "started" => FreshRootEffectState::Started,
            "returned_success" => FreshRootEffectState::ReturnedSuccess,
            "returned_failure" => FreshRootEffectState::ReturnedFailure,
            _ => return Err("root effect state invalid".into()),
        };
        Ok(Some(FreshRootEffect {
            handoff_id,
            invocation_uuid,
            session_id,
            actor: actor.clone(),
            intent: receipt.root_work_intent.clone(),
            state,
        }))
    }

    /// A returned entry result is separate from physical drain and from any
    /// provider/native-work result. Duplicate identical returns are readback.
    pub fn return_root_effect(
        &self,
        receipt: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
        success: bool,
    ) -> Result<FreshRootEffect, String> {
        let existing = self
            .read_root_effect(receipt, actor, session)?
            .ok_or("root effect was never started")?;
        let terminal = if success {
            FreshRootEffectState::ReturnedSuccess
        } else {
            FreshRootEffectState::ReturnedFailure
        };
        if existing.state == terminal {
            return Ok(existing);
        }
        if existing.state != FreshRootEffectState::Started {
            return Err("root effect already returned a different result".into());
        }
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state
            .execute_batch("PRAGMA synchronous=FULL")
            .map_err(|e| e.to_string())?;
        let changed = state
            .execute(
                "UPDATE fresh_root_effect SET state=?2,returned_at=?3
             WHERE handoff_id=?1 AND state='started'",
                params![
                    receipt.handoff_id,
                    if success {
                        "returned_success"
                    } else {
                        "returned_failure"
                    },
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(|e| e.to_string())?;
        let readback = self
            .read_root_effect(receipt, actor, session)?
            .ok_or("root effect disappeared after return")?;
        if changed > 1 || readback.state != terminal {
            return Err("root effect return readback conflict".into());
        }
        Ok(readback)
    }

    /// Persist a no-fork normal-work boundary under the exact released root.
    /// The broker still has no native K, so this cannot authorize execution.
    pub fn prepare_normal_work(
        &self,
        receipt: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
    ) -> Result<FreshNormalWorkPreparation, String> {
        self.require_released_invocation(receipt, actor, session)?;
        if !matches!(&receipt.root_work_intent, FreshRootWorkIntent::NormalCli(_))
            || !receipt.root_work_intent.valid()
        {
            return Err("normal work requires exact normal root intent".into());
        }
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state
            .execute_batch("PRAGMA synchronous=FULL; BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        let started: i64 = state
            .query_row(
                "SELECT count(*) FROM fresh_root_effect WHERE handoff_id=?1",
                [&receipt.handoff_id],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if started != 0 {
            return Err("normal work preparation followed root effect start".into());
        }
        state
            .execute(
                "INSERT INTO fresh_normal_work_preparation
             (handoff_id,invocation_uuid,session_id,actor_identity,intent_json,state,prepared_at)
             VALUES(?1,?2,?3,?4,?5,'held',?6)
             ON CONFLICT(handoff_id) DO NOTHING",
                params![
                    receipt.handoff_id,
                    receipt.invocation_uuid,
                    session.session_id,
                    serde_json::to_string(actor).map_err(|e| e.to_string())?,
                    serde_json::to_string(&receipt.root_work_intent).map_err(|e| e.to_string())?,
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(|e| e.to_string())?;
        state.execute_batch("COMMIT").map_err(|e| e.to_string())?;
        self.read_normal_work(receipt, actor, session)?
            .ok_or_else(|| "normal work preparation disappeared".into())
    }

    pub fn read_normal_work(
        &self,
        receipt: &FreshReleasedHandoff,
        actor: &FreshRecipientIdentity,
        session: &FreshV30Session,
    ) -> Result<Option<FreshNormalWorkPreparation>, String> {
        self.require_released_invocation(receipt, actor, session)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let row: Option<(String, String, String, String, String, String)> = state
            .query_row(
                "SELECT handoff_id,invocation_uuid,session_id,actor_identity,intent_json,state
             FROM fresh_normal_work_preparation WHERE handoff_id=?1",
                [&receipt.handoff_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((handoff_id, invocation_uuid, session_id, actor_json, intent_json, state)) = row
        else {
            return Ok(None);
        };
        if handoff_id != receipt.handoff_id
            || invocation_uuid != receipt.invocation_uuid
            || session_id != session.session_id
            || state != "held"
            || read_bash_json::<FreshRecipientIdentity>(
                &actor_json,
                "fresh_normal_work.actor_identity",
            )? != *actor
            || read_bash_json::<FreshRootWorkIntent>(&intent_json, "fresh_normal_work.intent_json")?
                != receipt.root_work_intent
        {
            return Err("normal work preparation identity readback conflict".into());
        }
        Ok(Some(FreshNormalWorkPreparation {
            handoff_id,
            invocation_uuid,
            session_id,
            actor: actor.clone(),
            intent: receipt.root_work_intent.clone(),
            state,
        }))
    }

    /// The intended caller is a released Runner root retaining both UUIDs
    /// across U and D. The broker supplies the pinned peer identity; caller
    /// JSON cannot select an actor. Release and invocation linkage are still
    /// a later gate. This intent grants no work or registration authority.
    pub fn reserve_child_request(
        &self,
        request_id: &str,
        invocation_uuid: &str,
        actor: &FreshRecipientIdentity,
    ) -> Result<(), String> {
        validate_request_id(request_id)?;
        validate_request_id(invocation_uuid)?;
        if actor.host_pid <= 0 || actor.starttime_ticks == 0 || actor.boot_id.is_empty() {
            return Err("invalid fresh child actor".into());
        }
        let actor_json = serde_json::to_string(actor).map_err(|e| e.to_string())?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state
            .execute(
                "INSERT INTO fresh_lane_child_request
                 (request_id,invocation_uuid,actor_identity,lane_id,source_generation,reserved_at)
                 VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT DO NOTHING",
                params![
                    request_id,
                    invocation_uuid,
                    actor_json,
                    self.identity.lane_id,
                    self.identity.source_generation,
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(|e| e.to_string())?;
        self.require_child_request(request_id, invocation_uuid, actor)
    }

    pub fn require_child_request(
        &self,
        request_id: &str,
        invocation_uuid: &str,
        actor: &FreshRecipientIdentity,
    ) -> Result<(), String> {
        validate_request_id(request_id)?;
        validate_request_id(invocation_uuid)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let row: Option<(String, String, String, String)> = state
            .query_row(
                "SELECT invocation_uuid,actor_identity,lane_id,source_generation
                 FROM fresh_lane_child_request WHERE request_id=?1",
                [request_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((stored_invocation, stored_actor, lane_id, source_generation)) = row else {
            return Err("fresh child request absent".into());
        };
        if stored_invocation != invocation_uuid
            || read_bash_json::<FreshRecipientIdentity>(
                &stored_actor,
                "fresh_lane_child_request.actor_identity",
            )? != *actor
            || lane_id != self.identity.lane_id
            || source_generation != self.identity.source_generation
        {
            return Err("fresh child request actor or lane conflict".into());
        }
        Ok(())
    }

    /// Production D requires a preceding child intent. Private fixtures may
    /// exercise older D rows, but cannot spend a reserved sibling key.
    pub fn require_child_actor(
        &self,
        request_id: &str,
        actor: &FreshRecipientIdentity,
        required: bool,
    ) -> Result<(), String> {
        validate_request_id(request_id)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let stored: Option<String> = state
            .query_row(
                "SELECT actor_identity FROM fresh_lane_child_request WHERE request_id=?1",
                [request_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        match stored {
            Some(stored)
                if read_bash_json::<FreshRecipientIdentity>(
                    &stored,
                    "fresh_lane_child_request.actor_identity",
                )? == *actor => {}
            Some(_) => return Err("fresh child request belongs to another actor".into()),
            None if required => return Err("fresh child request absent before D".into()),
            None => {}
        }
        Ok(())
    }

    /// A request UUID is created before the first send and retained by the
    /// caller until readback. SQLite's unique request key makes concurrent
    /// retries and lost replies return the exact first allocation. The key
    /// never selects a session ID or an old ledger.
    pub fn allocate_session(&mut self, request_id: &str) -> Result<FreshV30Session, String> {
        validate_request_id(request_id)?;
        let mailbox_existing: bool = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fresh_lane_session WHERE request_id=?1)",
                [request_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if mailbox_existing {
            return self
                .read_session(request_id)?
                .ok_or_else(|| "fresh mailbox session disappeared".into());
        }
        // State is written first. A crash before the mailbox insert leaves a
        // non-effect-bearing reservation that the same D request can finish.
        // A different D key never adopts it. Neither database is copied from
        // v29, and no session is returned until both exact rows agree.
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        let candidate = FreshV30Session {
            lane_id: self.identity.lane_id.clone(),
            source_generation: self.identity.source_generation.clone(),
            session_id: format!("v30:{}:{}", self.identity.lane_id, Uuid::new_v4()),
            request_id: request_id.to_owned(),
            allocation_id: Uuid::new_v4().to_string(),
        };
        state.execute(
            "INSERT INTO fresh_lane_session_admission(request_id,session_id,allocation_id,lane_id,source_generation,admitted_at)
             VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(request_id) DO NOTHING",
            params![candidate.request_id, candidate.session_id, candidate.allocation_id,
                candidate.lane_id, candidate.source_generation, Utc::now().to_rfc3339()],
        ).map_err(|e| e.to_string())?;
        let session = self
            .read_state_admission_on(&state, request_id)?
            .ok_or("fresh State session admission absent after commit")?;
        self.sidecar.mailbox().conn.execute(
            "INSERT INTO fresh_lane_session(session_id,request_id,allocation_id,lane_id,source_generation,allocated_at)
             VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(request_id) DO NOTHING",
            params![session.session_id, session.request_id, session.allocation_id,
                session.lane_id, session.source_generation, Utc::now().to_rfc3339()],
        ).map_err(|e| e.to_string())?;
        self.read_session(request_id)?
            .ok_or_else(|| "fresh session allocation lost its durable row".into())
    }

    /// Exact readback is available without allocating, including after a
    /// restart or while the entry gate is closed.
    pub fn read_session(&self, request_id: &str) -> Result<Option<FreshV30Session>, String> {
        validate_request_id(request_id)?;
        let mut query = self
            .sidecar
            .mailbox()
            .conn
            .prepare(
                "SELECT session_id,request_id,allocation_id,lane_id,source_generation
             FROM fresh_lane_session WHERE request_id=?1",
            )
            .map_err(|e| e.to_string())?;
        let mut rows = query
            .query(params![request_id])
            .map_err(|e| e.to_string())?;
        let session = rows
            .next()
            .map_err(|e| e.to_string())?
            .map(|row| {
                Ok::<_, rusqlite::Error>(FreshV30Session {
                    session_id: row.get(0)?,
                    request_id: row.get(1)?,
                    allocation_id: row.get(2)?,
                    lane_id: row.get(3)?,
                    source_generation: row.get(4)?,
                })
            })
            .transpose()
            .map_err(|e| e.to_string())?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let admitted = self.read_state_admission_on(&state, request_id)?;
        if session != admitted {
            return Err(
                "fresh mailbox and State session admissions differ or D is incomplete".into(),
            );
        }
        if let Some(ref session) = session {
            self.require_session(session)?;
        }
        Ok(session)
    }

    fn state_connection(&self, flags: OpenFlags) -> Result<Connection, String> {
        let state = self.sidecar.bound_state()?;
        if state.path() != self.state_path {
            return Err("fresh State path changed".into());
        }
        Connection::open_with_flags(&self.state_path, flags).map_err(|e| e.to_string())
    }

    fn read_state_admission_on(
        &self,
        state: &Connection,
        request_id: &str,
    ) -> Result<Option<FreshV30Session>, String> {
        let row = state
            .query_row(
                "SELECT session_id,request_id,allocation_id,lane_id,source_generation
             FROM fresh_lane_session_admission WHERE request_id=?1",
                [request_id],
                |r| {
                    Ok(FreshV30Session {
                        session_id: r.get(0)?,
                        request_id: r.get(1)?,
                        allocation_id: r.get(2)?,
                        lane_id: r.get(3)?,
                        source_generation: r.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(ref session) = row {
            let suffix = session
                .session_id
                .strip_prefix(&format!("v30:{}:", self.identity.lane_id))
                .ok_or("fresh State session ID has wrong lane")?;
            for id in [&session.request_id, &session.allocation_id, suffix] {
                let parsed =
                    Uuid::parse_str(id).map_err(|_| "fresh State admission UUID invalid")?;
                if parsed.is_nil() || parsed.to_string() != *id {
                    return Err("fresh State admission UUID is noncanonical or nil".into());
                }
            }
            if session.request_id != request_id
                || session.lane_id != self.identity.lane_id
                || session.source_generation != self.identity.source_generation
            {
                return Err("fresh State admission has wrong request or generation".into());
            }
        }
        Ok(row)
    }

    /// Routing prerequisite for a later v30 resume, wake, recipient or ACK
    /// protocol. This alone grants no work, delivery or acknowledgement.
    /// An unqualified `(session,seq)` is not an authority input.
    pub fn require_session(&self, session: &FreshV30Session) -> Result<(), String> {
        if session.lane_id != self.identity.lane_id
            || session.source_generation != self.identity.source_generation
        {
            return Err("wrong fresh lane or generation".into());
        }
        let exists: bool = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM fresh_lane_session WHERE session_id=?1
             AND request_id=?2 AND allocation_id=?3 AND lane_id=?4 AND source_generation=?5)",
                params![
                    session.session_id,
                    session.request_id,
                    session.allocation_id,
                    session.lane_id,
                    session.source_generation
                ],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exists {
            return Err("session was not allocated by this lane".into());
        }
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        if self
            .read_state_admission_on(&state, &session.request_id)?
            .as_ref()
            != Some(session)
        {
            return Err("session lacks exact fresh State admission".into());
        }
        Ok(())
    }

    /// Exact lane/row membership only; recipient and ACK grants are separate.
    pub fn require_mailbox_row(&self, session: &FreshV30Session, seq: i64) -> Result<(), String> {
        self.require_session(session)?;
        let exists: bool = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM mailbox WHERE session_id=?1 AND seq=?2)",
                params![session.session_id, seq],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !exists {
            return Err("mailbox row is not in this lane".into());
        }
        Ok(())
    }

    /// A caller cannot nominate a session or registration for source
    /// admission. Resolve the listener's session from this immutable ledger,
    /// then re-read the consumed grant's exact State, owner, registration and
    /// listener projection. No provenance row is written by this method.
    pub fn source_provenance(
        &self,
        grant: &super::BrokerSourceEffectGrant,
    ) -> Result<FreshV30SourceProvenance, String> {
        if grant.source_generation != self.identity.source_generation
            || !Self::is_reserved_session_id(&grant.candidate.listener.session_id)
        {
            return Err("source is outside the fresh lane generation/session".into());
        }
        let session = self
            .sidecar
            .mailbox()
            .conn
            .query_row(
                "SELECT session_id,request_id,allocation_id,lane_id,source_generation
                 FROM fresh_lane_session WHERE session_id=?1",
                [&grant.candidate.listener.session_id],
                |row| {
                    Ok(FreshV30Session {
                        session_id: row.get(0)?,
                        request_id: row.get(1)?,
                        allocation_id: row.get(2)?,
                        lane_id: row.get(3)?,
                        source_generation: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or("source listener has no broker-minted fresh session")?;
        self.require_session(&session)?;
        let binding = self.sidecar.read_consumed_source_candidate(grant)?;
        let source = binding.registration()?;
        let listener = binding.admission_listener()?;
        if source.domain_id != self.identity.domain_id
            || source.owner_session_id != session.session_id
            || listener.session_id != session.session_id
            || listener.owner_invocation_uuid != source.owner_invocation_uuid
            || listener != grant.candidate.listener
            || source.registration_id != grant.candidate.registration_id
            || binding.registration_digest() != grant.candidate.registration_digest
        {
            return Err("fresh source State/session/owner/listener provenance changed".into());
        }
        Ok(FreshV30SourceProvenance {
            lane_id: session.lane_id,
            allocation_id: session.allocation_id,
            session_id: session.session_id,
            source_generation: grant.source_generation.clone(),
            state_admission_id: binding.caller_admission_id().into(),
            registration_id: source.registration_id,
            registration_digest: binding.registration_digest().into(),
            root_id: grant.root_id.clone(),
            owner_generation: grant.owner_generation.clone(),
        })
    }
}

fn fresh_child_request_schema_count(state: &Connection) -> Result<i64, String> {
    state
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE
             (type='table' AND name='fresh_lane_child_request') OR
             (type='trigger' AND name IN
             ('fresh_lane_child_request_no_update','fresh_lane_child_request_no_delete'))",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
}

fn fresh_handoff_schema_count(state: &Connection) -> Result<i64, String> {
    state
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE
         (type='table' AND name IN ('fresh_released_handoff','fresh_released_owner')) OR
         (type='trigger' AND name IN
          ('fresh_released_handoff_no_update','fresh_released_handoff_no_delete',
           'fresh_released_owner_no_update','fresh_released_owner_no_delete'))",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
}

fn fresh_root_effect_schema_count(state: &Connection) -> Result<i64, String> {
    state
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE
         (type='table' AND name='fresh_root_effect') OR
         (type='trigger' AND name IN
          ('fresh_root_effect_no_delete','fresh_root_effect_return_once'))",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
}

fn verify_fresh_root_effect_schema(state: &Connection) -> Result<(), String> {
    fn objects(state: &Connection) -> Result<Vec<(String, String, String)>, String> {
        let mut statement = state
            .prepare(
                "SELECT type,name,sql FROM sqlite_master WHERE
                 (type='table' AND name='fresh_root_effect') OR
                 (type='trigger' AND tbl_name='fresh_root_effect')
                 ORDER BY type,name",
            )
            .map_err(|e| e.to_string())?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    let canonical = Connection::open_in_memory().map_err(|e| e.to_string())?;
    canonical
        .execute_batch(FRESH_ROOT_EFFECT_SCHEMA)
        .map_err(|e| e.to_string())?;
    if objects(state)? != objects(&canonical)? {
        return Err("fresh root effect schema differs from embedded SQL".into());
    }
    Ok(())
}

fn fresh_normal_work_schema_count(state: &Connection) -> Result<i64, String> {
    state
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE
         (type='table' AND name='fresh_normal_work_preparation') OR
         (type='trigger' AND name IN
          ('fresh_normal_work_preparation_no_update','fresh_normal_work_preparation_no_delete'))",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())
}

fn verify_fresh_sql_objects(
    state: &Connection,
    sql: &str,
    first: &str,
    others: &[&str],
) -> Result<(), String> {
    fn objects(
        state: &Connection,
        names: &[&str],
    ) -> Result<Vec<(String, String, String)>, String> {
        let mut statement = state.prepare(
            "SELECT type,name,sql FROM sqlite_master WHERE name=?1 OR name=?2 OR name=?3 OR name=?4 OR name=?5 OR name=?6 ORDER BY type,name"
        ).map_err(|e| e.to_string())?;
        let mut bind = [""; 6];
        for (slot, name) in bind.iter_mut().zip(names) {
            *slot = name;
        }
        statement
            .query_map(rusqlite::params_from_iter(bind), |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
    let canonical = Connection::open_in_memory().map_err(|e| e.to_string())?;
    canonical.execute_batch(sql).map_err(|e| e.to_string())?;
    let mut names = vec![first];
    names.extend_from_slice(others);
    if objects(state, &names)? != objects(&canonical, &names)? {
        return Err(format!("fresh {first} schema differs from embedded SQL"));
    }
    Ok(())
}

fn validate_request_id(request_id: &str) -> Result<(), String> {
    let parsed = Uuid::parse_str(request_id).map_err(|_| "invalid fresh request UUID")?;
    if parsed.is_nil() || parsed.to_string() != request_id {
        return Err("noncanonical or nil fresh request UUID".into());
    }
    Ok(())
}

fn require_root() -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("fresh v30 lane requires root".into());
    }
    Ok(())
}

fn require_broker_root(path: &Path) -> Result<(), String> {
    #[cfg(feature = "age319-private-broker-fixture")]
    if fs::read_to_string("/proc/self/uid_map")
        .ok()
        .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
    {
        let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if path.is_absolute()
            && meta.is_dir()
            && !meta.is_symlink()
            && meta.uid() == 0
            && meta.mode() & 0o077 == 0
        {
            return Ok(());
        }
        return Err("private fresh broker root is not owner-only".into());
    }
    require_root_owned_ancestors(path)
}

fn harden_tree(root: &Path) -> Result<(), String> {
    for item in fs::read_dir(root).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() || meta.uid() != 0 || (meta.is_file() && meta.nlink() != 1)
        {
            return Err("fresh lane staging artifact is untrusted".into());
        }
        if meta.is_dir() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
            harden_tree(&path)?;
        } else if meta.is_file() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        } else {
            return Err("fresh lane staging artifact type is unsupported".into());
        }
    }
    Ok(())
}

fn rename_noreplace(from: &Path, to: &Path) -> Result<(), String> {
    let from = CString::new(from.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    if unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } != 0
    {
        return Err(format!(
            "fresh v30 publication refused: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}
