//! One bounded event-writer worker for one producer process instance.
//!
//! A worker owns exactly one writer directory, one active generation lease,
//! and one SQLite connection.  It never opens a sibling writer directory and
//! never performs historical discovery or maintenance.

use super::generation::{
    GenerationError, GenerationLease, GenerationSource, HeadRecord, HeadSlot, PreparedManifest,
    SelectedHead, WriterLayout, WriterOwnershipGuard,
};
use super::reader::reconcile_exact_generation_while_leased;
use super::{
    AppendDisposition, AppendTicket, EventEnvelopeV1, EventStoreError, GenerationId,
    GenerationMetadata, GenerationState, ProducerIdentity, ReconciliationOutcome, WriterInstanceId,
    append_batch, close_generation, initialize_generation_schema, mark_generation_writable,
    read_generation_metadata, reconcile_exact_generation, verify_generation_schema,
};
use rusqlite::{Connection, OpenFlags};
use std::collections::VecDeque;
use std::fmt;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::{AtomicI64, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_EVENT_WRITER_QUEUE_CAPACITY: usize = 256;
pub const EVENT_WRITER_REPLY_CHANNEL_CAPACITY: usize = 1;
pub const EVENT_WRITER_GROUP_MAX_RECORDS: usize = 32;
pub const EVENT_WRITER_GROUP_MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
pub const EVENT_WRITER_GROUP_MAX_RESIDENCE: Duration = Duration::from_millis(10);
pub const DEFAULT_GENERATION_ROTATION_SOFT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct EventWriterConfig {
    pub event_store_root: PathBuf,
    pub producer: ProducerIdentity,
    pub source: GenerationSource,
    pub queue_capacity: usize,
    pub rotation_soft_bytes: u64,
    #[cfg(test)]
    test_now_unix_micros: Option<Arc<AtomicI64>>,
}

impl EventWriterConfig {
    pub fn native(event_store_root: impl Into<PathBuf>, producer: ProducerIdentity) -> Self {
        Self {
            event_store_root: event_store_root.into(),
            producer,
            source: GenerationSource::NativeProcess,
            queue_capacity: DEFAULT_EVENT_WRITER_QUEUE_CAPACITY,
            rotation_soft_bytes: DEFAULT_GENERATION_ROTATION_SOFT_BYTES,
            #[cfg(test)]
            test_now_unix_micros: None,
        }
    }

    fn now_unix_micros(&self) -> Result<i64, WriterError> {
        #[cfg(test)]
        if let Some(now) = &self.test_now_unix_micros {
            return Ok(now.load(AtomicOrdering::SeqCst));
        }
        now_unix_micros()
    }

    fn validate(&self) -> Result<(), WriterError> {
        if self.queue_capacity == 0 || self.rotation_soft_bytes == 0 {
            return Err(WriterError::InvalidConfiguration(
                "queue capacity and rotation threshold must be positive".to_owned(),
            ));
        }
        match (&self.source, &self.producer.native_process) {
            (GenerationSource::NativeProcess, Some(_)) => Ok(()),
            (GenerationSource::LegacyImport { .. }, _) => Ok(()),
            (GenerationSource::Repair { .. }, _) => Ok(()),
            _ => Err(WriterError::InvalidConfiguration(
                "generation source and process provenance disagree".to_owned(),
            )),
        }
    }
}

#[derive(Debug)]
pub enum WriterError {
    InvalidConfiguration(String),
    Generation(GenerationError),
    EventStore(EventStoreError),
    QueueDisconnected,
    QueueFull,
    ReplyDisconnected {
        ticket: Option<AppendTicket>,
    },
    IdentityConflict {
        ticket: AppendTicket,
    },
    UnknownWrite {
        ticket: AppendTicket,
        reason: String,
    },
    WorkerPanicked,
}

impl fmt::Display for WriterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(reason) => {
                write!(formatter, "invalid event writer: {reason}")
            }
            Self::Generation(error) => error.fmt(formatter),
            Self::EventStore(error) => error.fmt(formatter),
            Self::QueueDisconnected => write!(formatter, "event writer queue is disconnected"),
            Self::QueueFull => write!(formatter, "event writer queue is full"),
            Self::ReplyDisconnected { .. } => write!(formatter, "event writer reply disconnected"),
            Self::IdentityConflict { .. } => write!(formatter, "event identity conflict"),
            Self::UnknownWrite { reason, .. } => {
                write!(formatter, "event write outcome is unknown: {reason}")
            }
            Self::WorkerPanicked => write!(formatter, "event writer worker panicked"),
        }
    }
}

impl std::error::Error for WriterError {}

impl From<GenerationError> for WriterError {
    fn from(value: GenerationError) -> Self {
        Self::Generation(value)
    }
}

impl From<EventStoreError> for WriterError {
    fn from(value: EventStoreError) -> Self {
        Self::EventStore(value)
    }
}

impl From<rusqlite::Error> for WriterError {
    fn from(value: rusqlite::Error) -> Self {
        Self::EventStore(EventStoreError::from(value))
    }
}

