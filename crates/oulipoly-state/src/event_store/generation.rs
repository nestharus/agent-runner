//! Durable generation-directory and two-slot head publication protocol.
//!
//! This module intentionally knows only one writer directory at a time.  It
//! never discovers sibling writers or historical generations.  Cross-writer
//! discovery and historical maintenance are detached-reader concerns.

use super::{Digest32, EVENT_SCHEMA_VERSION, GenerationId, ProducerIdentity, WriterInstanceId};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const EVENT_STORE_FORMAT_VERSION: u32 = 1;
pub const DATABASE_FILE_NAME: &str = "events.sqlite3";
pub const PREPARED_MANIFEST_FILE_NAME: &str = "prepared.manifest.json";
pub const SEALED_MANIFEST_FILE_NAME: &str = "sealed.manifest.json";
pub const SEALED_FILE_DIGEST_BUFFER_BYTES: usize = 64 * 1024;
pub const MAX_GENERATION_CONTROL_RECORD_BYTES: usize = 64 * 1024;

type GenerationAggregateFacts = (
    i64,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
);

/// A filesystem-safe lowercase hexadecimal rendering of a 128-bit identity.
pub fn id_hex(id: &[u8; 16]) -> String {
    let mut out = String::with_capacity(32);
    for byte in id {
        use fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub fn parse_id_hex(value: &str) -> Result<[u8; 16], GenerationError> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GenerationError::InvalidIdentity(value.to_owned()));
    }
    let mut bytes = [0_u8; 16];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let pair = std::str::from_utf8(chunk)
            .map_err(|_| GenerationError::InvalidIdentity(value.to_owned()))?;
        bytes[index] = u8::from_str_radix(pair, 16)
            .map_err(|_| GenerationError::InvalidIdentity(value.to_owned()))?;
    }
    Ok(bytes)
}

pub fn digest_hex(bytes: &[u8]) -> String {
    let digest = Digest32::sha256(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest.as_bytes() {
        use fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[derive(Debug)]
pub enum GenerationError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
    Json(serde_json::Error),
    Sqlite(rusqlite::Error),
    InvalidIdentity(String),
    UnsafePath(String),
    UnsupportedFilesystem(String),
    WriterAlreadyOwned(PathBuf),
    GenerationAlreadyOwned(PathBuf),
    NoRecoverableHead,
    HeadConflict(String),
    PublicationConflict(PathBuf),
    InvalidManifest(String),
    InvalidHead(String),
    Validation(String),
}

impl fmt::Display for GenerationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(f, "{operation}: {source}"),
            Self::Json(error) => write!(f, "invalid event-store JSON: {error}"),
            Self::Sqlite(error) => write!(f, "event-store SQLite failure: {error}"),
            Self::InvalidIdentity(value) => write!(f, "invalid 128-bit identity: {value}"),
            Self::UnsafePath(reason) => write!(f, "unsafe event-store path: {reason}"),
            Self::UnsupportedFilesystem(reason) => {
                write!(f, "event-store filesystem is unsupported: {reason}")
            }
            Self::WriterAlreadyOwned(path) => {
                write!(f, "writer instance is already owned: {}", path.display())
            }
            Self::GenerationAlreadyOwned(path) => {
                write!(f, "generation is already owned: {}", path.display())
            }
            Self::NoRecoverableHead => write!(f, "no recoverable writer head"),
            Self::HeadConflict(reason) => write!(f, "head conflict: {reason}"),
            Self::PublicationConflict(path) => {
                write!(f, "create-once publication conflicts at {}", path.display())
            }
            Self::InvalidManifest(reason) => write!(f, "invalid generation manifest: {reason}"),
            Self::InvalidHead(reason) => write!(f, "invalid writer head: {reason}"),
            Self::Validation(reason) => write!(f, "generation validation failed: {reason}"),
        }
    }
}

impl std::error::Error for GenerationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json(error) => Some(error),
            Self::Sqlite(error) => Some(error),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for GenerationError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<rusqlite::Error> for GenerationError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}

fn io_error(operation: &'static str, source: io::Error) -> GenerationError {
    GenerationError::Io { operation, source }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationSource {
    NativeProcess,
    LegacyImport { legacy_source_sha256: Digest32 },
    Repair { source_generation_id: GenerationId },
}

/// Immutable generation identity.  These values must exactly match the
/// generation metadata row created in the schema transaction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedManifest {
    pub format_version: u32,
    pub schema_version: i64,
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub predecessor_generation_id: Option<GenerationId>,
    pub database_relative_path: String,
    pub created_at_unix_micros: i64,
    pub head_epoch: i64,
    pub source: GenerationSource,
    pub producer: ProducerIdentity,
}