#[derive(Debug)]
pub struct EnqueueError {
    pub kind: EnqueueErrorKind,
    pub event: Box<EventEnvelopeV1>,
    /// Present only when admission observed an exact writable generation.
    /// Rotation-fence backpressure happens before a ticket exists.
    pub ticket: Option<AppendTicket>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnqueueErrorKind {
    Full,
    Disconnected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppendReceipt {
    Committed {
        ticket: AppendTicket,
        local_sequence: i64,
    },
    AlreadyCommitted {
        ticket: AppendTicket,
        local_sequence: i64,
    },
}

#[derive(Debug)]
pub struct PendingAppend {
    pub ticket: AppendTicket,
    receiver: Receiver<Result<AppendReceipt, WriterError>>,
}

impl PendingAppend {
    pub fn try_result(&self) -> Result<Option<AppendReceipt>, WriterError> {
        match self.receiver.try_recv() {
            Ok(result) => result.map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(WriterError::ReplyDisconnected {
                ticket: Some(self.ticket.clone()),
            }),
        }
    }

    pub fn wait(self) -> Result<AppendReceipt, WriterError> {
        self.receiver
            .recv()
            .map_err(|_| WriterError::ReplyDisconnected {
                ticket: Some(self.ticket),
            })?
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RotationReceipt {
    pub previous_generation_id: GenerationId,
    pub current_generation_id: GenerationId,
    pub head_epoch: i64,
}

struct AdmissionState {
    generation_id: GenerationId,
    accepting: bool,
}

enum Command {
    Append {
        ticket: AppendTicket,
        event: Box<EventEnvelopeV1>,
        enqueued_at: Instant,
        reply: SyncSender<Result<AppendReceipt, WriterError>>,
    },
    Rotate {
        reply: SyncSender<Result<RotationReceipt, WriterError>>,
    },
    Reconcile {
        ticket: AppendTicket,
        reply: SyncSender<ReconciliationOutcome>,
    },
    Shutdown {
        reply: SyncSender<Result<(), WriterError>>,
    },
    #[cfg(test)]
    Pause {
        entered: SyncSender<()>,
        release: Receiver<()>,
    },
    #[cfg(test)]
    FailNextAppend { armed: SyncSender<()> },
}

pub struct ProcessEventWriter {
    writer_instance_id: WriterInstanceId,
    sender: Arc<RwLock<Option<SyncSender<Command>>>>,
    admission: Arc<Mutex<AdmissionState>>,
    worker: Option<JoinHandle<()>>,
}

/// Cloneable access to the exact process writer's bounded admission path.
///
/// The owning [`ProcessEventWriter`] remains single-use for lifecycle closure.
/// Submission handles share its admission fence and sender slot, so shutdown
/// can stop and disconnect every handle without holding lifecycle ownership
/// while a durable caller waits for its worker reply.
#[derive(Clone)]
pub(crate) struct ProcessEventWriterSubmission {
    sender: Arc<RwLock<Option<SyncSender<Command>>>>,
    admission: Arc<Mutex<AdmissionState>>,
}

impl ProcessEventWriter {
    pub fn start(config: EventWriterConfig) -> Result<Self, WriterError> {
        config.validate()?;
        let writer_instance_id = config.producer.writer_instance_id;
        let layout =
            WriterLayout::create(&config.event_store_root, *writer_instance_id.as_bytes())?;
        let owner = layout.acquire_writer_ownership()?;
        let state = WriterState::open(config, layout, owner)?;
        let initial_generation = state.generation_id;
        let admission = Arc::new(Mutex::new(AdmissionState {
            generation_id: initial_generation,
            accepting: true,
        }));
        let (sender, receiver) = mpsc::sync_channel(state.config.queue_capacity);
        let sender = Arc::new(RwLock::new(Some(sender)));
        let worker_admission = Arc::clone(&admission);
        let worker = thread::Builder::new()
            .name(format!(
                "event-writer-{}",
                super::generation::id_hex(writer_instance_id.as_bytes())
            ))
            .spawn(move || run_worker(state, receiver, worker_admission))
            .map_err(|error| {
                WriterError::InvalidConfiguration(format!("failed to start writer worker: {error}"))
            })?;
        Ok(Self {
            writer_instance_id,
            sender,
            admission,
            worker: Some(worker),
        })
    }

    pub fn writer_instance_id(&self) -> WriterInstanceId {
        self.writer_instance_id
    }

    pub fn current_generation_id(&self) -> Result<GenerationId, WriterError> {
        Ok(self.lock_admission()?.generation_id)
    }

    pub(crate) fn submission_handle(&self) -> ProcessEventWriterSubmission {
        ProcessEventWriterSubmission {
            sender: Arc::clone(&self.sender),
            admission: Arc::clone(&self.admission),
        }
    }

    pub fn try_append(&self, event: EventEnvelopeV1) -> Result<PendingAppend, EnqueueError> {
        self.submission_handle().try_append(event)
    }

    /// Synchronous only after bounded admission succeeds. A full queue returns
    /// explicit backpressure; this method never waits for queue capacity.
    pub fn append(&self, event: EventEnvelopeV1) -> Result<AppendReceipt, WriterError> {
        self.submission_handle().append(event)
    }

    pub fn request_rotation(&self) -> Result<RotationReceipt, WriterError> {
        let (reply, result) = mpsc::sync_channel(EVENT_WRITER_REPLY_CHANNEL_CAPACITY);
        self.send_control(Command::Rotate { reply })?;
        result
            .recv()
            .map_err(|_| WriterError::ReplyDisconnected { ticket: None })?
    }

    pub fn reconcile(&self, ticket: AppendTicket) -> Result<ReconciliationOutcome, WriterError> {
        if ticket.writer_instance_id != self.writer_instance_id {
            return Ok(ReconciliationOutcome::Unknown {
                reason: "ticket belongs to another writer instance".to_owned(),
            });
        }
        let (reply, result) = mpsc::sync_channel(EVENT_WRITER_REPLY_CHANNEL_CAPACITY);
        self.send_control(Command::Reconcile { ticket, reply })?;
        result
            .recv()
            .map_err(|_| WriterError::ReplyDisconnected { ticket: None })
    }

    fn send_control(&self, command: Command) -> Result<(), WriterError> {
        let sender = self
            .sender
            .read()
            .map_err(|_| WriterError::QueueDisconnected)?;
        sender
            .as_ref()
            .ok_or(WriterError::QueueDisconnected)?
            .try_send(command)
            .map_err(|error| match error {
                TrySendError::Full(_) => WriterError::QueueFull,
                TrySendError::Disconnected(_) => WriterError::QueueDisconnected,
            })
    }

    pub fn retry_unknown(
        &self,
        ticket: AppendTicket,
        mut event: EventEnvelopeV1,
    ) -> Result<AppendReceipt, WriterError> {
        let attempted = AppendTicket::new(ticket.generation_id, &event)?;
        if attempted.writer_instance_id != ticket.writer_instance_id
            || attempted.event_id != ticket.event_id
            || attempted.immutable_sha256 != ticket.immutable_sha256
        {
            return Err(WriterError::IdentityConflict { ticket });
        }
        match self.reconcile(ticket.clone())? {
            ReconciliationOutcome::AlreadyCommitted { local_sequence } => {
                Ok(AppendReceipt::AlreadyCommitted {
                    ticket,
                    local_sequence,
                })
            }
            ReconciliationOutcome::IdentityConflict { .. } => {
                Err(WriterError::IdentityConflict { ticket })
            }
            ReconciliationOutcome::Unknown { reason } => {
                Err(WriterError::UnknownWrite { ticket, reason })
            }
            ReconciliationOutcome::AbsentHealthy => {
                event.retry_of_generation_id = Some(ticket.generation_id);
                self.append(event)
            }
        }
    }

    pub fn shutdown(mut self) -> Result<(), WriterError> {
        let sender = {
            let mut admission = self.lock_admission()?;
            admission.accepting = false;
            self.sender
                .write()
                .map_err(|_| WriterError::QueueDisconnected)?
                .take()
                .ok_or(WriterError::QueueDisconnected)?
        };
        let (reply, result) = mpsc::sync_channel(EVENT_WRITER_REPLY_CHANNEL_CAPACITY);
        // Admission is fenced and the shared sender slot is empty, so no new
        // work can enter. Waiting here for one control slot lets the worker
        // drain every pre-fence command even when the bounded queue was full.
        sender
            .send(Command::Shutdown { reply })
            .map_err(|_| WriterError::QueueDisconnected)?;
        result
            .recv()
            .map_err(|_| WriterError::ReplyDisconnected { ticket: None })??;
        if self
            .worker
            .take()
            .is_some_and(|worker| worker.join().is_err())
        {
            return Err(WriterError::WorkerPanicked);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fail_next_append_for_test(&self) -> Result<(), WriterError> {
        let (armed, result) = mpsc::sync_channel(EVENT_WRITER_REPLY_CHANNEL_CAPACITY);
        self.send_control(Command::FailNextAppend { armed })?;
        result
            .recv()
            .map_err(|_| WriterError::ReplyDisconnected { ticket: None })
    }

    #[cfg(test)]
    pub(crate) fn pause_for_test(&self) -> Result<SyncSender<()>, WriterError> {
        let (entered, entered_result) = mpsc::sync_channel(EVENT_WRITER_REPLY_CHANNEL_CAPACITY);
        let (release, release_result) = mpsc::sync_channel(EVENT_WRITER_REPLY_CHANNEL_CAPACITY);
        self.send_control(Command::Pause {
            entered,
            release: release_result,
        })?;
        entered_result
            .recv()
            .map_err(|_| WriterError::ReplyDisconnected { ticket: None })?;
        Ok(release)
    }

    fn lock_admission(&self) -> Result<MutexGuard<'_, AdmissionState>, WriterError> {
        self.admission
            .lock()
            .map_err(|_| WriterError::InvalidConfiguration("admission lock poisoned".to_owned()))
    }
}

impl ProcessEventWriterSubmission {
    pub(crate) fn try_append(&self, event: EventEnvelopeV1) -> Result<PendingAppend, EnqueueError> {
        let admission = match self.admission.try_lock() {
            Ok(admission) => admission,
            Err(TryLockError::WouldBlock) => {
                return Err(EnqueueError {
                    kind: EnqueueErrorKind::Full,
                    event: Box::new(event),
                    ticket: None,
                });
            }
            Err(TryLockError::Poisoned(_)) => {
                return Err(EnqueueError {
                    kind: EnqueueErrorKind::Disconnected,
                    event: Box::new(event),
                    ticket: None,
                });
            }
        };
        let ticket = AppendTicket::new(admission.generation_id, &event)
            .expect("a normalized event must yield a ticket");
        if !admission.accepting {
            return Err(EnqueueError {
                kind: EnqueueErrorKind::Disconnected,
                event: Box::new(event),
                ticket: Some(ticket),
            });
        }
        let (reply, receiver) = mpsc::sync_channel(EVENT_WRITER_REPLY_CHANNEL_CAPACITY);
        let command = Command::Append {
            ticket: ticket.clone(),
            event: Box::new(event),
            enqueued_at: Instant::now(),
            reply,
        };
        let sender = match self.sender.read() {
            Ok(sender) => sender,
            Err(_) => {
                let Command::Append { event, .. } = command else {
                    unreachable!()
                };
                return Err(EnqueueError {
                    kind: EnqueueErrorKind::Disconnected,
                    event,
                    ticket: Some(ticket),
                });
            }
        };
        let Some(sender) = sender.as_ref() else {
            let Command::Append { event, .. } = command else {
                unreachable!()
            };
            return Err(EnqueueError {
                kind: EnqueueErrorKind::Disconnected,
                event,
                ticket: Some(ticket),
            });
        };
        match sender.try_send(command) {
            Ok(()) => Ok(PendingAppend { ticket, receiver }),
            Err(TrySendError::Full(Command::Append { event, .. })) => Err(EnqueueError {
                kind: EnqueueErrorKind::Full,
                event,
                ticket: Some(ticket),
            }),
            Err(TrySendError::Disconnected(Command::Append { event, .. })) => Err(EnqueueError {
                kind: EnqueueErrorKind::Disconnected,
                event,
                ticket: Some(ticket),
            }),
            Err(_) => unreachable!("try_append sends only append commands"),
        }
    }

    /// Synchronous only after bounded admission succeeds. A full queue returns
    /// explicit backpressure; this method never waits for queue capacity.
    pub(crate) fn append(&self, event: EventEnvelopeV1) -> Result<AppendReceipt, WriterError> {
        self.try_append(event)
            .map_err(|error| match error.kind {
                EnqueueErrorKind::Full => WriterError::QueueFull,
                EnqueueErrorKind::Disconnected => WriterError::QueueDisconnected,
            })?
            .wait()
    }

    #[cfg(test)]
    pub(crate) fn accepting_for_test(&self) -> bool {
        self.admission
            .lock()
            .is_ok_and(|admission| admission.accepting)
    }
}

impl Drop for ProcessEventWriter {
    fn drop(&mut self) {
        if let Ok(mut admission) = self.admission.lock() {
            admission.accepting = false;
        }
        if let Ok(mut sender) = self.sender.write() {
            sender.take();
        }
        // Do not make a destructor an unbounded disk wait. Receiver disconnect
        // causes the worker to best-effort close its exact current generation.
        self.worker.take();
    }
}

struct WriterState {
    config: EventWriterConfig,
    layout: WriterLayout,
    _owner: WriterOwnershipGuard,
    lease: GenerationLease,
    connection: Option<Connection>,
    generation_id: GenerationId,
    head_slot: HeadSlot,
    head_epoch: i64,
    generation_day: i64,
}

impl WriterState {
    fn open(
        config: EventWriterConfig,
        layout: WriterLayout,
        owner: WriterOwnershipGuard,
    ) -> Result<Self, WriterError> {
        let selected = layout.read_head(validate_head_database)?;
        match selected {
            None => Self::create_initial(config, layout, owner),
            Some(selected) => Self::recover_exact(config, layout, owner, selected),
        }
    }

    fn create_initial(
        config: EventWriterConfig,
        layout: WriterLayout,
        owner: WriterOwnershipGuard,
    ) -> Result<Self, WriterError> {
        let generation_id = GenerationId::random();
        let created_at = config.now_unix_micros()?;
        let manifest = build_manifest(&config, generation_id, None, created_at, 0);
        // Own the exact shared generation lease before the discovery intent
        // becomes visible. Detached classification can therefore never win an
        // exclusive lease over an in-progress live publication.
        let lease = layout.acquire_generation_lease(generation_id)?;
        let published = publish_generation(&layout, &manifest)?;
        let head = HeadRecord::new(
            0,
            config.producer.writer_instance_id,
            generation_id,
            published.prepared_manifest_sha256,
        )?;
        let head_slot = layout.publish_head(None, &head)?;
        let _ = super::maintenance_discovery::record_generation_phase(
            layout.event_store_root(),
            config.producer.writer_instance_id,
            generation_id,
            super::maintenance_discovery::DiscoveryPhase::Selected,
        );
        let connection = open_writable_generation(&published.database_path, &manifest)?;
        Ok(Self {
            config,
            layout,
            _owner: owner,
            lease,
            connection: Some(connection),
            generation_id,
            head_slot,
            head_epoch: 0,
            generation_day: utc_day(created_at),
        })
    }

    fn recover_exact(
        config: EventWriterConfig,
        layout: WriterLayout,
        owner: WriterOwnershipGuard,
        selected: SelectedHead,
    ) -> Result<Self, WriterError> {
        let selected_manifest = super::generation::read_prepared_manifest(
            &layout.generation_dir(selected.record.generation_id),
        )?;
        ensure_manifest_owner(&selected_manifest, &config)?;
        let selected_path = layout.generation_db(selected.record.generation_id);
        let selected_connection = open_generation_connection(&selected_path, &selected_manifest)?;
        let selected_metadata = read_generation_metadata(&selected_connection)?;
        match selected_metadata.state {
            GenerationState::Prepared => {
                let lease = layout.acquire_generation_lease(selected.record.generation_id)?;
                let _ = super::maintenance_discovery::register_legacy_generation(
                    layout.event_store_root(),
                    config.producer.writer_instance_id,
                    selected.record.generation_id,
                    super::maintenance_discovery::DiscoveryPhase::Selected,
                    None,
                    Some(&selected_manifest),
                );
                mark_generation_writable(&selected_connection)?;
                Ok(Self {
                    config,
                    layout,
                    _owner: owner,
                    lease,
                    connection: Some(selected_connection),
                    generation_id: selected.record.generation_id,
                    head_slot: selected.slot,
                    head_epoch: selected.record.epoch,
                    generation_day: utc_day(selected_manifest.created_at_unix_micros),
                })
            }
            GenerationState::Writable => {
                let lease = layout.acquire_generation_lease(selected.record.generation_id)?;
                let _ = super::maintenance_discovery::register_legacy_generation(
                    layout.event_store_root(),
                    config.producer.writer_instance_id,
                    selected.record.generation_id,
                    super::maintenance_discovery::DiscoveryPhase::Selected,
                    None,
                    Some(&selected_manifest),
                );
                Ok(Self {
                    config,
                    layout,
                    _owner: owner,
                    lease,
                    connection: Some(selected_connection),
                    generation_id: selected.record.generation_id,
                    head_slot: selected.slot,
                    head_epoch: selected.record.epoch,
                    generation_day: utc_day(selected_manifest.created_at_unix_micros),
                })
            }
            GenerationState::Closed => {
                let _closed_lease =
                    layout.acquire_generation_lease(selected.record.generation_id)?;
                let successor = selected_metadata.successor_generation_id.ok_or_else(|| {
                    WriterError::InvalidConfiguration(
                        "closed selected head lacks exact successor".to_owned(),
                    )
                })?;
                drop(selected_connection);
                let successor_dir = layout.generation_dir(successor);
                let successor_manifest = super::generation::read_prepared_manifest(&successor_dir)?;
                ensure_manifest_owner(&successor_manifest, &config)?;
                if successor_manifest.predecessor_generation_id
                    != Some(selected.record.generation_id)
                    || successor_manifest.head_epoch != selected.record.epoch + 1
                {
                    return Err(WriterError::InvalidConfiguration(
                        "closed head's exact successor metadata disagrees".to_owned(),
                    ));
                }
                let lease = layout.acquire_generation_lease(successor)?;
                let _ = super::maintenance_discovery::register_legacy_generation(
                    layout.event_store_root(),
                    config.producer.writer_instance_id,
                    successor,
                    super::maintenance_discovery::DiscoveryPhase::Prepared,
                    None,
                    Some(&successor_manifest),
                );
                let successor_head = HeadRecord::new(
                    successor_manifest.head_epoch,
                    config.producer.writer_instance_id,
                    successor,
                    successor_manifest.sha256()?,
                )?;
                let head_slot = layout.publish_head(Some(selected.slot), &successor_head)?;
                let _ = super::maintenance_discovery::record_closed_generation(
                    layout.event_store_root(),
                    config.producer.writer_instance_id,
                    selected.record.generation_id,
                );
                let _ = super::maintenance_discovery::record_generation_phase(
                    layout.event_store_root(),
                    config.producer.writer_instance_id,
                    successor,
                    super::maintenance_discovery::DiscoveryPhase::Selected,
                );
                let connection = open_writable_generation(
                    &layout.generation_db(successor),
                    &successor_manifest,
                )?;
                Ok(Self {
                    config,
                    layout,
                    _owner: owner,
                    lease,
                    connection: Some(connection),
                    generation_id: successor,
                    head_slot,
                    head_epoch: successor_manifest.head_epoch,
                    generation_day: utc_day(successor_manifest.created_at_unix_micros),
                })
            }
        }
    }

    fn connection_mut(&mut self) -> Result<&mut Connection, WriterError> {
        self.connection.as_mut().ok_or_else(|| {
            WriterError::InvalidConfiguration("writer has no open generation".to_owned())
        })
    }

    fn should_rotate(&self) -> Result<bool, WriterError> {
        let now = self.config.now_unix_micros()?;
        if utc_day(now) != self.generation_day {
            return Ok(true);
        }
        let db = self.layout.generation_db(self.generation_id);
        let wal = db.with_file_name(format!("{}-wal", super::generation::DATABASE_FILE_NAME));
        let bytes = file_len_or_zero(&db)?.saturating_add(file_len_or_zero(&wal)?);
        Ok(bytes >= self.config.rotation_soft_bytes)
    }

    fn reconcile(&self, ticket: &AppendTicket) -> ReconciliationOutcome {
        let path = self.layout.generation_db(ticket.generation_id);
        if ticket.generation_id == self.generation_id {
            reconcile_exact_generation_while_leased(&path, ticket)
        } else {
            reconcile_exact_generation(
                &path,
                &self.layout.generation_lease_path(ticket.generation_id),
                ticket,
            )
        }
    }
}

fn run_worker(
    mut state: WriterState,
    receiver: Receiver<Command>,
    admission: Arc<Mutex<AdmissionState>>,
) {
    let mut pending = VecDeque::new();
    #[cfg(test)]
    let mut fail_next_append = false;
    loop {
        let command = pending.pop_front().or_else(|| receiver.recv().ok());
        match command {
            Some(command @ Command::Append { .. }) => {
                #[cfg(test)]
                if fail_next_append {
                    let Command::Append { ticket, reply, .. } = command else {
                        unreachable!()
                    };
                    fail_next_append = false;
                    let _ = reply.send(Err(WriterError::UnknownWrite {
                        ticket,
                        reason: "deterministic post-admission test failure".to_string(),
                    }));
                    continue;
                }
                let mut batch = Vec::new();
                collect_batch(command, &receiver, &mut pending, &mut batch);
                commit_batch(&mut state, batch);
                match state.should_rotate() {
                    Ok(true) => {
                        if rotate_exact_current(&mut state, &receiver, &admission, &mut pending)
                            .is_err()
                            && let Ok(mut fence) = admission.lock()
                        {
                            fence.accepting = false;
                        }
                    }
                    Ok(false) => {}
                    Err(_) => {
                        if let Ok(mut fence) = admission.lock() {
                            fence.accepting = false;
                        }
                    }
                }
            }
            Some(Command::Rotate { reply }) => {
                let result = rotate_exact_current(&mut state, &receiver, &admission, &mut pending);
                if result.is_err()
                    && let Ok(mut fence) = admission.lock()
                {
                    fence.accepting = false;
                }
                let _ = reply.send(result);
            }
            Some(Command::Reconcile { ticket, reply }) => {
                let _ = reply.send(state.reconcile(&ticket));
            }
            Some(Command::Shutdown { reply }) => {
                let result = close_for_shutdown(&mut state, &receiver, &admission, &mut pending);
                let _ = reply.send(result);
                break;
            }
            #[cfg(test)]
            Some(Command::Pause { entered, release }) => {
                let _ = entered.send(());
                let _ = release.recv();
            }
            #[cfg(test)]
            Some(Command::FailNextAppend { armed }) => {
                fail_next_append = true;
                let _ = armed.send(());
            }
            None => {
                let _ = close_for_shutdown(&mut state, &receiver, &admission, &mut pending);
                break;
            }
        }
    }
}

type BatchItem = (
    AppendTicket,
    EventEnvelopeV1,
    SyncSender<Result<AppendReceipt, WriterError>>,
);

fn collect_batch(
    first: Command,
    receiver: &Receiver<Command>,
    pending: &mut VecDeque<Command>,
    batch: &mut Vec<BatchItem>,
) {
    let deadline = match &first {
        Command::Append { enqueued_at, .. } => *enqueued_at + EVENT_WRITER_GROUP_MAX_RESIDENCE,
        _ => unreachable!("a batch starts with an append command"),
    };
    let mut payload_bytes = 0usize;
    let mut next = Some(first);
    while batch.len() < EVENT_WRITER_GROUP_MAX_RECORDS {
        let command = match next.take() {
            Some(command) => command,
            None => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match receiver.recv_timeout(remaining) {
                    Ok(command) => command,
                    Err(_) => break,
                }
            }
        };
        match command {
            Command::Append {
                ticket,
                event,
                enqueued_at,
                reply,
            } => {
                let event_bytes = usize::try_from(event.payload_bytes).unwrap_or(usize::MAX);
                if !batch.is_empty()
                    && payload_bytes.saturating_add(event_bytes)
                        > EVENT_WRITER_GROUP_MAX_PAYLOAD_BYTES
                {
                    pending.push_front(Command::Append {
                        ticket,
                        event,
                        enqueued_at,
                        reply,
                    });
                    break;
                }
                payload_bytes = payload_bytes.saturating_add(event_bytes);
                batch.push((ticket, *event, reply));
            }
            control => {
                pending.push_front(control);
                break;
            }
        }
    }
}

fn commit_batch(state: &mut WriterState, batch: Vec<BatchItem>) {
    if batch.is_empty() {
        return;
    }
    let expected_generation = state.generation_id;
    if batch
        .iter()
        .any(|(ticket, _, _)| ticket.generation_id != expected_generation)
    {
        for (ticket, _, reply) in batch {
            let _ = reply.send(Err(WriterError::UnknownWrite {
                ticket,
                reason: "ticket generation is no longer the exact writable head".to_owned(),
            }));
        }
        return;
    }
    let events: Vec<_> = batch.iter().map(|(_, event, _)| event.clone()).collect();
    let append_result = match state.connection_mut() {
        Ok(connection) => append_batch(connection, expected_generation, &events),
        Err(error) => {
            let reason = error.to_string();
            for (ticket, _, reply) in batch {
                let _ = reply.send(Err(WriterError::UnknownWrite {
                    ticket,
                    reason: reason.clone(),
                }));
            }
            return;
        }
    };
    match append_result {
        Ok(dispositions) => {
            for ((ticket, _, reply), disposition) in batch.into_iter().zip(dispositions) {
                let receipt = match disposition {
                    AppendDisposition::Committed { local_sequence } => AppendReceipt::Committed {
                        ticket,
                        local_sequence,
                    },
                    AppendDisposition::AlreadyCommitted { local_sequence } => {
                        AppendReceipt::AlreadyCommitted {
                            ticket,
                            local_sequence,
                        }
                    }
                };
                let _ = reply.send(Ok(receipt));
            }
        }
        Err(EventStoreError::IdentityConflict { event_id, .. }) => {
            for (ticket, _, reply) in batch {
                let error = if ticket.event_id == event_id {
                    WriterError::IdentityConflict { ticket }
                } else {
                    WriterError::UnknownWrite {
                        ticket,
                        reason: "append group rolled back because another event had an identity conflict"
                            .to_owned(),
                    }
                };
                let _ = reply.send(Err(error));
            }
        }
        Err(error) => {
            let reason = error.to_string();
            for (ticket, _, reply) in batch {
                let _ = reply.send(Err(WriterError::UnknownWrite {
                    ticket,
                    reason: reason.clone(),
                }));
            }
        }
    }
}

fn rotate_exact_current(
    state: &mut WriterState,
    receiver: &Receiver<Command>,
    admission: &Arc<Mutex<AdmissionState>>,
    pending: &mut VecDeque<Command>,
) -> Result<RotationReceipt, WriterError> {
    let previous_generation_id = state.generation_id;
    let successor = GenerationId::random();
    let created_at = state.config.now_unix_micros()?;
    let next_epoch = state.head_epoch + 1;
    let manifest = build_manifest(
        &state.config,
        successor,
        Some(previous_generation_id),
        created_at,
        next_epoch,
    );
    let successor_lease = state.layout.acquire_generation_lease(successor)?;
    let published = publish_generation(&state.layout, &manifest)?;

    let mut fence = admission
        .lock()
        .map_err(|_| WriterError::InvalidConfiguration("admission lock poisoned".to_owned()))?;
    while let Ok(command) = receiver.try_recv() {
        pending.push_back(command);
    }
    let mut controls = VecDeque::new();
    while let Some(command) = pending.pop_front() {
        match command {
            append @ Command::Append { .. } => {
                let mut batch = Vec::new();
                collect_batch(append, receiver, &mut controls, &mut batch);
                commit_batch(state, batch);
            }
            control => controls.push_back(control),
        }
    }
    *pending = controls;

    let closed_at = state.config.now_unix_micros()?;
    close_generation(state.connection_mut()?, closed_at, successor)?;
    drop(state.connection.take());
    let _ = super::maintenance_discovery::record_closed_generation(
        state.layout.event_store_root(),
        state.config.producer.writer_instance_id,
        previous_generation_id,
    );
    let head = HeadRecord::new(
        next_epoch,
        state.config.producer.writer_instance_id,
        successor,
        published.prepared_manifest_sha256,
    )?;
    let new_slot = state.layout.publish_head(Some(state.head_slot), &head)?;
    let _ = super::maintenance_discovery::record_generation_phase(
        state.layout.event_store_root(),
        state.config.producer.writer_instance_id,
        successor,
        super::maintenance_discovery::DiscoveryPhase::Selected,
    );
    let successor_connection = open_writable_generation(&published.database_path, &manifest)?;

    state.connection = Some(successor_connection);
    state.lease = successor_lease;
    state.generation_id = successor;
    state.head_slot = new_slot;
    state.head_epoch = next_epoch;
    state.generation_day = utc_day(created_at);
    fence.generation_id = successor;
    Ok(RotationReceipt {
        previous_generation_id,
        current_generation_id: successor,
        head_epoch: next_epoch,
    })
}

fn close_for_shutdown(
    state: &mut WriterState,
    receiver: &Receiver<Command>,
    admission: &Arc<Mutex<AdmissionState>>,
    pending: &mut VecDeque<Command>,
) -> Result<(), WriterError> {
    // A lifecycle close requires an exact successor. Use the normal rotation
    // protocol, leaving its published successor as the exact restart head.
    rotate_exact_current(state, receiver, admission, pending)?;
    state.connection.take();
    Ok(())
}

fn build_manifest(
    config: &EventWriterConfig,
    generation_id: GenerationId,
    predecessor: Option<GenerationId>,
    created_at: i64,
    head_epoch: i64,
) -> PreparedManifest {
    PreparedManifest {
        format_version: super::generation::EVENT_STORE_FORMAT_VERSION,
        schema_version: super::EVENT_SCHEMA_VERSION,
        writer_instance_id: config.producer.writer_instance_id,
        generation_id,
        predecessor_generation_id: predecessor,
        database_relative_path: super::generation::DATABASE_FILE_NAME.to_owned(),
        created_at_unix_micros: created_at,
        head_epoch,
        source: config.source.clone(),
        producer: config.producer.clone(),
    }
}

fn publish_generation(
    layout: &WriterLayout,
    manifest: &PreparedManifest,
) -> Result<super::generation::PublishedGeneration, WriterError> {
    let metadata = manifest.generation_metadata()?;
    layout
        .publish_prepared_generation(
            manifest,
            |db| prepare_database(db, &metadata),
            validate_prepared_database,
        )
        .map_err(WriterError::from)
}

fn prepare_database(db: &Path, metadata: &GenerationMetadata) -> Result<(), GenerationError> {
    let mut connection = Connection::open(db)?;
    initialize_generation_schema(&mut connection, metadata)
        .map_err(|error| GenerationError::Validation(error.to_string()))?;
    let checkpoint: (i64, i64, i64) =
        connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if checkpoint.0 != 0 || checkpoint.1 != checkpoint.2 {
        return Err(GenerationError::Validation(format!(
            "successor checkpoint incomplete: {checkpoint:?}"
        )));
    }
    drop(connection);
    Ok(())
}

fn validate_prepared_database(
    db: &Path,
    manifest: &PreparedManifest,
) -> Result<(), GenerationError> {
    let connection = open_read_only_generation(db, manifest)
        .map_err(|error| GenerationError::Validation(error.to_string()))?;
    let quick: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if quick != "ok" {
        return Err(GenerationError::Validation(format!(
            "quick_check failed: {quick}"
        )));
    }
    let expected = manifest.generation_metadata()?;
    let observed = read_generation_metadata(&connection)
        .map_err(|error| GenerationError::Validation(error.to_string()))?;
    if observed != expected {
        return Err(GenerationError::Validation(
            "prepared manifest and metadata row disagree".to_owned(),
        ));
    }
    Ok(())
}

fn validate_head_database(db: &Path, head: &HeadRecord) -> Result<(), GenerationError> {
    let generation_dir = db.parent().ok_or_else(|| {
        GenerationError::Validation("generation database has no parent".to_owned())
    })?;
    let manifest = super::generation::read_prepared_manifest(generation_dir)?;
    if manifest.generation_id != head.generation_id || manifest.head_epoch != head.epoch {
        return Err(GenerationError::Validation(
            "head and database generation disagree".to_owned(),
        ));
    }
    let connection = open_read_only_generation(db, &manifest)
        .map_err(|error| GenerationError::Validation(error.to_string()))?;
    let metadata = read_generation_metadata(&connection)
        .map_err(|error| GenerationError::Validation(error.to_string()))?;
    ensure_manifest_metadata(&manifest, &metadata)
}

fn open_writable_generation(
    db: &Path,
    manifest: &PreparedManifest,
) -> Result<Connection, WriterError> {
    let connection = open_generation_connection(db, manifest)?;
    let metadata = read_generation_metadata(&connection)?;
    match metadata.state {
        GenerationState::Prepared => mark_generation_writable(&connection)?,
        GenerationState::Writable => {}
        GenerationState::Closed => return Err(EventStoreError::InvalidGenerationState.into()),
    }
    Ok(connection)
}

fn open_generation_connection(
    db: &Path,
    manifest: &PreparedManifest,
) -> Result<Connection, WriterError> {
    let connection = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    let journal: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if !journal.eq_ignore_ascii_case("wal") {
        return Err(WriterError::EventStore(EventStoreError::Configuration(
            format!("generation journal mode is {journal}, expected WAL"),
        )));
    }
    connection.pragma_update(None, "synchronous", "FULL")?;
    verify_generation_schema(&connection, None)?;
    let metadata = read_generation_metadata(&connection)?;
    ensure_manifest_metadata(manifest, &metadata)?;
    Ok(connection)
}

fn open_read_only_generation(
    db: &Path,
    manifest: &PreparedManifest,
) -> Result<Connection, WriterError> {
    let connection = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    connection.pragma_update(None, "query_only", true)?;
    let journal: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if !journal.eq_ignore_ascii_case("wal") {
        return Err(WriterError::EventStore(EventStoreError::Configuration(
            format!("generation journal mode is {journal}, expected WAL"),
        )));
    }
    verify_generation_schema(&connection, None)?;
    let metadata = read_generation_metadata(&connection)?;
    ensure_manifest_metadata(manifest, &metadata)?;
    Ok(connection)
}

fn ensure_manifest_metadata(
    manifest: &PreparedManifest,
    metadata: &GenerationMetadata,
) -> Result<(), GenerationError> {
    let expected = manifest.generation_metadata()?;
    if metadata.schema_version != expected.schema_version
        || metadata.generation_id != expected.generation_id
        || metadata.predecessor_generation_id != expected.predecessor_generation_id
        || metadata.writer_instance_id != expected.writer_instance_id
        || metadata.process_instance_id != expected.process_instance_id
        || metadata.process_root_id != expected.process_root_id
        || metadata.parent_process_instance_id != expected.parent_process_instance_id
        || metadata.supervisor_authority_id != expected.supervisor_authority_id
        || metadata.native_process != expected.native_process
        || metadata.created_at_unix_micros != expected.created_at_unix_micros
        || metadata.head_epoch != expected.head_epoch
        || metadata.predecessor_status != expected.predecessor_status
    {
        return Err(GenerationError::Validation(
            "prepared manifest and immutable metadata disagree".to_owned(),
        ));
    }
    Ok(())
}

fn ensure_manifest_owner(
    manifest: &PreparedManifest,
    config: &EventWriterConfig,
) -> Result<(), WriterError> {
    if manifest.writer_instance_id != config.producer.writer_instance_id
        || manifest.producer != config.producer
        || manifest.source != config.source
    {
        return Err(WriterError::InvalidConfiguration(
            "existing writer head belongs to a different producer identity".to_owned(),
        ));
    }
    Ok(())
}

fn now_unix_micros() -> Result<i64, WriterError> {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| WriterError::InvalidConfiguration(format!("clock before epoch: {error}")))?
        .as_micros();
    i64::try_from(micros)
        .map_err(|_| WriterError::InvalidConfiguration("clock exceeds i64 micros".to_owned()))
}

fn utc_day(unix_micros: i64) -> i64 {
    unix_micros.div_euclid(86_400_000_000)
}

fn file_len_or_zero(path: &Path) -> Result<u64, WriterError> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(WriterError::Generation(GenerationError::Io {
            operation: "inspect generation size",
            source: error,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_store::{
        Digest32, EventCorrelations, EventFamily, EventId, EventKind, NativeProcessIdentity,
        NewEventV1, PayloadNormalizationPolicy, ProcessInstanceId,
    };
    use serde_json::json;
    use std::fs;
    use std::sync::{Arc, Barrier, Mutex};

    fn producer(byte: u8) -> ProducerIdentity {
        ProducerIdentity {
            writer_instance_id: WriterInstanceId::from_bytes([byte; 16]),
            process_instance_id: ProcessInstanceId::from_bytes([byte; 16]),
            process_root_id: ProcessInstanceId::from_bytes([byte; 16]),
            parent_process_instance_id: None,
            supervisor_authority_id: None,
            native_process: Some(NativeProcessIdentity {
                os_pid: i64::from(std::process::id()),
                os_boot_id_sha256: Digest32::sha256(&[byte]),
                os_pid_starttime_ticks: i64::from(byte),
            }),
        }
    }

    fn event(producer: &ProducerIdentity, id: u8, sequence: i64) -> EventEnvelopeV1 {
        EventEnvelopeV1::normalize(
            NewEventV1 {
                event_id: EventId::from_bytes([id; 16]),
                family: EventFamily::Diagnostic,
                kind: EventKind::registered("writer.test").unwrap(),
                recorded_at_unix_micros: 100 + sequence,
                producer_sequence: sequence,
                producer: producer.clone(),
                correlations: EventCorrelations::default(),
                payload: json!({"status": "safe"}),
                legacy_provenance: None,
                retry_of_generation_id: None,
            },
            &PayloadNormalizationPolicy::registered(&["status"]).unwrap(),
            200 + sequence,
        )
        .unwrap()
    }

    fn pause(writer: &ProcessEventWriter) -> SyncSender<()> {
        writer.pause_for_test().unwrap()
    }

    #[test]
    fn bounded_queue_reports_backpressure_without_waiting() {
        let root = tempfile::tempdir().unwrap();
        let identity = producer(11);
        let mut config = EventWriterConfig::native(root.path(), identity.clone());
        config.queue_capacity = 1;
        let writer = ProcessEventWriter::start(config).unwrap();
        let release = pause(&writer);
        let accepted = writer.try_append(event(&identity, 1, 1)).unwrap();
        let rejected = writer.try_append(event(&identity, 2, 2)).unwrap_err();
        assert_eq!(rejected.kind, EnqueueErrorKind::Full);
        release.send(()).unwrap();
        assert!(matches!(
            accepted.wait().unwrap(),
            AppendReceipt::Committed { .. }
        ));
        writer.shutdown().unwrap();
    }

    #[test]
    fn rotation_fence_reports_backpressure_without_waiting_for_admission_lock() {
        let root = tempfile::tempdir().unwrap();
        let identity = producer(63);
        let writer =
            ProcessEventWriter::start(EventWriterConfig::native(root.path(), identity.clone()))
                .unwrap();
        let fence = writer.admission.lock().unwrap();
        let rejected = writer.try_append(event(&identity, 1, 1)).unwrap_err();
        assert_eq!(rejected.kind, EnqueueErrorKind::Full);
        assert!(rejected.ticket.is_none());
        drop(fence);
        writer.shutdown().unwrap();
    }

    #[test]
    fn tickets_queued_around_rotation_commit_only_to_old_exact_head() {
        let root = tempfile::tempdir().unwrap();
        let identity = producer(12);
        let mut config = EventWriterConfig::native(root.path(), identity.clone());
        config.queue_capacity = 8;
        let writer = ProcessEventWriter::start(config).unwrap();
        let old = writer.current_generation_id().unwrap();
        let release = pause(&writer);
        let before = writer.try_append(event(&identity, 1, 1)).unwrap();
        let (rotation_tx, rotation_rx) = mpsc::sync_channel(EVENT_WRITER_REPLY_CHANNEL_CAPACITY);
        writer
            .send_control(Command::Rotate { reply: rotation_tx })
            .unwrap();
        // This admission happens after the rotate request is queued, but the
        // rotation fence must drain its already-fixed old-head ticket.
        let after_request = writer.try_append(event(&identity, 2, 2)).unwrap();
        assert_eq!(before.ticket.generation_id, old);
        assert_eq!(after_request.ticket.generation_id, old);
        release.send(()).unwrap();
        assert!(matches!(
            before.wait().unwrap(),
            AppendReceipt::Committed { .. }
        ));
        assert!(matches!(
            after_request.wait().unwrap(),
            AppendReceipt::Committed { .. }
        ));
        let rotation = rotation_rx.recv().unwrap().unwrap();
        assert_eq!(rotation.previous_generation_id, old);
        assert_ne!(rotation.current_generation_id, old);

        let old_dir = root
            .path()
            .join("writers")
            .join(super::super::generation::id_hex(
                identity.writer_instance_id.as_bytes(),
            ))
            .join("generations")
            .join(super::super::generation::id_hex(old.as_bytes()));
        let old_connection = Connection::open_with_flags(
            old_dir.join(super::super::generation::DATABASE_FILE_NAME),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        let old_metadata = read_generation_metadata(&old_connection).unwrap();
        assert_eq!(old_metadata.state, GenerationState::Closed);
        assert_eq!(
            old_metadata.successor_generation_id,
            Some(rotation.current_generation_id)
        );
        assert!(
            !old_dir
                .join(super::super::generation::SEALED_MANIFEST_FILE_NAME)
                .exists()
        );
        writer.shutdown().unwrap();
    }

    #[test]
    fn rotation_shutdown_and_exact_head_restart_reopen_sqlite() {
        let root = tempfile::tempdir().unwrap();
        let identity = producer(13);
        let config = EventWriterConfig::native(root.path(), identity.clone());
        let writer = ProcessEventWriter::start(config.clone()).unwrap();
        let first = writer.current_generation_id().unwrap();
        writer.append(event(&identity, 1, 1)).unwrap();
        let rotation = writer.request_rotation().unwrap();
        assert_eq!(rotation.previous_generation_id, first);
        writer.append(event(&identity, 2, 2)).unwrap();
        writer.shutdown().unwrap();

        let restarted = ProcessEventWriter::start(config).unwrap();
        let restart_head = restarted.current_generation_id().unwrap();
        assert_ne!(restart_head, rotation.current_generation_id);
        restarted.append(event(&identity, 3, 3)).unwrap();
        restarted.shutdown().unwrap();
    }

    #[test]
    fn active_writer_rotates_after_each_observed_utc_day_change() {
        let root = tempfile::tempdir().unwrap();
        let identity = producer(14);
        let day_micros = 86_400_000_000_i64;
        let clock = Arc::new(AtomicI64::new(10 * day_micros + 1));
        let mut config = EventWriterConfig::native(root.path(), identity.clone());
        config.test_now_unix_micros = Some(Arc::clone(&clock));
        let writer = ProcessEventWriter::start(config).unwrap();
        let first = writer.current_generation_id().unwrap();

        writer.append(event(&identity, 1, 1)).unwrap();
        assert_eq!(writer.current_generation_id().unwrap(), first);

        clock.store(11 * day_micros + 1, AtomicOrdering::SeqCst);
        writer.append(event(&identity, 2, 2)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let second = loop {
            let observed = writer.current_generation_id().unwrap();
            if observed != first {
                break observed;
            }
            assert!(
                Instant::now() < deadline,
                "first daily rotation did not finish"
            );
            thread::yield_now();
        };

        clock.store(12 * day_micros + 1, AtomicOrdering::SeqCst);
        writer.append(event(&identity, 3, 3)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let third = loop {
            let observed = writer.current_generation_id().unwrap();
            if observed != second {
                break observed;
            }
            assert!(
                Instant::now() < deadline,
                "second daily rotation did not finish"
            );
            thread::yield_now();
        };

        assert_ne!(third, first);
        writer.shutdown().unwrap();
    }

    #[test]
    #[ignore = "bounded timing observation; run explicitly with --ignored --nocapture"]
    fn observe_rotation_with_small_and_retained_generation_sets() {
        fn sample(retained_generations: usize, producer_id: u8) -> (usize, Vec<u128>) {
            let root = tempfile::tempdir().unwrap();
            let identity = producer(producer_id);
            let writer =
                ProcessEventWriter::start(EventWriterConfig::native(root.path(), identity.clone()))
                    .unwrap();
            for _ in 0..retained_generations {
                writer.request_rotation().unwrap();
            }
            let writer_dir = root
                .path()
                .join("writers")
                .join(super::super::generation::id_hex(
                    identity.writer_instance_id.as_bytes(),
                ));
            let before = fs::read_dir(writer_dir.join("generations"))
                .unwrap()
                .filter_map(Result::ok)
                .count();
            let mut elapsed_micros = Vec::new();
            for _ in 0..3 {
                let started = Instant::now();
                writer.request_rotation().unwrap();
                elapsed_micros.push(started.elapsed().as_micros());
            }
            writer.shutdown().unwrap();
            (before, elapsed_micros)
        }

        let (small_count, small_samples) = sample(0, 91);
        let (retained_count, retained_samples) = sample(12, 92);
        assert_eq!(small_count, 1);
        assert_eq!(retained_count, 13);
        eprintln!(
            "AGE374_ROTATION_OBSERVATION small_generation_count={small_count} small_elapsed_micros={small_samples:?} retained_generation_count={retained_count} retained_elapsed_micros={retained_samples:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn exact_rotation_succeeds_when_a_history_dependent_neighbor_cannot_read_retained_heads() {
        use std::io;
        use std::os::unix::fs::PermissionsExt;

        struct RestorePermissions(Vec<PathBuf>);

        impl Drop for RestorePermissions {
            fn drop(&mut self) {
                for path in &self.0 {
                    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
                }
            }
        }

        fn history_dependent_neighbor(generations: &Path, current: &str) -> io::Result<usize> {
            let mut examined = 0;
            for entry in fs::read_dir(generations)? {
                let entry = entry?;
                if entry.file_name().to_str() == Some(current) {
                    continue;
                }
                fs::read(
                    entry
                        .path()
                        .join(super::super::generation::PREPARED_MANIFEST_FILE_NAME),
                )?;
                examined += 1;
            }
            Ok(examined)
        }

        let root = tempfile::tempdir().unwrap();
        let identity = producer(93);
        let writer =
            ProcessEventWriter::start(EventWriterConfig::native(root.path(), identity.clone()))
                .unwrap();
        for _ in 0..12 {
            writer.request_rotation().unwrap();
        }
        let current = writer.current_generation_id().unwrap();
        let current_name = super::super::generation::id_hex(current.as_bytes());
        let layout =
            WriterLayout::open_existing(root.path(), *identity.writer_instance_id.as_bytes())
                .unwrap();
        let mut protected = Vec::new();
        for entry in fs::read_dir(layout.generations_dir()).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().and_then(|name| name.to_str()) != Some(&current_name) {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
                protected.push(path);
            }
        }
        assert_eq!(protected.len(), 12);
        let restore = RestorePermissions(protected);

        let neighbor_error = history_dependent_neighbor(layout.generations_dir(), &current_name)
            .expect_err("the history-dependent neighbor unexpectedly read protected history");
        assert_eq!(neighbor_error.kind(), io::ErrorKind::PermissionDenied);

        let rotation = writer.request_rotation().unwrap();
        assert_eq!(rotation.previous_generation_id, current);
        drop(restore);
        writer.shutdown().unwrap();
    }

    #[test]
    fn different_writer_instances_have_different_sqlite_writers() {
        let root = tempfile::tempdir().unwrap();
        let first_identity = producer(21);
        let second_identity = producer(22);
        let first = ProcessEventWriter::start(EventWriterConfig::native(
            root.path(),
            first_identity.clone(),
        ))
        .unwrap();
        let second = ProcessEventWriter::start(EventWriterConfig::native(
            root.path(),
            second_identity.clone(),
        ))
        .unwrap();
        let barrier = Barrier::new(3);
        thread::scope(|scope| {
            let first_thread = scope.spawn(|| {
                barrier.wait();
                first.append(event(&first_identity, 1, 1)).unwrap()
            });
            let second_thread = scope.spawn(|| {
                barrier.wait();
                second.append(event(&second_identity, 2, 1)).unwrap()
            });
            barrier.wait();
            assert!(matches!(
                first_thread.join().unwrap(),
                AppendReceipt::Committed { .. }
            ));
            assert!(matches!(
                second_thread.join().unwrap(),
                AppendReceipt::Committed { .. }
            ));
        });
        let writer_dirs: Vec<_> = fs::read_dir(root.path().join("writers"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(writer_dirs.len(), 2);
        let first_db = root
            .path()
            .join("writers")
            .join(super::super::generation::id_hex(
                first_identity.writer_instance_id.as_bytes(),
            ))
            .join("generations")
            .join(super::super::generation::id_hex(
                first.current_generation_id().unwrap().as_bytes(),
            ))
            .join(super::super::generation::DATABASE_FILE_NAME);
        let second_db = root
            .path()
            .join("writers")
            .join(super::super::generation::id_hex(
                second_identity.writer_instance_id.as_bytes(),
            ))
            .join("generations")
            .join(super::super::generation::id_hex(
                second.current_generation_id().unwrap().as_bytes(),
            ))
            .join(super::super::generation::DATABASE_FILE_NAME);
        assert_ne!(first_db, second_db);
        first.shutdown().unwrap();
        second.shutdown().unwrap();
    }

    #[test]
    fn unavailable_discovery_shard_does_not_veto_concurrent_independent_producer() {
        let root = tempfile::tempdir().unwrap();
        let discovery = root.path().join("maintenance-discovery-v1/active");
        fs::create_dir_all(&discovery).unwrap();
        fs::write(discovery.join("01"), b"blocked exact writer shard").unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let blocked_barrier = Arc::clone(&barrier);
        let healthy_barrier = Arc::clone(&barrier);
        let blocked_root = root.path().to_path_buf();
        let healthy_root = root.path().to_path_buf();

        let blocked = thread::spawn(move || {
            blocked_barrier.wait();
            ProcessEventWriter::start(EventWriterConfig::native(blocked_root, producer(1)))
        });
        let healthy = thread::spawn(move || {
            healthy_barrier.wait();
            ProcessEventWriter::start(EventWriterConfig::native(healthy_root, producer(2)))
        });
        barrier.wait();

        assert!(matches!(
            blocked.join().unwrap(),
            Err(WriterError::Generation(GenerationError::Validation(_)))
        ));
        let healthy = healthy.join().unwrap().unwrap();
        healthy.shutdown().unwrap();
        assert!(
            !root
                .path()
                .join("maintenance-inventory-v1.sqlite3")
                .exists()
        );
    }

    #[test]
    fn producer_shared_lease_is_owned_before_discovery_intent_is_actionable() {
        let root = tempfile::tempdir().unwrap();
        let identity = producer(88);
        let expected_writer = identity.writer_instance_id;
        let generation = Arc::new(Mutex::new(None));
        let generation_from_hook = Arc::clone(&generation);
        let intent_visible = Arc::new(Barrier::new(2));
        let intent_visible_from_hook = Arc::clone(&intent_visible);
        let release = Arc::new(Barrier::new(2));
        let release_from_hook = Arc::clone(&release);
        let hook = Arc::new(move |_: &Path, writer, observed_generation| {
            if writer == expected_writer {
                *generation_from_hook.lock().unwrap() = Some(observed_generation);
                intent_visible_from_hook.wait();
                release_from_hook.wait();
            }
        });

        let writer = super::super::maintenance_discovery::with_generation_intent_hook(hook, || {
            let producer_root = root.path().to_path_buf();
            let writer_thread = thread::spawn(move || {
                ProcessEventWriter::start(EventWriterConfig::native(producer_root, identity))
            });
            intent_visible.wait();
            let generation = generation.lock().unwrap().unwrap();
            let layout =
                WriterLayout::open_existing(root.path(), *expected_writer.as_bytes()).unwrap();
            assert!(matches!(
                layout.acquire_generation_maintenance_lease(generation),
                Err(GenerationError::GenerationAlreadyOwned(_))
            ));
            release.wait();
            writer_thread.join().unwrap().unwrap()
        });
        writer.shutdown().unwrap();
    }

    #[test]
    fn subprocess_writer_helper() {
        let Some(root) = std::env::var_os("OULIPOLY_AGE376_SUBPROCESS_ROOT") else {
            return;
        };
        let id: u8 = std::env::var("OULIPOLY_AGE376_SUBPROCESS_ID")
            .unwrap()
            .parse()
            .unwrap();
        let identity = producer(id);
        let writer = ProcessEventWriter::start(EventWriterConfig::native(
            PathBuf::from(root),
            identity.clone(),
        ))
        .unwrap();
        writer.append(event(&identity, id, 1)).unwrap();
        writer.shutdown().unwrap();
    }

    #[test]
    fn independent_processes_never_share_an_event_sqlite_writer() {
        let root = tempfile::tempdir().unwrap();
        let executable = std::env::current_exe().unwrap();
        let mut first = std::process::Command::new(&executable)
            .arg("event_store::writer::tests::subprocess_writer_helper")
            .arg("--exact")
            .env("OULIPOLY_AGE376_SUBPROCESS_ROOT", root.path())
            .env("OULIPOLY_AGE376_SUBPROCESS_ID", "61")
            .spawn()
            .unwrap();
        let mut second = std::process::Command::new(&executable)
            .arg("event_store::writer::tests::subprocess_writer_helper")
            .arg("--exact")
            .env("OULIPOLY_AGE376_SUBPROCESS_ROOT", root.path())
            .env("OULIPOLY_AGE376_SUBPROCESS_ID", "62")
            .spawn()
            .unwrap();
        assert!(first.wait().unwrap().success());
        assert!(second.wait().unwrap().success());

        let first_writer = root
            .path()
            .join("writers")
            .join(super::super::generation::id_hex(
                WriterInstanceId::from_bytes([61; 16]).as_bytes(),
            ));
        let second_writer = root
            .path()
            .join("writers")
            .join(super::super::generation::id_hex(
                WriterInstanceId::from_bytes([62; 16]).as_bytes(),
            ));
        assert_ne!(first_writer, second_writer);
        assert!(first_writer.join("HEAD.0").exists() || first_writer.join("HEAD.1").exists());
        assert!(second_writer.join("HEAD.0").exists() || second_writer.join("HEAD.1").exists());
        let first_databases = fs::read_dir(first_writer.join("generations"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .join(super::super::generation::DATABASE_FILE_NAME)
                    .is_file()
            })
            .count();
        let second_databases = fs::read_dir(second_writer.join("generations"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .join(super::super::generation::DATABASE_FILE_NAME)
                    .is_file()
            })
            .count();
        assert!(
            first_databases >= 2,
            "shutdown rotates the first writer head"
        );
        assert!(
            second_databases >= 2,
            "shutdown rotates the second writer head"
        );
    }

    #[test]
    fn unknown_write_reconciliation_is_idempotent_and_refuses_identity_conflict() {
        let root = tempfile::tempdir().unwrap();
        let identity = producer(31);
        let writer =
            ProcessEventWriter::start(EventWriterConfig::native(root.path(), identity.clone()))
                .unwrap();
        let original = event(&identity, 1, 1);
        let receipt = writer.append(original.clone()).unwrap();
        let ticket = match receipt {
            AppendReceipt::Committed { ticket, .. } => ticket,
            AppendReceipt::AlreadyCommitted { .. } => panic!("first append must commit"),
        };
        writer.request_rotation().unwrap();
        assert!(matches!(
            writer.retry_unknown(ticket.clone(), original).unwrap(),
            AppendReceipt::AlreadyCommitted { .. }
        ));

        let mut conflicting = event(&identity, 1, 1);
        conflicting.payload = "{\"status\":\"different\"}".to_owned();
        conflicting.payload_bytes = conflicting.payload.len() as i64;
        conflicting.payload_sha256 = Digest32::sha256(conflicting.payload.as_bytes());
        assert!(matches!(
            writer.retry_unknown(ticket, conflicting),
            Err(WriterError::IdentityConflict { .. })
        ));

        let absent = event(&identity, 2, 2);
        let absent_ticket =
            AppendTicket::new(writer.current_generation_id().unwrap(), &absent).unwrap();
        assert!(matches!(
            writer.retry_unknown(absent_ticket, absent).unwrap(),
            AppendReceipt::Committed { .. }
        ));
        writer.shutdown().unwrap();
    }

    #[test]
    fn recovery_does_not_adopt_prepared_successor_before_old_close() {
        let root = tempfile::tempdir().unwrap();
        let identity = producer(41);
        let config = EventWriterConfig::native(root.path(), identity);
        let layout =
            WriterLayout::create(root.path(), *config.producer.writer_instance_id.as_bytes())
                .unwrap();
        let owner = layout.acquire_writer_ownership().unwrap();
        let state = WriterState::create_initial(config.clone(), layout, owner).unwrap();
        let old = state.generation_id;
        let successor = GenerationId::random();
        let manifest = build_manifest(
            &config,
            successor,
            Some(old),
            now_unix_micros().unwrap(),
            state.head_epoch + 1,
        );
        publish_generation(&state.layout, &manifest).unwrap();
        drop(state); // interruption after final directory, before old close

        let restarted = ProcessEventWriter::start(config).unwrap();
        assert_eq!(restarted.current_generation_id().unwrap(), old);
        assert!(
            root.path()
                .join("writers")
                .join(super::super::generation::id_hex(
                    restarted.writer_instance_id().as_bytes(),
                ))
                .join("generations")
                .join(super::super::generation::id_hex(successor.as_bytes()))
                .exists()
        );
        restarted.shutdown().unwrap();
    }

    #[test]
    fn recovery_follows_only_exact_successor_after_old_close_before_head() {
        let root = tempfile::tempdir().unwrap();
        let identity = producer(42);
        let config = EventWriterConfig::native(root.path(), identity);
        let layout =
            WriterLayout::create(root.path(), *config.producer.writer_instance_id.as_bytes())
                .unwrap();
        let owner = layout.acquire_writer_ownership().unwrap();
        let mut state = WriterState::create_initial(config.clone(), layout, owner).unwrap();
        let old = state.generation_id;
        let successor = GenerationId::random();
        let manifest = build_manifest(
            &config,
            successor,
            Some(old),
            now_unix_micros().unwrap(),
            state.head_epoch + 1,
        );
        publish_generation(&state.layout, &manifest).unwrap();
        close_generation(
            state.connection_mut().unwrap(),
            now_unix_micros().unwrap(),
            successor,
        )
        .unwrap();
        state.connection.take();
        drop(state); // interruption after old close, before head publication

        let restarted = ProcessEventWriter::start(config).unwrap();
        assert_eq!(restarted.current_generation_id().unwrap(), successor);
        restarted.shutdown().unwrap();
    }
}