impl PreparedManifest {
    pub fn validate(&self) -> Result<(), GenerationError> {
        if self.format_version != EVENT_STORE_FORMAT_VERSION
            || self.schema_version != EVENT_SCHEMA_VERSION
        {
            return Err(GenerationError::InvalidManifest(format!(
                "unsupported format/schema {}/{}",
                self.format_version, self.schema_version
            )));
        }
        if self.database_relative_path != DATABASE_FILE_NAME {
            return Err(GenerationError::InvalidManifest(
                "database path is not the fixed relative name".to_owned(),
            ));
        }
        if self.created_at_unix_micros < 0 || self.head_epoch < 0 {
            return Err(GenerationError::InvalidManifest(
                "creation time and head epoch must be non-negative".to_owned(),
            ));
        }
        if self.writer_instance_id.is_nil() || self.generation_id.is_nil() {
            return Err(GenerationError::InvalidManifest(
                "prepared manifest identities must be non-nil".to_owned(),
            ));
        }
        if self.producer.writer_instance_id != self.writer_instance_id {
            return Err(GenerationError::InvalidManifest(
                "manifest producer/writer identities disagree".to_owned(),
            ));
        }
        self.producer
            .validate()
            .map_err(|error| GenerationError::InvalidManifest(error.to_string()))?;
        match &self.source {
            GenerationSource::NativeProcess => {
                if self.producer.native_process.is_none() {
                    return Err(GenerationError::InvalidManifest(
                        "native generation lacks exact native process identity".to_owned(),
                    ));
                }
            }
            // The source discriminator, deterministic writer ID, and envelope
            // provenance distinguish an import partition from a live writer.
            // Exact native identity from the preserved source remains useful
            // evidence and is therefore not discarded.
            GenerationSource::LegacyImport { .. } => {}
            GenerationSource::Repair { .. } => {}
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, GenerationError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn sha256(&self) -> Result<Digest32, GenerationError> {
        Ok(Digest32::sha256(&self.canonical_bytes()?))
    }

    pub fn generation_metadata(&self) -> Result<super::GenerationMetadata, GenerationError> {
        self.validate()?;
        let metadata = super::GenerationMetadata::prepared(
            self.generation_id,
            self.predecessor_generation_id,
            &self.producer,
            self.created_at_unix_micros,
            self.head_epoch,
        );
        metadata
            .validate()
            .map_err(|error| GenerationError::Validation(error.to_string()))?;
        Ok(metadata)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableFileIdentity {
    pub relative_path: String,
    pub bytes: u64,
    pub sha256: Digest32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedManifest {
    pub format_version: u32,
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub prepared_manifest_sha256: Digest32,
    pub closed_at_unix_micros: i64,
    pub min_recorded_at_unix_micros: Option<i64>,
    pub max_recorded_at_unix_micros: Option<i64>,
    pub min_ingested_at_unix_micros: Option<i64>,
    pub max_ingested_at_unix_micros: Option<i64>,
    pub row_count: u64,
    pub high_water_local_sequence: Option<u64>,
    pub durable_files: Vec<DurableFileIdentity>,
    pub transient_sidecars_observed: Vec<String>,
    pub hold: bool,
}

impl SealedManifest {
    pub fn validate(&self) -> Result<(), GenerationError> {
        if self.format_version != EVENT_STORE_FORMAT_VERSION {
            return Err(GenerationError::InvalidManifest(
                "unsupported sealed-manifest version".to_owned(),
            ));
        }
        if self.closed_at_unix_micros < 0 {
            return Err(GenerationError::InvalidManifest(
                "closed time must be positive".to_owned(),
            ));
        }
        if self.writer_instance_id.is_nil() || self.generation_id.is_nil() {
            return Err(GenerationError::InvalidManifest(
                "sealed manifest identities must be non-nil".to_owned(),
            ));
        }
        let time_bounds_valid = match self.row_count {
            0 => {
                self.high_water_local_sequence.is_none()
                    && self.min_recorded_at_unix_micros.is_none()
                    && self.max_recorded_at_unix_micros.is_none()
                    && self.min_ingested_at_unix_micros.is_none()
                    && self.max_ingested_at_unix_micros.is_none()
            }
            rows => {
                self.high_water_local_sequence == Some(rows)
                    && self
                        .min_recorded_at_unix_micros
                        .zip(self.max_recorded_at_unix_micros)
                        .is_some_and(|(min, max)| min >= 0 && min <= max)
                    && self
                        .min_ingested_at_unix_micros
                        .zip(self.max_ingested_at_unix_micros)
                        .is_some_and(|(min, max)| min >= 0 && min <= max)
            }
        };
        if !time_bounds_valid {
            return Err(GenerationError::InvalidManifest(
                "sealed row/high-water/time facts are inconsistent".to_owned(),
            ));
        }
        for file in &self.durable_files {
            if file.relative_path.contains('/')
                || file.relative_path.contains('\\')
                || file.relative_path == "."
                || file.relative_path == ".."
            {
                return Err(GenerationError::InvalidManifest(
                    "invalid sealed file inventory".to_owned(),
                ));
            }
        }
        let database_count = self
            .durable_files
            .iter()
            .filter(|file| file.relative_path == DATABASE_FILE_NAME)
            .count();
        let wal_name = format!("{DATABASE_FILE_NAME}-wal");
        let wal_count = self
            .durable_files
            .iter()
            .filter(|file| file.relative_path == wal_name)
            .count();
        if database_count != 1
            || wal_count > 1
            || self.durable_files.iter().any(|file| {
                file.relative_path != DATABASE_FILE_NAME && file.relative_path != wal_name
            })
        {
            return Err(GenerationError::InvalidManifest(
                "sealed inventory requires exactly one database and at most one WAL".to_owned(),
            ));
        }
        let shm_name = format!("{DATABASE_FILE_NAME}-shm");
        if self.transient_sidecars_observed.len() > 1
            || self
                .transient_sidecars_observed
                .iter()
                .any(|name| name != &shm_name)
        {
            return Err(GenerationError::InvalidManifest(
                "sealed transient sidecar inventory is invalid".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HeadRecordBody {
    format_version: u32,
    epoch: i64,
    writer_instance_id: WriterInstanceId,
    generation_id: GenerationId,
    prepared_manifest_sha256: Digest32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeadRecord {
    pub format_version: u32,
    pub epoch: i64,
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub prepared_manifest_sha256: Digest32,
    pub checksum_sha256: Digest32,
}

impl HeadRecord {
    pub fn new(
        epoch: i64,
        writer_instance_id: WriterInstanceId,
        generation_id: GenerationId,
        prepared_manifest_sha256: Digest32,
    ) -> Result<Self, GenerationError> {
        let mut record = Self {
            format_version: EVENT_STORE_FORMAT_VERSION,
            epoch,
            writer_instance_id,
            generation_id,
            prepared_manifest_sha256,
            checksum_sha256: Digest32::from_bytes([0; 32]),
        };
        record.checksum_sha256 = record.expected_checksum()?;
        record.validate()?;
        Ok(record)
    }

    fn body(&self) -> HeadRecordBody {
        HeadRecordBody {
            format_version: self.format_version,
            epoch: self.epoch,
            writer_instance_id: self.writer_instance_id,
            generation_id: self.generation_id,
            prepared_manifest_sha256: self.prepared_manifest_sha256,
        }
    }

    pub fn expected_checksum(&self) -> Result<Digest32, GenerationError> {
        Ok(Digest32::sha256(&serde_json::to_vec(&self.body())?))
    }

    pub fn validate(&self) -> Result<(), GenerationError> {
        if self.format_version != EVENT_STORE_FORMAT_VERSION {
            return Err(GenerationError::InvalidHead(
                "unsupported head version".to_owned(),
            ));
        }
        if self.epoch < 0 {
            return Err(GenerationError::InvalidHead(
                "head epoch must be non-negative".to_owned(),
            ));
        }
        if self.writer_instance_id.is_nil() || self.generation_id.is_nil() {
            return Err(GenerationError::InvalidHead(
                "head identities must be non-nil".to_owned(),
            ));
        }
        if self.expected_checksum()? != self.checksum_sha256 {
            return Err(GenerationError::InvalidHead(
                "head checksum mismatch".to_owned(),
            ));
        }
        Ok(())
    }

    fn canonical_bytes(&self) -> Result<Vec<u8>, GenerationError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeadSlot {
    Zero,
    One,
}

impl HeadSlot {
    fn other(self) -> Self {
        match self {
            Self::Zero => Self::One,
            Self::One => Self::Zero,
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::Zero => "HEAD.0",
            Self::One => "HEAD.1",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeadCoverageIssue {
    SlotMissing { slot: HeadSlot },
    SlotInvalid { slot: HeadSlot, reason: String },
    HeadPublicationIncomplete { invalid_slot: HeadSlot },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedHead {
    pub slot: HeadSlot,
    pub record: HeadRecord,
    pub issues: Vec<HeadCoverageIssue>,
}

#[derive(Debug)]
enum SlotRead {
    Missing,
    Invalid {
        epoch_hint: Option<i64>,
        reason: String,
    },
    Valid(HeadRecord),
}

#[derive(Clone, Debug)]
pub struct WriterLayout {
    event_store_root: PathBuf,
    writer_instance_id: WriterInstanceId,
    writer_dir: PathBuf,
    generations_dir: PathBuf,
    staging_dir: PathBuf,
    leases_dir: PathBuf,
}

impl WriterLayout {
    pub fn create(
        event_store_root: impl AsRef<Path>,
        writer_instance_id: [u8; 16],
    ) -> Result<Self, GenerationError> {
        let event_store_root = event_store_root.as_ref().to_path_buf();
        let writer_instance_id = WriterInstanceId::from_bytes(writer_instance_id);
        let writers = event_store_root.join("writers");
        let writer_dir = writers.join(id_hex(writer_instance_id.as_bytes()));
        let generations_dir = writer_dir.join("generations");
        let staging_dir = writer_dir.join("staging");
        let leases_dir = writer_dir.join("leases");
        for path in [
            &event_store_root,
            &writers,
            &writer_dir,
            &generations_dir,
            &staging_dir,
            &leases_dir,
        ] {
            fs::create_dir_all(path)
                .map_err(|error| io_error("create event-store directory", error))?;
            set_private_directory(path)?;
        }
        require_same_filesystem(&event_store_root, &writer_dir)?;
        require_same_filesystem(&staging_dir, &generations_dir)?;
        sync_dir(&event_store_root)?;
        sync_dir(&writers)?;
        sync_dir(&writer_dir)?;
        Ok(Self {
            event_store_root,
            writer_instance_id,
            writer_dir,
            generations_dir,
            staging_dir,
            leases_dir,
        })
    }

    /// Open an already-published writer layout without creating a directory,
    /// lease, head, or database. Detached maintenance uses this path so a
    /// typo or stale request cannot manufacture historical authority.
    pub fn open_existing(
        event_store_root: impl AsRef<Path>,
        writer_instance_id: [u8; 16],
    ) -> Result<Self, GenerationError> {
        let event_store_root = event_store_root.as_ref().to_path_buf();
        let writer_instance_id = WriterInstanceId::from_bytes(writer_instance_id);
        let writer_dir = event_store_root
            .join("writers")
            .join(id_hex(writer_instance_id.as_bytes()));
        let generations_dir = writer_dir.join("generations");
        let staging_dir = writer_dir.join("staging");
        let leases_dir = writer_dir.join("leases");
        for path in [
            &event_store_root,
            &writer_dir,
            &generations_dir,
            &leases_dir,
        ] {
            if !path
                .try_exists()
                .map_err(|error| io_error("inspect existing writer layout", error))?
                || !path
                    .metadata()
                    .map_err(|error| io_error("inspect existing writer layout", error))?
                    .is_dir()
            {
                return Err(GenerationError::UnsafePath(format!(
                    "missing existing writer layout component {}",
                    path.display()
                )));
            }
        }
        require_same_filesystem(&event_store_root, &writer_dir)?;
        Ok(Self {
            event_store_root,
            writer_instance_id,
            writer_dir,
            generations_dir,
            staging_dir,
            leases_dir,
        })
    }

    pub fn event_store_root(&self) -> &Path {
        &self.event_store_root
    }

    pub fn writer_instance_id(&self) -> WriterInstanceId {
        self.writer_instance_id
    }

    pub fn writer_dir(&self) -> &Path {
        &self.writer_dir
    }

    pub fn generations_dir(&self) -> &Path {
        &self.generations_dir
    }

    pub fn generation_dir(&self, generation_id: GenerationId) -> PathBuf {
        self.generations_dir.join(id_hex(generation_id.as_bytes()))
    }

    pub fn generation_db(&self, generation_id: GenerationId) -> PathBuf {
        self.generation_dir(generation_id).join(DATABASE_FILE_NAME)
    }

    pub fn generation_lease_path(&self, generation_id: GenerationId) -> PathBuf {
        self.leases_dir
            .join(format!("{}.lock", id_hex(generation_id.as_bytes())))
    }

    pub fn acquire_writer_ownership(&self) -> Result<WriterOwnershipGuard, GenerationError> {
        let path = self.writer_dir.join("writer.owner.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| io_error("open writer ownership lock", error))?;
        match <File as fs4::FileExt>::try_lock(&file) {
            Ok(()) => Ok(WriterOwnershipGuard { file }),
            Err(fs4::TryLockError::WouldBlock) => Err(GenerationError::WriterAlreadyOwned(path)),
            Err(fs4::TryLockError::Error(error)) => Err(io_error("lock writer ownership", error)),
        }
    }

    pub fn acquire_generation_lease(
        &self,
        generation_id: GenerationId,
    ) -> Result<GenerationLease, GenerationError> {
        let path = self.generation_lease_path(generation_id);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| io_error("open generation lease", error))?;
        match <File as fs4::FileExt>::try_lock_shared(&file) {
            Ok(()) => Ok(GenerationLease { file, path }),
            Err(fs4::TryLockError::WouldBlock) => {
                Err(GenerationError::GenerationAlreadyOwned(path))
            }
            Err(fs4::TryLockError::Error(error)) => {
                Err(io_error("lock shared writer generation lease", error))
            }
        }
    }

    /// Acquire the exact exclusive lease used by detached maintenance. It is
    /// refused while either the writer or any bounded reader holds a shared
    /// lease for this generation.
    pub fn acquire_generation_maintenance_lease(
        &self,
        generation_id: GenerationId,
    ) -> Result<GenerationMaintenanceLease, GenerationError> {
        let path = self.generation_lease_path(generation_id);
        acquire_generation_maintenance_lease_at(&path)
    }

    /// Acquire a shared lease before a bounded reader opens a generation.
    /// AGE-377's exclusive lease uses the same external lock file, so a
    /// successful exclusive lock proves no reader/writer still owns the path.
    pub fn acquire_generation_reader_lease(
        &self,
        generation_id: GenerationId,
    ) -> Result<GenerationReaderLease, GenerationError> {
        let path = self.generation_lease_path(generation_id);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| io_error("open generation reader lease", error))?;
        match <File as fs4::FileExt>::try_lock_shared(&file) {
            Ok(()) => Ok(GenerationReaderLease { file, path }),
            Err(fs4::TryLockError::WouldBlock) => {
                Err(GenerationError::GenerationAlreadyOwned(path))
            }
            Err(fs4::TryLockError::Error(error)) => {
                Err(io_error("lock shared generation reader lease", error))
            }
        }
    }

    /// Publish a prepared generation from a unique staging directory.
    /// `prepare_database` must close every handle before returning.
    pub fn publish_prepared_generation<F, V>(
        &self,
        manifest: &PreparedManifest,
        prepare_database: F,
        validate_database: V,
    ) -> Result<PublishedGeneration, GenerationError>
    where
        F: FnOnce(&Path) -> Result<(), GenerationError>,
        V: Fn(&Path, &PreparedManifest) -> Result<(), GenerationError>,
    {
        manifest.validate()?;
        if manifest.writer_instance_id != self.writer_instance_id {
            return Err(GenerationError::InvalidManifest(
                "manifest writer does not match writer directory".to_owned(),
            ));
        }
        let final_path = self.generation_dir(manifest.generation_id);
        if final_path
            .try_exists()
            .map_err(|error| io_error("inspect final generation", error))?
        {
            let on_disk = read_prepared_manifest(&final_path)?;
            if &on_disk != manifest {
                return Err(GenerationError::PublicationConflict(final_path));
            }
            let final_db = final_path.join(DATABASE_FILE_NAME);
            validate_database(&final_db, manifest)?;
            // A caller may be recovering after the create-once rename became
            // visible but before its parent-directory sync completed.
            sync_dir(&self.generations_dir)?;
            // Discovery is a derived, per-generation journal. An existing
            // authoritative generation must never become a failed publication
            // merely because its scheduling evidence needs repair.
            let _ = super::maintenance_discovery::register_legacy_generation(
                &self.event_store_root,
                self.writer_instance_id,
                manifest.generation_id,
                super::maintenance_discovery::DiscoveryPhase::Prepared,
                None,
                Some(manifest),
            );
            return Ok(PublishedGeneration {
                generation_dir: final_path,
                database_path: final_db,
                prepared_manifest_sha256: manifest.sha256()?,
                already_published: true,
            });
        }
        let nonce = Uuid::new_v4().simple().to_string();
        let staging_path = self.staging_dir.join(format!(
            "{}.{}.tmp",
            id_hex(manifest.generation_id.as_bytes()),
            nonce
        ));
        let staging_name = staging_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| GenerationError::UnsafePath("invalid staging name".to_owned()))?;
        // This exact journal is durable before the first staging side effect.
        // It is isolated by generation identity and cannot contend with an
        // unrelated producer's publication.
        super::maintenance_discovery::publish_generation_intent(
            &self.event_store_root,
            manifest,
            staging_name,
        )
        .map_err(GenerationError::Validation)?;
        fs::create_dir(&staging_path)
            .map_err(|error| io_error("create unique generation staging directory", error))?;
        set_private_directory(&staging_path)?;
        let _ = super::maintenance_discovery::record_generation_phase(
            &self.event_store_root,
            self.writer_instance_id,
            manifest.generation_id,
            super::maintenance_discovery::DiscoveryPhase::Staging,
        );
        let db_path = staging_path.join(DATABASE_FILE_NAME);
        prepare_database(&db_path)?;
        sync_file(&db_path)?;
        let wal_path = staging_path.join(format!("{DATABASE_FILE_NAME}-wal"));
        if wal_path
            .try_exists()
            .map_err(|error| io_error("inspect staged WAL", error))?
        {
            sync_file(&wal_path)?;
        }
        publish_create_once_file(
            &staging_path,
            PREPARED_MANIFEST_FILE_NAME,
            &manifest.canonical_bytes()?,
        )?;
        sync_dir(&staging_path)?;
        validate_database(&db_path, manifest)?;

        let _ = super::maintenance_discovery::record_generation_phase(
            &self.event_store_root,
            self.writer_instance_id,
            manifest.generation_id,
            super::maintenance_discovery::DiscoveryPhase::Publishing,
        );
        rename_create_once(&staging_path, &final_path)?;
        sync_dir(&self.staging_dir)?;
        sync_dir(&self.generations_dir)?;
        let final_db = final_path.join(DATABASE_FILE_NAME);
        let on_disk = read_prepared_manifest(&final_path)?;
        if &on_disk != manifest {
            return Err(GenerationError::PublicationConflict(final_path));
        }
        validate_database(&final_db, manifest)?;
        // The authoritative publication is already durable. A derived journal
        // update may lag, but the pre-published intent remains an exact bounded
        // recovery source and must not turn success into a generic failure.
        let _ = super::maintenance_discovery::record_generation_phase(
            &self.event_store_root,
            self.writer_instance_id,
            manifest.generation_id,
            super::maintenance_discovery::DiscoveryPhase::Prepared,
        );
        Ok(PublishedGeneration {
            generation_dir: final_path,
            database_path: final_db,
            prepared_manifest_sha256: manifest.sha256()?,
            already_published: false,
        })
    }

    pub fn read_head<V>(&self, validate: V) -> Result<Option<SelectedHead>, GenerationError>
    where
        V: Fn(&Path, &HeadRecord) -> Result<(), GenerationError>,
    {
        let slots = [HeadSlot::Zero, HeadSlot::One];
        let mut reads = [SlotRead::Missing, SlotRead::Missing];
        for (index, slot) in slots.into_iter().enumerate() {
            reads[index] = read_slot(&self.writer_dir.join(slot.file_name()), |record| {
                if record.writer_instance_id != self.writer_instance_id {
                    return Err(GenerationError::InvalidHead(
                        "head writer identity does not match directory".to_owned(),
                    ));
                }
                let generation_dir = self.generation_dir(record.generation_id);
                let bytes =
                    read_bounded_control_file(&generation_dir.join(PREPARED_MANIFEST_FILE_NAME))?;
                if Digest32::sha256(&bytes) != record.prepared_manifest_sha256 {
                    return Err(GenerationError::InvalidHead(
                        "prepared-manifest digest mismatch".to_owned(),
                    ));
                }
                let manifest: PreparedManifest = serde_json::from_slice(&bytes)?;
                manifest.validate()?;
                if manifest.writer_instance_id != record.writer_instance_id
                    || manifest.generation_id != record.generation_id
                    || manifest.head_epoch != record.epoch
                {
                    return Err(GenerationError::InvalidHead(
                        "head and prepared manifest identities disagree".to_owned(),
                    ));
                }
                validate(&generation_dir.join(DATABASE_FILE_NAME), record)
            });
        }
        select_head(reads)
    }

    /// Publish into the inactive slot.  If an inactive record exists, only
    /// that record is removed while the selected slot remains durable.
    pub fn publish_head(
        &self,
        selected_slot: Option<HeadSlot>,
        record: &HeadRecord,
    ) -> Result<HeadSlot, GenerationError> {
        record.validate()?;
        if record.writer_instance_id != self.writer_instance_id {
            return Err(GenerationError::InvalidHead(
                "head writer identity does not match directory".to_owned(),
            ));
        }
        if let Some(active) = selected_slot {
            let active_path = self.writer_dir.join(active.file_name());
            let active_record = match read_slot(&active_path, |_| Ok(())) {
                SlotRead::Valid(active_record) => active_record,
                _ => {
                    return Err(GenerationError::HeadConflict(
                        "selected active slot is not valid".to_owned(),
                    ));
                }
            };
            if record.epoch != active_record.epoch + 1 {
                return Err(GenerationError::HeadConflict(
                    "new head epoch is not the exact successor epoch".to_owned(),
                ));
            }
        } else if record.epoch != 0 {
            return Err(GenerationError::HeadConflict(
                "initial head epoch must be zero".to_owned(),
            ));
        }
        let target_slot = selected_slot.map_or(HeadSlot::Zero, HeadSlot::other);
        let target = self.writer_dir.join(target_slot.file_name());
        if target
            .try_exists()
            .map_err(|error| io_error("inspect inactive head", error))?
        {
            if selected_slot.is_none() {
                return Err(GenerationError::PublicationConflict(target));
            }
            match read_slot(&target, |_| Ok(())) {
                SlotRead::Valid(existing) if existing == *record => {
                    // Complete a possibly interrupted publication. Visibility
                    // alone is not proof that the directory entry is durable.
                    sync_dir(&self.writer_dir)?;
                    return Ok(target_slot);
                }
                SlotRead::Valid(existing) if existing.epoch >= record.epoch => {
                    return Err(GenerationError::PublicationConflict(target));
                }
                SlotRead::Valid(_) | SlotRead::Invalid { .. } | SlotRead::Missing => {}
            }
            fs::remove_file(&target).map_err(|error| io_error("remove inactive head", error))?;
            sync_dir(&self.writer_dir)?;
        }
        let temp_name = format!(
            "{}.{}.tmp",
            target_slot.file_name(),
            Uuid::new_v4().simple()
        );
        let temp = self.writer_dir.join(temp_name);
        write_new_synced(&temp, &record.canonical_bytes()?)?;
        rename_create_once(&temp, &target)?;
        sync_dir(&self.writer_dir)?;
        Ok(target_slot)
    }
}

#[derive(Debug)]
pub struct WriterOwnershipGuard {
    file: File,
}

impl Drop for WriterOwnershipGuard {
    fn drop(&mut self) {
        let _ = <File as fs4::FileExt>::unlock(&self.file);
    }
}

#[derive(Debug)]
pub struct GenerationLease {
    file: File,
    path: PathBuf,
}

#[derive(Debug)]
pub struct GenerationReaderLease {
    file: File,
    path: PathBuf,
}

#[derive(Debug)]
pub struct GenerationMaintenanceLease {
    file: File,
    path: PathBuf,
}

/// Acquire an exact existing reader lease without creating a writer layout or
/// any filesystem object. Missing lease evidence is an explicit error rather
/// than permission to race retirement.
pub fn acquire_generation_reader_lease_at(
    lease_path: &Path,
) -> Result<GenerationReaderLease, GenerationError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(lease_path)
        .map_err(|error| io_error("open existing generation reader lease", error))?;
    match <File as fs4::FileExt>::try_lock_shared(&file) {
        Ok(()) => Ok(GenerationReaderLease {
            file,
            path: lease_path.to_path_buf(),
        }),
        Err(fs4::TryLockError::WouldBlock) => Err(GenerationError::GenerationAlreadyOwned(
            lease_path.to_path_buf(),
        )),
        Err(fs4::TryLockError::Error(error)) => {
            Err(io_error("lock existing generation reader lease", error))
        }
    }
}

pub fn acquire_generation_maintenance_lease_at(
    lease_path: &Path,
) -> Result<GenerationMaintenanceLease, GenerationError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(lease_path)
        .map_err(|error| io_error("open existing generation maintenance lease", error))?;
    match <File as fs4::FileExt>::try_lock(&file) {
        Ok(()) => Ok(GenerationMaintenanceLease {
            file,
            path: lease_path.to_path_buf(),
        }),
        Err(fs4::TryLockError::WouldBlock) => Err(GenerationError::GenerationAlreadyOwned(
            lease_path.to_path_buf(),
        )),
        Err(fs4::TryLockError::Error(error)) => Err(io_error(
            "lock exclusive generation maintenance lease",
            error,
        )),
    }
}

impl GenerationMaintenanceLease {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for GenerationMaintenanceLease {
    fn drop(&mut self) {
        let _ = <File as fs4::FileExt>::unlock(&self.file);
    }
}

impl GenerationReaderLease {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for GenerationReaderLease {
    fn drop(&mut self) {
        let _ = <File as fs4::FileExt>::unlock(&self.file);
    }
}

impl GenerationLease {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for GenerationLease {
    fn drop(&mut self) {
        let _ = <File as fs4::FileExt>::unlock(&self.file);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedGeneration {
    pub generation_dir: PathBuf,
    pub database_path: PathBuf,
    pub prepared_manifest_sha256: Digest32,
    pub already_published: bool,
}

pub fn read_prepared_manifest(generation_dir: &Path) -> Result<PreparedManifest, GenerationError> {
    let bytes = read_bounded_control_file(&generation_dir.join(PREPARED_MANIFEST_FILE_NAME))?;
    let manifest: PreparedManifest = serde_json::from_slice(&bytes)?;
    manifest.validate()?;
    if Digest32::sha256(&bytes) != manifest.sha256()? {
        return Err(GenerationError::InvalidManifest(
            "prepared manifest is not canonically encoded".to_owned(),
        ));
    }
    Ok(manifest)
}

fn publish_sealed_manifest(
    generation_dir: &Path,
    manifest: &SealedManifest,
) -> Result<Digest32, GenerationError> {
    manifest.validate()?;
    let expected_dir_name = id_hex(manifest.generation_id.as_bytes());
    if generation_dir.file_name().and_then(|value| value.to_str()) != Some(&expected_dir_name) {
        return Err(GenerationError::InvalidManifest(
            "sealed manifest generation does not match directory".to_owned(),
        ));
    }
    let prepared = read_prepared_manifest(generation_dir)?;
    if prepared.writer_instance_id != manifest.writer_instance_id
        || prepared.generation_id != manifest.generation_id
        || prepared.sha256()? != manifest.prepared_manifest_sha256
    {
        return Err(GenerationError::InvalidManifest(
            "sealed manifest identity does not match prepared manifest".to_owned(),
        ));
    }
    // The caller has just derived this manifest under the exact exclusive
    // generation lease: checkpoint_closed_generation_under_lease performed
    // the single SQLite quick_check, and seal_closed_generation_under_lease
    // read the aggregate facts and hashed each immutable durable file once.
    // Repeating those whole-generation traversals here would not strengthen
    // the same lease-held observation. Retirement independently revalidates
    // the published identities immediately before destructive eligibility.
    let mut bytes = serde_json::to_vec(manifest)?;
    bytes.push(b'\n');
    publish_create_once_file(generation_dir, SEALED_MANIFEST_FILE_NAME, &bytes)?;
    sync_dir(generation_dir)?;
    Ok(Digest32::sha256(&bytes))
}

/// Materialize and publish exact eligibility evidence for one already-closed
/// generation. This is a detached AGE-377 fixture/primitive: the live writer
/// never calls it during rotation; the helper acquires the exact exclusive
/// generation lease, while callers remain responsible for singleton job
/// scheduling. It does not enumerate history, checkpoint, compact, repair,
/// quarantine, or delete anything.
pub fn seal_closed_generation(
    generation_dir: &Path,
    hold: bool,
) -> Result<(SealedManifest, Digest32), GenerationError> {
    let prepared = read_prepared_manifest(generation_dir)?;
    let writer_dir = generation_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| GenerationError::UnsafePath(generation_dir.display().to_string()))?;
    if writer_dir.file_name().and_then(|value| value.to_str())
        != Some(&id_hex(prepared.writer_instance_id.as_bytes()))
    {
        return Err(GenerationError::InvalidManifest(
            "prepared manifest writer does not match its writer directory".to_owned(),
        ));
    }
    let lease_path = writer_dir.join("leases").join(format!(
        "{}.lock",
        id_hex(prepared.generation_id.as_bytes())
    ));
    let maintenance_lease = acquire_generation_maintenance_lease_at(&lease_path)?;
    seal_closed_generation_under_lease(generation_dir, hold, &maintenance_lease)
}

/// Finish the create-once seal while the caller retains the exact exclusive
/// lease. This prevents a checkpoint/seal worker from dropping ownership
/// between validation and immutable manifest publication.
pub fn seal_closed_generation_under_lease(
    generation_dir: &Path,
    hold: bool,
    maintenance_lease: &GenerationMaintenanceLease,
) -> Result<(SealedManifest, Digest32), GenerationError> {
    let prepared = read_prepared_manifest(generation_dir)?;
    let writer_dir = generation_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| GenerationError::UnsafePath(generation_dir.display().to_string()))?;
    let expected_lease = writer_dir.join("leases").join(format!(
        "{}.lock",
        id_hex(prepared.generation_id.as_bytes())
    ));
    if maintenance_lease.path() != expected_lease {
        return Err(GenerationError::Validation(
            "seal does not hold the exact generation maintenance lease".to_owned(),
        ));
    }
    let wal_path = generation_dir.join(format!("{DATABASE_FILE_NAME}-wal"));
    match fs::metadata(&wal_path) {
        Ok(metadata) if metadata.len() > 0 => {
            return Err(GenerationError::Validation(
                "sealing requires a completed detached TRUNCATE checkpoint".to_owned(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error("inspect checkpointed WAL before sealing", error)),
    }
    let connection = rusqlite::Connection::open_with_flags(
        generation_dir.join(DATABASE_FILE_NAME),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    let metadata = super::read_generation_metadata(&connection)
        .map_err(|error| GenerationError::Validation(error.to_string()))?;
    if metadata.state != super::GenerationState::Closed
        || metadata.writer_instance_id != prepared.writer_instance_id
        || metadata.generation_id != prepared.generation_id
    {
        return Err(GenerationError::Validation(
            "only an exactly identified closed generation may be sealed".to_owned(),
        ));
    }
    let facts: GenerationAggregateFacts = connection.query_row(
        "SELECT count(*), max(local_sequence),
                    min(recorded_at_unix_micros), max(recorded_at_unix_micros),
                    min(ingested_at_unix_micros), max(ingested_at_unix_micros)
               FROM events",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        },
    )?;
    drop(connection);

    let mut durable_files = Vec::new();
    for relative_path in [
        DATABASE_FILE_NAME.to_owned(),
        format!("{DATABASE_FILE_NAME}-wal"),
    ] {
        let path = generation_dir.join(&relative_path);
        match fs::metadata(&path) {
            Ok(metadata) => {
                sync_file(&path)?;
                durable_files.push(DurableFileIdentity {
                    relative_path,
                    bytes: metadata.len(),
                    sha256: hash_file(&path)?,
                });
            }
            Err(error)
                if error.kind() == io::ErrorKind::NotFound && relative_path.ends_with("-wal") => {}
            Err(error) => return Err(io_error("inspect closed generation inventory", error)),
        }
    }
    let shm_name = format!("{DATABASE_FILE_NAME}-shm");
    let transient_sidecars_observed = if generation_dir
        .join(&shm_name)
        .try_exists()
        .map_err(|error| io_error("inspect closed generation sidecar", error))?
    {
        vec![shm_name]
    } else {
        Vec::new()
    };
    sync_dir(generation_dir)?;
    let manifest = SealedManifest {
        format_version: EVENT_STORE_FORMAT_VERSION,
        writer_instance_id: prepared.writer_instance_id,
        generation_id: prepared.generation_id,
        prepared_manifest_sha256: prepared.sha256()?,
        closed_at_unix_micros: metadata.closed_at_unix_micros.ok_or_else(|| {
            GenerationError::Validation("closed generation lacks close time".to_owned())
        })?,
        min_recorded_at_unix_micros: facts.2,
        max_recorded_at_unix_micros: facts.3,
        min_ingested_at_unix_micros: facts.4,
        max_ingested_at_unix_micros: facts.5,
        row_count: u64::try_from(facts.0).map_err(|_| {
            GenerationError::Validation("negative closed generation row count".to_owned())
        })?,
        high_water_local_sequence: facts.1.and_then(|value| u64::try_from(value).ok()),
        durable_files,
        transient_sidecars_observed,
        hold,
    };
    let digest = publish_sealed_manifest(generation_dir, &manifest)?;
    Ok((manifest, digest))
}

/// Checkpoint one exact closed generation under its exclusive maintenance
/// lease. No handle escapes this function and no head or writer connection is
/// opened. A busy or incomplete checkpoint fails closed.
pub fn checkpoint_closed_generation_under_lease(
    generation_dir: &Path,
    maintenance_lease: &GenerationMaintenanceLease,
) -> Result<(), GenerationError> {
    let prepared = read_prepared_manifest(generation_dir)?;
    let writer_dir = generation_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| GenerationError::UnsafePath(generation_dir.display().to_string()))?;
    let expected_lease = writer_dir.join("leases").join(format!(
        "{}.lock",
        id_hex(prepared.generation_id.as_bytes())
    ));
    if maintenance_lease.path() != expected_lease {
        return Err(GenerationError::Validation(
            "checkpoint does not hold the exact generation maintenance lease".to_owned(),
        ));
    }
    let database = generation_dir.join(DATABASE_FILE_NAME);
    let connection = rusqlite::Connection::open_with_flags(
        &database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )?;
    let metadata = super::read_generation_metadata(&connection)
        .map_err(|error| GenerationError::Validation(error.to_string()))?;
    if metadata.state != super::GenerationState::Closed
        || metadata.writer_instance_id != prepared.writer_instance_id
        || metadata.generation_id != prepared.generation_id
    {
        return Err(GenerationError::Validation(
            "checkpoint target is not the exact closed generation".to_owned(),
        ));
    }
    let checkpoint: (i64, i64, i64) =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if checkpoint.0 != 0 || checkpoint.1 != checkpoint.2 {
        return Err(GenerationError::Validation(format!(
            "closed generation checkpoint incomplete: {checkpoint:?}"
        )));
    }
    let quick_check: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if quick_check != "ok" {
        return Err(GenerationError::Validation(format!(
            "quick_check failed before seal: {quick_check}"
        )));
    }
    drop(connection);
    sync_file(&database)?;
    let wal = generation_dir.join(format!("{DATABASE_FILE_NAME}-wal"));
    if wal
        .try_exists()
        .map_err(|error| io_error("inspect checkpointed WAL", error))?
    {
        sync_file(&wal)?;
    }
    sync_dir(generation_dir)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementReceipt {
    pub format_version: u32,
    pub writer_instance_id: WriterInstanceId,
    pub generation_id: GenerationId,
    pub prepared_manifest_sha256: Digest32,
    pub sealed_manifest_sha256: Digest32,
    pub retention_policy_version: String,
    pub retention_cutoff_unix_micros: i64,
    pub original_relative_path: String,
    pub pending_trash_relative_path: String,
    pub retirement_epoch: i64,
}

impl RetirementReceipt {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        writer_instance_id: WriterInstanceId,
        generation_id: GenerationId,
        prepared_manifest_sha256: Digest32,
        sealed_manifest_sha256: Digest32,
        retention_policy_version: String,
        retention_cutoff_unix_micros: i64,
        retirement_epoch: i64,
    ) -> Self {
        let writer = id_hex(writer_instance_id.as_bytes());
        let generation = id_hex(generation_id.as_bytes());
        Self {
            format_version: EVENT_STORE_FORMAT_VERSION,
            writer_instance_id,
            generation_id,
            prepared_manifest_sha256,
            sealed_manifest_sha256,
            retention_policy_version,
            retention_cutoff_unix_micros,
            original_relative_path: format!("writers/{writer}/generations/{generation}"),
            pending_trash_relative_path: format!("retirement/trash/{writer}/{generation}.pending"),
            retirement_epoch,
        }
    }

    pub fn validate(&self) -> Result<(), GenerationError> {
        if self.format_version != EVENT_STORE_FORMAT_VERSION
            || self.retention_cutoff_unix_micros < 0
            || self.retirement_epoch < 0
            || self.writer_instance_id.is_nil()
            || self.generation_id.is_nil()
        {
            return Err(GenerationError::InvalidManifest(
                "invalid retirement receipt version/timestamp/epoch".to_owned(),
            ));
        }
        if self.retention_policy_version.is_empty()
            || self.retention_policy_version.len() > 64
            || !self
                .retention_policy_version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(GenerationError::InvalidManifest(
                "invalid retention policy version".to_owned(),
            ));
        }
        let writer = id_hex(self.writer_instance_id.as_bytes());
        let generation = id_hex(self.generation_id.as_bytes());
        let expected_original = format!("writers/{writer}/generations/{generation}");
        let expected_pending = format!("retirement/trash/{writer}/{generation}.pending");
        if self.original_relative_path != expected_original
            || self.pending_trash_relative_path != expected_pending
        {
            return Err(GenerationError::InvalidManifest(
                "retirement paths are not the deterministic exact paths".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, GenerationError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec(self)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// Publish an AGE-377-produced receipt after the directory move. This helper
/// only publishes the validated create-once receipt; it never moves or deletes
/// a generation or pending-trash directory.
pub fn publish_retirement_receipt(
    event_store_root: &Path,
    receipt: &RetirementReceipt,
) -> Result<PathBuf, GenerationError> {
    receipt.validate()?;
    let original = event_store_root.join(&receipt.original_relative_path);
    if original
        .try_exists()
        .map_err(|error| io_error("inspect retired generation source", error))?
    {
        return Err(GenerationError::Validation(
            "retirement receipt cannot precede the exact directory move".to_owned(),
        ));
    }
    let pending = event_store_root.join(&receipt.pending_trash_relative_path);
    if !pending
        .try_exists()
        .map_err(|error| io_error("inspect pending retirement directory", error))?
    {
        return Err(GenerationError::Validation(
            "pending retirement directory is absent".to_owned(),
        ));
    }
    let prepared_bytes = read_bounded_control_file(&pending.join(PREPARED_MANIFEST_FILE_NAME))?;
    let sealed_bytes = read_bounded_control_file(&pending.join(SEALED_MANIFEST_FILE_NAME))?;
    let prepared: PreparedManifest = serde_json::from_slice(&prepared_bytes)?;
    let sealed: SealedManifest = serde_json::from_slice(&sealed_bytes)?;
    prepared.validate()?;
    sealed.validate()?;
    if prepared.writer_instance_id != receipt.writer_instance_id
        || prepared.generation_id != receipt.generation_id
        || sealed.writer_instance_id != receipt.writer_instance_id
        || sealed.generation_id != receipt.generation_id
        || Digest32::sha256(&prepared_bytes) != receipt.prepared_manifest_sha256
        || Digest32::sha256(&sealed_bytes) != receipt.sealed_manifest_sha256
    {
        return Err(GenerationError::Validation(
            "retirement receipt manifest identity/digest mismatch".to_owned(),
        ));
    }
    let writer = id_hex(receipt.writer_instance_id.as_bytes());
    let generation = id_hex(receipt.generation_id.as_bytes());
    let retirement = event_store_root.join("retirement");
    let receipts = retirement.join("receipts");
    let writer_receipts = receipts.join(writer);
    for path in [&retirement, &receipts, &writer_receipts] {
        fs::create_dir_all(path)
            .map_err(|error| io_error("create retirement receipt directory", error))?;
        set_private_directory(path)?;
    }
    require_same_filesystem(event_store_root, &writer_receipts)?;
    require_same_filesystem(event_store_root, &pending)?;
    let file_name = format!("{generation}.json");
    publish_create_once_file(&writer_receipts, &file_name, &receipt.canonical_bytes()?)?;
    sync_dir(&writer_receipts)?;
    sync_dir(&receipts)?;
    sync_dir(&retirement)?;
    Ok(writer_receipts.join(file_name))
}

fn hash_file(path: &Path) -> Result<Digest32, GenerationError> {
    use sha2::Digest as _;
    use std::io::Read;
    let mut file =
        File::open(path).map_err(|error| io_error("open durable file for digest", error))?;
    let mut digest = sha2::Sha256::new();
    let mut buffer = [0_u8; SEALED_FILE_DIGEST_BUFFER_BYTES];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| io_error("read durable file for digest", error))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(Digest32::from_bytes(digest.finalize().into()))
}

fn read_slot<V>(path: &Path, validate: V) -> SlotRead
where
    V: FnOnce(&HeadRecord) -> Result<(), GenerationError>,
{
    let bytes = match read_bounded_control_file(path) {
        Ok(bytes) => bytes,
        Err(GenerationError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            return SlotRead::Missing;
        }
        Err(error) => {
            return SlotRead::Invalid {
                epoch_hint: None,
                reason: format!("read failed: {error}"),
            };
        }
    };
    let epoch_hint = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| value.get("epoch")?.as_i64());
    let record = match serde_json::from_slice::<HeadRecord>(&bytes) {
        Ok(record) => record,
        Err(error) => {
            return SlotRead::Invalid {
                epoch_hint,
                reason: format!("decode failed: {error}"),
            };
        }
    };
    if let Err(error) = record.validate().and_then(|()| validate(&record)) {
        return SlotRead::Invalid {
            epoch_hint,
            reason: error.to_string(),
        };
    }
    SlotRead::Valid(record)
}

fn select_head(reads: [SlotRead; 2]) -> Result<Option<SelectedHead>, GenerationError> {
    let slot_for = |index| {
        if index == 0 {
            HeadSlot::Zero
        } else {
            HeadSlot::One
        }
    };
    let valid: Vec<_> = reads
        .iter()
        .enumerate()
        .filter_map(|(index, read)| match read {
            SlotRead::Valid(record) => Some((slot_for(index), record)),
            _ => None,
        })
        .collect();
    if valid.is_empty() {
        if reads.iter().all(|read| matches!(read, SlotRead::Missing)) {
            return Ok(None);
        }
        return Err(GenerationError::NoRecoverableHead);
    }
    let (selected_slot, selected_record) = if valid.len() == 2 {
        let (slot_a, a) = valid[0];
        let (slot_b, b) = valid[1];
        if a.epoch == b.epoch && a != b {
            return Err(GenerationError::HeadConflict(format!(
                "equal epoch {} names different generations",
                a.epoch
            )));
        }
        if a.epoch >= b.epoch {
            (slot_a, a.clone())
        } else {
            (slot_b, b.clone())
        }
    } else {
        (valid[0].0, valid[0].1.clone())
    };
    let mut issues = Vec::new();
    for (index, read) in reads.iter().enumerate() {
        let slot = slot_for(index);
        match read {
            SlotRead::Missing => issues.push(HeadCoverageIssue::SlotMissing { slot }),
            SlotRead::Invalid { epoch_hint, reason } => {
                issues.push(HeadCoverageIssue::SlotInvalid {
                    slot,
                    reason: reason.clone(),
                });
                if epoch_hint.is_none_or(|epoch| epoch > selected_record.epoch) {
                    issues
                        .push(HeadCoverageIssue::HeadPublicationIncomplete { invalid_slot: slot });
                }
            }
            SlotRead::Valid(_) => {}
        }
    }
    Ok(Some(SelectedHead {
        slot: selected_slot,
        record: selected_record,
        issues,
    }))
}

fn publish_create_once_file(
    parent: &Path,
    file_name: &str,
    bytes: &[u8],
) -> Result<(), GenerationError> {
    if Path::new(file_name).components().count() != 1 {
        return Err(GenerationError::UnsafePath(file_name.to_owned()));
    }
    let target = parent.join(file_name);
    if target
        .try_exists()
        .map_err(|error| io_error("inspect publication target", error))?
    {
        let existing = read_bounded_control_file(&target)?;
        return if existing == bytes {
            // Idempotent recovery also completes the parent-directory sync.
            sync_dir(parent)
        } else {
            Err(GenerationError::PublicationConflict(target))
        };
    }
    let temp = parent.join(format!("{file_name}.{}.tmp", Uuid::new_v4().simple()));
    write_new_synced(&temp, bytes)?;
    rename_create_once(&temp, &target)?;
    sync_dir(parent)
}

fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<(), GenerationError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| io_error("create publication temp file", error))?;
    file.write_all(bytes)
        .map_err(|error| io_error("write publication temp file", error))?;
    file.sync_all()
        .map_err(|error| io_error("sync publication temp file", error))
}

pub(crate) fn read_bounded_control_file(path: &Path) -> Result<Vec<u8>, GenerationError> {
    use std::io::Read;
    let file = File::open(path).map_err(|error| io_error("open control record", error))?;
    let mut bytes = Vec::with_capacity(MAX_GENERATION_CONTROL_RECORD_BYTES.min(4096));
    file.take(MAX_GENERATION_CONTROL_RECORD_BYTES.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("read bounded control record", error))?;
    if bytes.len() > MAX_GENERATION_CONTROL_RECORD_BYTES {
        return Err(GenerationError::Validation(format!(
            "control record exceeds {MAX_GENERATION_CONTROL_RECORD_BYTES} bytes"
        )));
    }
    Ok(bytes)
}

fn sync_file(path: &Path) -> Result<(), GenerationError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| io_error("sync generation file", error))
}

#[cfg(unix)]
fn sync_dir(path: &Path) -> Result<(), GenerationError> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| io_error("sync event-store directory", error))
}

#[cfg(windows)]
fn sync_dir(_path: &Path) -> Result<(), GenerationError> {
    // Windows does not permit FlushFileBuffers on directory handles. Every
    // create-once publication uses MoveFileExW(MOVEFILE_WRITE_THROUGH), which
    // is the platform durability boundary for the rename.
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn sync_dir(path: &Path) -> Result<(), GenerationError> {
    Err(GenerationError::UnsupportedFilesystem(format!(
        "durable directory sync is unavailable for {}",
        path.display()
    )))
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), GenerationError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error("set private event-store permissions", error))
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), GenerationError> {
    Ok(())
}

#[cfg(unix)]
fn require_same_filesystem(left: &Path, right: &Path) -> Result<(), GenerationError> {
    use std::os::unix::fs::MetadataExt;
    let left_device = fs::metadata(left)
        .map_err(|error| io_error("inspect source filesystem", error))?
        .dev();
    let right_device = fs::metadata(right)
        .map_err(|error| io_error("inspect destination filesystem", error))?
        .dev();
    if left_device != right_device {
        return Err(GenerationError::UnsupportedFilesystem(format!(
            "{} and {} are on different devices",
            left.display(),
            right.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn require_same_filesystem(left: &Path, right: &Path) -> Result<(), GenerationError> {
    // A create/rename/rename-back probe proves that the filesystem supplies the
    // exact atomic primitive used by publication.  Failure is explicit and the
    // caller must enter JSONL fallback.
    let nonce = Uuid::new_v4().simple().to_string();
    let source = left.join(format!(".same-filesystem-{nonce}"));
    let destination = right.join(format!(".same-filesystem-{nonce}"));
    write_new_synced(&source, b"event-store-filesystem-probe\n")?;
    rename_create_once(&source, &destination)?;
    rename_create_once(&destination, &source)?;
    fs::remove_file(&source).map_err(|error| io_error("remove filesystem probe", error))?;
    sync_dir(left)?;
    sync_dir(right)
}

#[cfg(target_os = "linux")]
fn rename_create_once(source: &Path, destination: &Path) -> Result<(), GenerationError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let source_c = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| GenerationError::UnsafePath(source.display().to_string()))?;
    let destination_c = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| GenerationError::UnsafePath(destination.display().to_string()))?;
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source_c.as_ptr(),
            libc::AT_FDCWD,
            destination_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::AlreadyExists {
        Err(GenerationError::PublicationConflict(
            destination.to_path_buf(),
        ))
    } else {
        Err(io_error("rename create-once publication", error))
    }
}

#[cfg(target_os = "macos")]
fn rename_create_once(source: &Path, destination: &Path) -> Result<(), GenerationError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let source_c = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| GenerationError::UnsafePath(source.display().to_string()))?;
    let destination_c = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| GenerationError::UnsafePath(destination.display().to_string()))?;
    let result =
        unsafe { libc::renamex_np(source_c.as_ptr(), destination_c.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::AlreadyExists {
        return Err(GenerationError::PublicationConflict(
            destination.to_path_buf(),
        ));
    }
    Err(io_error("rename create-once publication", error))
}

#[cfg(windows)]
fn rename_create_once(source: &Path, destination: &Path) -> Result<(), GenerationError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let source_wide: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let destination_wide: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let result = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if result != 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::AlreadyExists {
        return Err(GenerationError::PublicationConflict(
            destination.to_path_buf(),
        ));
    }
    Err(io_error("rename create-once publication", error))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn rename_create_once(_source: &Path, _destination: &Path) -> Result<(), GenerationError> {
    Err(GenerationError::UnsupportedFilesystem(
        "atomic create-once rename is not implemented on this platform".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn producer(id: u8) -> ProducerIdentity {
        ProducerIdentity {
            writer_instance_id: WriterInstanceId::from_bytes([id; 16]),
            process_instance_id: super::super::ProcessInstanceId::from_bytes([id; 16]),
            process_root_id: super::super::ProcessInstanceId::from_bytes([id; 16]),
            parent_process_instance_id: None,
            supervisor_authority_id: None,
            native_process: Some(super::super::NativeProcessIdentity {
                os_pid: i64::from(std::process::id()),
                os_boot_id_sha256: Digest32::sha256(b"test-boot"),
                os_pid_starttime_ticks: 1,
            }),
        }
    }

    fn manifest(writer: u8, generation: u8, epoch: i64) -> PreparedManifest {
        PreparedManifest {
            format_version: EVENT_STORE_FORMAT_VERSION,
            schema_version: EVENT_SCHEMA_VERSION,
            writer_instance_id: WriterInstanceId::from_bytes([writer; 16]),
            generation_id: GenerationId::from_bytes([generation; 16]),
            predecessor_generation_id: None,
            database_relative_path: DATABASE_FILE_NAME.to_owned(),
            created_at_unix_micros: 1,
            head_epoch: epoch,
            source: GenerationSource::NativeProcess,
            producer: producer(writer),
        }
    }

    #[test]
    fn checksummed_head_rejects_torn_or_modified_content() {
        let writer = WriterInstanceId::from_bytes([1; 16]);
        let generation = GenerationId::from_bytes([2; 16]);
        let record = HeadRecord::new(7, writer, generation, Digest32::sha256(b"manifest")).unwrap();
        record.validate().unwrap();
        let mut modified = record.clone();
        modified.epoch = 8;
        assert!(matches!(
            modified.validate(),
            Err(GenerationError::InvalidHead(_))
        ));
    }

    #[test]
    fn writer_and_reader_share_generation_lease_while_maintenance_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let layout = WriterLayout::create(root.path(), [1; 16]).unwrap();
        let _owner = layout.acquire_writer_ownership().unwrap();
        assert!(matches!(
            layout.acquire_writer_ownership(),
            Err(GenerationError::WriterAlreadyOwned(_))
        ));
        let generation = GenerationId::from_bytes([2; 16]);
        let writer_lease = layout.acquire_generation_lease(generation).unwrap();
        let reader_lease = layout.acquire_generation_reader_lease(generation).unwrap();
        assert!(matches!(
            layout.acquire_generation_maintenance_lease(generation),
            Err(GenerationError::GenerationAlreadyOwned(_))
        ));
        drop(reader_lease);
        drop(writer_lease);
        let _maintenance = layout
            .acquire_generation_maintenance_lease(generation)
            .unwrap();
    }

    #[test]
    fn publication_is_create_once_and_head_uses_inactive_slot() {
        let root = tempfile::tempdir().unwrap();
        let layout = WriterLayout::create(root.path(), [3; 16]).unwrap();
        let writer = WriterInstanceId::from_bytes([3; 16]);
        let first_generation = GenerationId::from_bytes([4; 16]);
        let first_manifest = manifest(3, 4, 0);
        let first_publication = layout
            .publish_prepared_generation(
                &first_manifest,
                |db| {
                    fs::write(db, b"first").map_err(|e| io_error("write test db", e))?;
                    Ok(())
                },
                |db, _| {
                    (fs::read(db).map_err(|e| io_error("read test db", e))? == b"first")
                        .then_some(())
                        .ok_or_else(|| GenerationError::Validation("wrong bytes".into()))
                },
            )
            .unwrap();
        assert!(!first_publication.already_published);
        let repeated = layout
            .publish_prepared_generation(
                &first_manifest,
                |_| panic!("already-published generation must not be rebuilt"),
                |db, _| {
                    (fs::read(db).map_err(|e| io_error("read test db", e))? == b"first")
                        .then_some(())
                        .ok_or_else(|| GenerationError::Validation("wrong bytes".into()))
                },
            )
            .unwrap();
        assert!(repeated.already_published);
        let first_head = HeadRecord::new(
            0,
            writer,
            first_generation,
            first_manifest.sha256().unwrap(),
        )
        .unwrap();
        let first_slot = layout.publish_head(None, &first_head).unwrap();
        assert_eq!(first_slot, HeadSlot::Zero);
        let selected = layout
            .read_head(|db, _| {
                (fs::read(db).map_err(|e| io_error("read test db", e))? == b"first")
                    .then_some(())
                    .ok_or_else(|| GenerationError::Validation("wrong bytes".into()))
            })
            .unwrap()
            .unwrap();
        assert_eq!(selected.record, first_head);
        assert_eq!(selected.slot, HeadSlot::Zero);

        let second_generation = GenerationId::from_bytes([5; 16]);
        let mut second_manifest = manifest(3, 5, 1);
        second_manifest.predecessor_generation_id = Some(first_generation);
        layout
            .publish_prepared_generation(
                &second_manifest,
                |db| {
                    fs::write(db, b"second").map_err(|e| io_error("write test db", e))?;
                    Ok(())
                },
                |_, _| Ok(()),
            )
            .unwrap();
        let second_head = HeadRecord::new(
            1,
            writer,
            second_generation,
            second_manifest.sha256().unwrap(),
        )
        .unwrap();
        assert_eq!(
            layout
                .publish_head(Some(selected.slot), &second_head)
                .unwrap(),
            HeadSlot::One
        );
        assert_eq!(
            layout
                .publish_head(Some(selected.slot), &second_head)
                .unwrap(),
            HeadSlot::One
        );
        let selected = layout.read_head(|_, _| Ok(())).unwrap().unwrap();
        assert_eq!(selected.record, second_head);
    }

    #[test]
    fn invalid_higher_head_slot_is_explicitly_incomplete() {
        let root = tempfile::tempdir().unwrap();
        let layout = WriterLayout::create(root.path(), [6; 16]).unwrap();
        let writer = WriterInstanceId::from_bytes([6; 16]);
        let generation = GenerationId::from_bytes([7; 16]);
        let prepared = manifest(6, 7, 0);
        layout
            .publish_prepared_generation(
                &prepared,
                |db| {
                    fs::write(db, b"db").map_err(|e| io_error("write test db", e))?;
                    Ok(())
                },
                |_, _| Ok(()),
            )
            .unwrap();
        let head = HeadRecord::new(0, writer, generation, prepared.sha256().unwrap()).unwrap();
        layout.publish_head(None, &head).unwrap();
        fs::write(
            layout.writer_dir().join("HEAD.1"),
            vec![b'x'; MAX_GENERATION_CONTROL_RECORD_BYTES + 1],
        )
        .unwrap();
        let selected = layout.read_head(|_, _| Ok(())).unwrap().unwrap();
        assert_eq!(selected.record.epoch, 0);
        assert!(selected.issues.iter().any(|issue| matches!(
            issue,
            HeadCoverageIssue::HeadPublicationIncomplete {
                invalid_slot: HeadSlot::One
            }
        )));
        assert!(selected.issues.iter().any(|issue| matches!(
            issue,
            HeadCoverageIssue::SlotInvalid { reason, .. }
                if reason.contains("control record exceeds")
        )));
    }

    #[test]
    fn interruption_before_generation_rename_leaves_unpublished_staging_evidence() {
        let root = tempfile::tempdir().unwrap();
        let layout = WriterLayout::create(root.path(), [8; 16]).unwrap();
        let prepared = manifest(8, 9, 0);
        let result = layout.publish_prepared_generation(
            &prepared,
            |db| {
                fs::write(db, b"partial").map_err(|e| io_error("write staged test db", e))?;
                Err(GenerationError::Validation(
                    "injected interruption before directory rename".to_owned(),
                ))
            },
            |_, _| Ok(()),
        );
        assert!(matches!(result, Err(GenerationError::Validation(_))));
        assert!(layout.read_head(|_, _| Ok(())).unwrap().is_none());
        assert_eq!(fs::read_dir(&layout.staging_dir).unwrap().count(), 1);
        assert!(!layout.generation_dir(prepared.generation_id).exists());
    }

    #[test]
    fn detached_fixture_seals_and_publishes_idempotent_retirement_receipt() {
        let root = tempfile::tempdir().unwrap();
        let layout = WriterLayout::create(root.path(), [51; 16]).unwrap();
        let prepared = manifest(51, 52, 0);
        let metadata = prepared.generation_metadata().unwrap();
        let published = layout
            .publish_prepared_generation(
                &prepared,
                |db| {
                    let mut connection = rusqlite::Connection::open(db)?;
                    super::super::initialize_generation_schema(&mut connection, &metadata)
                        .map_err(|error| GenerationError::Validation(error.to_string()))?;
                    connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                        let busy: i64 = row.get(0)?;
                        if busy != 0 {
                            return Err(rusqlite::Error::InvalidQuery);
                        }
                        Ok(())
                    })?;
                    Ok(())
                },
                |db, _| {
                    let connection = rusqlite::Connection::open(db)?;
                    connection.pragma_update(None, "synchronous", "FULL")?;
                    super::super::verify_generation_schema(&connection, Some(&metadata))
                        .map_err(|error| GenerationError::Validation(error.to_string()))
                },
            )
            .unwrap();
        let mut connection = rusqlite::Connection::open(&published.database_path).unwrap();
        connection
            .pragma_update(None, "synchronous", "FULL")
            .unwrap();
        super::super::mark_generation_writable(&connection).unwrap();
        super::super::close_generation(&mut connection, 10, GenerationId::from_bytes([53; 16]))
            .unwrap();
        let busy: i64 = connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
            .unwrap();
        assert_eq!(busy, 0);
        drop(connection);
        drop(
            layout
                .acquire_generation_lease(prepared.generation_id)
                .unwrap(),
        );

        let (sealed, sealed_digest) =
            seal_closed_generation(&published.generation_dir, false).unwrap();
        assert_eq!(sealed.row_count, 0);
        assert!(
            sealed
                .durable_files
                .iter()
                .any(|file| file.relative_path == DATABASE_FILE_NAME)
        );

        let pending = root
            .path()
            .join("retirement/trash")
            .join(id_hex(prepared.writer_instance_id.as_bytes()))
            .join(format!(
                "{}.pending",
                id_hex(prepared.generation_id.as_bytes())
            ));
        fs::create_dir_all(pending.parent().unwrap()).unwrap();
        fs::rename(&published.generation_dir, &pending).unwrap();
        let receipt = RetirementReceipt::new(
            prepared.writer_instance_id,
            prepared.generation_id,
            prepared.sha256().unwrap(),
            sealed_digest,
            "age372-v1".to_owned(),
            20,
            1,
        );
        let receipt_path = publish_retirement_receipt(root.path(), &receipt).unwrap();
        assert!(receipt_path.is_file());
        assert_eq!(
            publish_retirement_receipt(root.path(), &receipt).unwrap(),
            receipt_path
        );
        let mut conflict = receipt;
        conflict.retirement_epoch = 2;
        assert!(matches!(
            publish_retirement_receipt(root.path(), &conflict),
            Err(GenerationError::PublicationConflict(_))
        ));
    }
}
