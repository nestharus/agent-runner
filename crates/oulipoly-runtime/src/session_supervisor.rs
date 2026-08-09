//! Resident, session-scoped owner for durable provider-turn coordination.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use oulipoly_state::{
    DispositionWrite, EventDisposition, ExactProcessIdentity, LeaseAcquire, LifecycleEvent,
    NewLifecycleEvent, ProviderTurnGeneration, SessionLifecycleError, SessionLifecycleRepository,
    SessionReconstruction, SupervisorFence, TurnFence, TurnState,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessObservation {
    ExactLive,
    Dead,
    Reused(ExactProcessIdentity),
    Unknown(String),
}

pub trait ProcessObserver: Send + Sync + 'static {
    fn observe(&self, expected: &ExactProcessIdentity) -> ProcessObservation;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LocalProcessObserver;

impl ProcessObserver for LocalProcessObserver {
    fn observe(&self, expected: &ExactProcessIdentity) -> ProcessObservation {
        match oulipoly_state::pid_identity::read_live_process_identity(expected.pid) {
            Ok(Some(live)) => {
                let observed = ExactProcessIdentity {
                    pid: live.os_pid,
                    boot_id: live.os_boot_id,
                    start_time_ticks: live.os_pid_starttime_ticks,
                };
                if observed == *expected {
                    ProcessObservation::ExactLive
                } else {
                    ProcessObservation::Reused(observed)
                }
            }
            Ok(None) => ProcessObservation::Dead,
            Err(error) => ProcessObservation::Unknown(error),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupervisorConfig {
    pub queue_capacity: usize,
    pub max_retries: usize,
    pub consumer_id: String,
    pub reconstruction_limit: usize,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 64,
            max_retries: 0,
            consumer_id: "resident-session-supervisor".to_owned(),
            reconstruction_limit: 256,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionNotification<Input> {
    pub sequence: i64,
    pub input: Input,
    pub turns: VecDeque<ProviderTurnGeneration>,
    pub expires_at: Option<i64>,
}

impl<Input> SessionNotification<Input> {
    pub fn new(sequence: i64, input: Input, turn: ProviderTurnGeneration) -> Self {
        Self {
            sequence,
            input,
            turns: VecDeque::from([turn]),
            expires_at: None,
        }
    }

    pub fn with_retry_turns(
        sequence: i64,
        input: Input,
        turns: impl IntoIterator<Item = ProviderTurnGeneration>,
    ) -> Self {
        Self {
            sequence,
            input,
            turns: turns.into_iter().collect(),
            expires_at: None,
        }
    }

    pub fn expiring_at(mut self, expires_at: i64) -> Self {
        self.expires_at = Some(expires_at);
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedNotification {
    pub sequence: i64,
    pub active_generation: Option<String>,
    pub queued_notifications: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupervisorPhase {
    Running,
    Paused,
    Draining,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupervisorSnapshot {
    pub session_id: String,
    pub fence: SupervisorFence,
    pub phase: SupervisorPhase,
    pub active_generation: Option<String>,
    pub active_sequence: Option<i64>,
    pub queued_sequences: Vec<i64>,
    pub child_adapter_connected: bool,
    pub published_events: usize,
    pub publication_failures: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnInput<Input> {
    pub sequence: i64,
    pub input: Input,
}

#[derive(Debug)]
pub struct TurnRequest<Input, Output> {
    pub session_id: String,
    pub owner_fence: SupervisorFence,
    pub turn: ProviderTurnGeneration,
    pub notification: TurnInput<Input>,
    pub completion: TurnCompletion<Input, Output>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TurnOutcome<Output> {
    Completed(Output),
    Cancelled,
    RetryScheduled(String),
    RetryExhausted(String),
    GenerationExhausted(String),
    Terminated(TerminalReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnResult<Output> {
    pub session_id: String,
    pub generation_id: String,
    pub spawn_invocation_id: String,
    pub notification_sequence: i64,
    pub outcome: TurnOutcome<Output>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalReason {
    Drained,
    ExplicitClose,
    Expired,
    HandleDropped,
    OwnerPanicked,
}

impl TerminalReason {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Drained => "supervisor_drained",
            Self::ExplicitClose => "supervisor_closed",
            Self::Expired => "supervisor_expired",
            Self::HandleDropped => "supervisor_handle_dropped",
            Self::OwnerPanicked => "supervisor_owner_panicked",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChildFailure {
    CompletionDropped,
    Panic,
    AbnormalExit(String),
}

impl fmt::Display for ChildFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CompletionDropped => formatter.write_str("completion capability dropped"),
            Self::Panic => formatter.write_str("child adapter panicked"),
            Self::AbnormalExit(reason) => write!(formatter, "abnormal child exit: {reason}"),
        }
    }
}

#[derive(Debug)]
pub enum SupervisorError {
    Closed,
    Paused,
    Draining,
    QueueFull,
    Expired,
    DuplicateOrStaleSequence,
    InvalidTurnFence,
    StaleCommand,
    NotFound,
    Repository(SessionLifecycleError),
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("session supervisor is closed"),
            Self::Paused => formatter.write_str("session supervisor is paused"),
            Self::Draining => formatter.write_str("session supervisor is draining"),
            Self::QueueFull => formatter.write_str("session supervisor queue is full"),
            Self::Expired => formatter.write_str("session work is expired"),
            Self::DuplicateOrStaleSequence => {
                formatter.write_str("notification sequence is duplicate or stale")
            }
            Self::InvalidTurnFence => formatter.write_str("turn is not fenced to this session"),
            Self::StaleCommand => formatter.write_str("command is stale for the active generation"),
            Self::NotFound => formatter.write_str("accepted work was not found"),
            Self::Repository(error) => write!(formatter, "durable lifecycle repository: {error}"),
        }
    }
}

impl std::error::Error for SupervisorError {}

impl From<SessionLifecycleError> for SupervisorError {
    fn from(value: SessionLifecycleError) -> Self {
        Self::Repository(value)
    }
}

#[derive(Debug)]
pub enum SupervisorStartError {
    InvalidConfig(&'static str),
    CandidateNotLive(ProcessObservation),
    ExistingOwnerLive,
    OwnerAlreadyRunning,
    ProcessObservationUnknown(String),
    ReplacementGenerationExhausted,
    ReplacementGenerationMismatch { expected: i64, actual: i64 },
    Repository(SessionLifecycleError),
}

impl fmt::Display for SupervisorStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(field) => write!(formatter, "invalid supervisor config: {field}"),
            Self::CandidateNotLive(observation) => {
                write!(
                    formatter,
                    "candidate process is not exact-live: {observation:?}"
                )
            }
            Self::ExistingOwnerLive => {
                formatter.write_str("an exact live owner already holds the lease")
            }
            Self::OwnerAlreadyRunning => {
                formatter.write_str("this process already has a resident owner for the session")
            }
            Self::ProcessObservationUnknown(error) => {
                write!(
                    formatter,
                    "process identity could not be reconciled: {error}"
                )
            }
            Self::ReplacementGenerationExhausted => {
                formatter.write_str("supervisor replacement generation is exhausted")
            }
            Self::ReplacementGenerationMismatch { expected, actual } => write!(
                formatter,
                "replacement generation mismatch: expected {expected}, got {actual}"
            ),
            Self::Repository(error) => write!(formatter, "durable lifecycle repository: {error}"),
        }
    }
}

impl std::error::Error for SupervisorStartError {}

impl From<SessionLifecycleError> for SupervisorStartError {
    fn from(value: SessionLifecycleError) -> Self {
        Self::Repository(value)
    }
}

#[derive(Debug)]
pub struct TurnCompletion<Input, Output> {
    fence: TurnFence,
    commands: Option<Sender<SupervisorCommand<Input, Output>>>,
}

impl<Input, Output> TurnCompletion<Input, Output> {
    pub fn complete(mut self, output: Output, completed_at: i64) -> Result<(), SupervisorError> {
        self.send(ChildExit::Completed(output), completed_at)
    }

    pub fn abnormal_exit(
        mut self,
        reason: impl Into<String>,
        completed_at: i64,
    ) -> Result<(), SupervisorError> {
        self.send(
            ChildExit::Failed(ChildFailure::AbnormalExit(reason.into())),
            completed_at,
        )
    }

    pub fn panicked(mut self, completed_at: i64) -> Result<(), SupervisorError> {
        self.send(ChildExit::Failed(ChildFailure::Panic), completed_at)
    }

    fn send(&mut self, exit: ChildExit<Output>, completed_at: i64) -> Result<(), SupervisorError> {
        let commands = self.commands.take().ok_or(SupervisorError::Closed)?;
        let (reply_tx, reply_rx) = mpsc::sync_channel(0);
        commands
            .send(SupervisorCommand::ChildExited {
                fence: self.fence.clone(),
                exit,
                completed_at,
                reply: reply_tx,
            })
            .map_err(|_| SupervisorError::Closed)?;
        reply_rx.recv().map_err(|_| SupervisorError::Closed)?
    }

    fn disarm(&mut self) {
        self.commands.take();
    }
}

impl<Input, Output> Drop for TurnCompletion<Input, Output> {
    fn drop(&mut self) {
        let Some(commands) = self.commands.take() else {
            return;
        };
        let (reply, _ignored) = mpsc::sync_channel(0);
        let _ = commands.send(SupervisorCommand::ChildExited {
            fence: self.fence.clone(),
            exit: ChildExit::Failed(ChildFailure::CompletionDropped),
            completed_at: 0,
            reply,
        });
    }
}

pub struct SessionSupervisor<Input, Output> {
    commands: Sender<SupervisorCommand<Input, Output>>,
    owner: Option<thread::JoinHandle<()>>,
}

impl<Input, Output> SessionSupervisor<Input, Output>
where
    Input: Clone + Send + 'static,
    Output: Send + 'static,
{
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        session_id: impl Into<String>,
        fence: SupervisorFence,
        acquired_at: i64,
        mut repository: Box<dyn SessionLifecycleRepository + Send>,
        process_observer: Arc<dyn ProcessObserver>,
        config: SupervisorConfig,
        turns: Sender<TurnRequest<Input, Output>>,
        events: Sender<LifecycleEvent>,
    ) -> Result<(Self, Receiver<TurnResult<Output>>), SupervisorStartError> {
        validate_config(&config)?;
        let session_id = session_id.into();
        let candidate_observation = process_observer.observe(&fence.process);
        if candidate_observation != ProcessObservation::ExactLive {
            return Err(SupervisorStartError::CandidateNotLive(
                candidate_observation,
            ));
        }

        let mut reconstruction = repository.reconstruct_session(
            &session_id,
            &config.consumer_id,
            config.reconstruction_limit,
        )?;
        acquire_or_replace(
            repository.as_mut(),
            &session_id,
            &fence,
            acquired_at,
            process_observer.as_ref(),
            &reconstruction,
        )?;
        let registration = OwnerRegistration::acquire(&session_id, &fence)?;

        let mut startup_events = reconstruction.undisposed_events.clone();
        let active = reconcile_reconstructed_turn(
            repository.as_mut(),
            &session_id,
            &fence,
            process_observer.as_ref(),
            reconstruction.active_turn.take(),
            acquired_at,
            &mut startup_events,
        )?;

        let (command_tx, command_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        for event in startup_events {
            command_tx
                .send(SupervisorCommand::ReplayCommitted { event })
                .map_err(|_| {
                    SupervisorStartError::Repository(SessionLifecycleError::Conflict(
                        "startup command",
                    ))
                })?;
        }

        let owner_commands = command_tx.clone();
        let owner = thread::spawn(move || {
            let mut state = OwnerState {
                session_id,
                fence,
                config,
                repository,
                turns,
                results: result_tx,
                events,
                phase: SupervisorPhase::Running,
                queued: VecDeque::new(),
                active,
                last_sequence: None,
                child_adapter_connected: true,
                published_events: 0,
                publication_failures: 0,
                terminal: false,
                _registration: registration,
            };
            let result = catch_unwind(AssertUnwindSafe(|| {
                run_owner(&mut state, command_rx, &owner_commands)
            }));
            if result.is_err() && !state.terminal {
                let _ = state.terminate(TerminalReason::OwnerPanicked, 0);
            }
        });

        Ok((
            Self {
                commands: command_tx,
                owner: Some(owner),
            },
            result_rx,
        ))
    }

    pub fn notify(
        &self,
        notification: SessionNotification<Input>,
        accepted_at: i64,
    ) -> Result<AcceptedNotification, SupervisorError> {
        self.request(|reply| SupervisorCommand::Notification {
            notification,
            accepted_at,
            reply,
        })
    }

    pub fn status(&self) -> Result<SupervisorSnapshot, SupervisorError> {
        self.request(|reply| SupervisorCommand::Status { reply })
    }

    pub fn pause(&self, at: i64) -> Result<(), SupervisorError> {
        self.request(|reply| SupervisorCommand::Pause { at, reply })
    }

    pub fn resume(&self, at: i64) -> Result<(), SupervisorError> {
        self.request(|reply| SupervisorCommand::Resume { at, reply })
    }

    pub fn cancel(&self, sequence: i64, at: i64) -> Result<(), SupervisorError> {
        self.request(|reply| SupervisorCommand::Cancel {
            sequence,
            at,
            reply,
        })
    }

    pub fn drain(&self, at: i64) -> Result<(), SupervisorError> {
        self.request(|reply| SupervisorCommand::Drain { at, reply })
    }

    pub fn reconnect_child_adapter(
        &self,
        turns: Sender<TurnRequest<Input, Output>>,
        at: i64,
    ) -> Result<(), SupervisorError> {
        self.request(|reply| SupervisorCommand::ReconnectChildAdapter { turns, at, reply })
    }

    pub fn commit_event(
        &self,
        event: NewLifecycleEvent,
    ) -> Result<LifecycleEvent, SupervisorError> {
        self.request(|reply| SupervisorCommand::CommitEvent { event, reply })
    }

    pub fn reconcile_reconstructed_exit(
        &self,
        fence: TurnFence,
        at: i64,
        reason: impl Into<String>,
    ) -> Result<(), SupervisorError> {
        self.request(|reply| SupervisorCommand::ReconstructedExit {
            fence,
            at,
            reason: reason.into(),
            reply,
        })
    }

    pub fn expire(mut self, at: i64) -> Result<(), SupervisorError> {
        let result = self.request(|reply| SupervisorCommand::Expire { at, reply });
        if result.is_ok() {
            self.join_owner()?;
        }
        result
    }

    pub fn close(mut self, at: i64) -> Result<(), SupervisorError> {
        let result = self.request(|reply| SupervisorCommand::Close { at, reply });
        if result.is_ok() {
            self.join_owner()?;
        }
        result
    }

    fn request<T>(
        &self,
        command: impl FnOnce(SyncSender<Result<T, SupervisorError>>) -> SupervisorCommand<Input, Output>,
    ) -> Result<T, SupervisorError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(0);
        self.commands
            .send(command(reply_tx))
            .map_err(|_| SupervisorError::Closed)?;
        reply_rx.recv().map_err(|_| SupervisorError::Closed)?
    }

    fn join_owner(&mut self) -> Result<(), SupervisorError> {
        if let Some(owner) = self.owner.take() {
            owner.join().map_err(|_| SupervisorError::Closed)?;
        }
        Ok(())
    }
}

impl<Input, Output> Drop for SessionSupervisor<Input, Output> {
    fn drop(&mut self) {
        if self.owner.is_none() {
            return;
        }
        let (reply, reply_rx) = mpsc::sync_channel(0);
        let _ = self
            .commands
            .send(SupervisorCommand::HandleDropped { reply });
        let _ = reply_rx.recv();
        if let Some(owner) = self.owner.take() {
            let _ = owner.join();
        }
    }
}

enum ChildExit<Output> {
    Completed(Output),
    Failed(ChildFailure),
}

enum SupervisorCommand<Input, Output> {
    Notification {
        notification: SessionNotification<Input>,
        accepted_at: i64,
        reply: SyncSender<Result<AcceptedNotification, SupervisorError>>,
    },
    ChildExited {
        fence: TurnFence,
        exit: ChildExit<Output>,
        completed_at: i64,
        reply: SyncSender<Result<(), SupervisorError>>,
    },
    ReplayCommitted {
        event: LifecycleEvent,
    },
    CommitEvent {
        event: NewLifecycleEvent,
        reply: SyncSender<Result<LifecycleEvent, SupervisorError>>,
    },
    Status {
        reply: SyncSender<Result<SupervisorSnapshot, SupervisorError>>,
    },
    Pause {
        at: i64,
        reply: SyncSender<Result<(), SupervisorError>>,
    },
    Resume {
        at: i64,
        reply: SyncSender<Result<(), SupervisorError>>,
    },
    Cancel {
        sequence: i64,
        at: i64,
        reply: SyncSender<Result<(), SupervisorError>>,
    },
    Drain {
        at: i64,
        reply: SyncSender<Result<(), SupervisorError>>,
    },
    ReconnectChildAdapter {
        turns: Sender<TurnRequest<Input, Output>>,
        at: i64,
        reply: SyncSender<Result<(), SupervisorError>>,
    },
    ReconstructedExit {
        fence: TurnFence,
        at: i64,
        reason: String,
        reply: SyncSender<Result<(), SupervisorError>>,
    },
    Expire {
        at: i64,
        reply: SyncSender<Result<(), SupervisorError>>,
    },
    Close {
        at: i64,
        reply: SyncSender<Result<(), SupervisorError>>,
    },
    HandleDropped {
        reply: SyncSender<Result<(), SupervisorError>>,
    },
}

struct PendingWork<Input> {
    notification: SessionNotification<Input>,
    attempts: usize,
}

struct ActiveTurn<Input> {
    turn: ProviderTurnGeneration,
    work: Option<PendingWork<Input>>,
}

struct OwnerState<Input, Output> {
    session_id: String,
    fence: SupervisorFence,
    config: SupervisorConfig,
    repository: Box<dyn SessionLifecycleRepository + Send>,
    turns: Sender<TurnRequest<Input, Output>>,
    results: Sender<TurnResult<Output>>,
    events: Sender<LifecycleEvent>,
    phase: SupervisorPhase,
    queued: VecDeque<PendingWork<Input>>,
    active: Option<ActiveTurn<Input>>,
    last_sequence: Option<i64>,
    child_adapter_connected: bool,
    published_events: usize,
    publication_failures: usize,
    terminal: bool,
    _registration: OwnerRegistration,
}

struct OwnerRegistration {
    owner_key: String,
}

impl OwnerRegistration {
    fn acquire(session_id: &str, fence: &SupervisorFence) -> Result<Self, SupervisorStartError> {
        let owner_key = format!(
            "{}:{}:{}:{}:{}:{}",
            session_id,
            fence.generation,
            fence.token,
            fence.process.pid,
            fence.process.boot_id,
            fence.process.start_time_ticks
        );
        let mut sessions = owner_sessions()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !sessions.insert(owner_key.clone()) {
            return Err(SupervisorStartError::OwnerAlreadyRunning);
        }
        Ok(Self { owner_key })
    }
}

impl Drop for OwnerRegistration {
    fn drop(&mut self) {
        owner_sessions()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.owner_key);
    }
}

fn owner_sessions() -> &'static Mutex<HashSet<String>> {
    static SESSIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn run_owner<Input, Output>(
    state: &mut OwnerState<Input, Output>,
    commands: Receiver<SupervisorCommand<Input, Output>>,
    command_tx: &Sender<SupervisorCommand<Input, Output>>,
) where
    Input: Clone + Send + 'static,
    Output: Send + 'static,
{
    while let Ok(command) = commands.recv() {
        let keep_running = match command {
            SupervisorCommand::Notification {
                notification,
                accepted_at,
                reply,
            } => {
                let _ = reply.send(state.accept(notification, accepted_at, command_tx));
                true
            }
            SupervisorCommand::ChildExited {
                fence,
                exit,
                completed_at,
                reply,
            } => {
                let result = state.child_exited(&fence, exit, completed_at, command_tx);
                let keep_running = !state.terminal;
                let _ = reply.send(result);
                keep_running
            }
            SupervisorCommand::ReplayCommitted { event } => {
                state.publish_committed(&event);
                true
            }
            SupervisorCommand::CommitEvent { event, reply } => {
                let result = state.commit_event(&event);
                let _ = reply.send(result);
                true
            }
            SupervisorCommand::Status { reply } => {
                let _ = reply.send(Ok(state.snapshot()));
                true
            }
            SupervisorCommand::Pause { at, reply } => {
                let result = state.pause(at);
                let _ = reply.send(result);
                true
            }
            SupervisorCommand::Resume { at, reply } => {
                let result = state.resume(at, command_tx);
                let _ = reply.send(result);
                true
            }
            SupervisorCommand::Cancel {
                sequence,
                at,
                reply,
            } => {
                let result = state.cancel(sequence, at, command_tx);
                let _ = reply.send(result);
                true
            }
            SupervisorCommand::Drain { at, reply } => {
                let result = state.drain(at, command_tx);
                let keep_running = !state.terminal;
                let _ = reply.send(result);
                keep_running
            }
            SupervisorCommand::ReconnectChildAdapter { turns, at, reply } => {
                state.turns = turns;
                state.child_adapter_connected = true;
                let result = state.start_next(at, command_tx);
                let _ = reply.send(result);
                true
            }
            SupervisorCommand::ReconstructedExit {
                fence,
                at,
                reason,
                reply,
            } => {
                let result = state.reconstructed_exit(&fence, at, &reason, command_tx);
                let _ = reply.send(result);
                true
            }
            SupervisorCommand::Expire { at, reply } => {
                let result = state.terminate(TerminalReason::Expired, at);
                let keep_running = result.is_err();
                let _ = reply.send(result);
                keep_running
            }
            SupervisorCommand::Close { at, reply } => {
                let result = state.terminate(TerminalReason::ExplicitClose, at);
                let keep_running = result.is_err();
                let _ = reply.send(result);
                keep_running
            }
            SupervisorCommand::HandleDropped { reply } => {
                let result = state.terminate(TerminalReason::HandleDropped, 0);
                let keep_running = result.is_err();
                let _ = reply.send(result);
                keep_running
            }
        };
        if !keep_running {
            return;
        }
    }

    if !state.terminal {
        let _ = state.terminate(TerminalReason::HandleDropped, 0);
    }
}

impl<Input, Output> OwnerState<Input, Output>
where
    Input: Clone + Send + 'static,
    Output: Send + 'static,
{
    fn accept(
        &mut self,
        notification: SessionNotification<Input>,
        accepted_at: i64,
        command_tx: &Sender<SupervisorCommand<Input, Output>>,
    ) -> Result<AcceptedNotification, SupervisorError> {
        if self.phase == SupervisorPhase::Draining {
            return Err(SupervisorError::Draining);
        }
        if notification
            .expires_at
            .is_some_and(|expiry| accepted_at >= expiry)
        {
            return Err(SupervisorError::Expired);
        }
        if notification.sequence <= 0
            || self
                .last_sequence
                .is_some_and(|last| notification.sequence <= last)
        {
            return Err(SupervisorError::DuplicateOrStaleSequence);
        }
        if self.queued.len() >= self.config.queue_capacity {
            return Err(SupervisorError::QueueFull);
        }
        validate_notification(&self.session_id, &notification)?;

        self.append_and_publish(
            "supervisor_work_accepted",
            &format!("notification:{}", notification.sequence),
            &format!("sequence={}", notification.sequence),
            accepted_at,
        )?;
        let sequence = notification.sequence;
        self.last_sequence = Some(sequence);
        self.queued.push_back(PendingWork {
            notification,
            attempts: 0,
        });
        self.start_next(accepted_at, command_tx)?;
        Ok(AcceptedNotification {
            sequence,
            active_generation: self
                .active
                .as_ref()
                .map(|active| active.turn.generation_id.clone()),
            queued_notifications: self.queued.len(),
        })
    }

    fn start_next(
        &mut self,
        at: i64,
        command_tx: &Sender<SupervisorCommand<Input, Output>>,
    ) -> Result<(), SupervisorError> {
        if self.active.is_some()
            || self.phase == SupervisorPhase::Paused
            || !self.child_adapter_connected
        {
            return Ok(());
        }

        while let Some(mut work) = self.queued.pop_front() {
            if work
                .notification
                .expires_at
                .is_some_and(|expiry| at >= expiry)
            {
                self.append_and_publish(
                    "supervisor_work_expired",
                    &format!("notification:{}", work.notification.sequence),
                    &format!("sequence={}", work.notification.sequence),
                    at,
                )?;
                continue;
            }
            let Some(turn) = work.notification.turns.pop_front() else {
                self.append_and_publish(
                    "supervisor_generation_exhausted",
                    &format!("notification:{}", work.notification.sequence),
                    &format!("sequence={}", work.notification.sequence),
                    at,
                )?;
                continue;
            };
            work.attempts += 1;
            self.repository.start_provider_turn(&turn)?;
            let fence = turn_fence(&turn);
            let request = TurnRequest {
                session_id: self.session_id.clone(),
                owner_fence: self.fence.clone(),
                turn: turn.clone(),
                notification: TurnInput {
                    sequence: work.notification.sequence,
                    input: work.notification.input.clone(),
                },
                completion: TurnCompletion {
                    fence: fence.clone(),
                    commands: Some(command_tx.clone()),
                },
            };
            match self.turns.send(request) {
                Ok(()) => {
                    self.active = Some(ActiveTurn {
                        turn,
                        work: Some(work),
                    });
                    return Ok(());
                }
                Err(error) => {
                    let mut request = error.0;
                    request.completion.disarm();
                    self.transition_and_publish(
                        &fence,
                        TurnState::Running,
                        "supervisor_child_adapter_disconnected",
                        &format!("sequence={}", work.notification.sequence),
                        at,
                    )?;
                    self.queued.push_front(work);
                    self.child_adapter_connected = false;
                    return Ok(());
                }
            }
        }

        if self.phase == SupervisorPhase::Draining {
            self.terminate(TerminalReason::Drained, at)?;
        }
        Ok(())
    }

    fn child_exited(
        &mut self,
        fence: &TurnFence,
        exit: ChildExit<Output>,
        at: i64,
        command_tx: &Sender<SupervisorCommand<Input, Output>>,
    ) -> Result<(), SupervisorError> {
        let Some(active) = self.active.as_ref() else {
            return Err(SupervisorError::StaleCommand);
        };
        if turn_fence(&active.turn) != *fence || active.work.is_none() {
            return Err(SupervisorError::StaleCommand);
        }
        let active = self.active.take().expect("active turn checked above");
        let work = active.work.expect("normal active turn checked above");

        match exit {
            ChildExit::Completed(output) => {
                self.transition_and_publish(
                    fence,
                    active.turn.state,
                    "supervisor_turn_completed",
                    &format!("sequence={}", work.notification.sequence),
                    at,
                )?;
                self.publish_result(&active.turn, &work, TurnOutcome::Completed(output));
            }
            ChildExit::Failed(failure) => {
                let reason = failure.to_string();
                self.transition_and_publish(
                    fence,
                    active.turn.state,
                    "supervisor_turn_failed",
                    &reason,
                    at,
                )?;
                if work.attempts <= self.config.max_retries {
                    if work.notification.turns.is_empty() {
                        self.publish_result(
                            &active.turn,
                            &work,
                            TurnOutcome::GenerationExhausted(reason.clone()),
                        );
                        self.append_and_publish(
                            "supervisor_generation_exhausted",
                            &active.turn.generation_id,
                            &reason,
                            at,
                        )?;
                    } else {
                        self.publish_result(
                            &active.turn,
                            &work,
                            TurnOutcome::RetryScheduled(reason),
                        );
                        self.queued.push_front(work);
                    }
                } else {
                    self.publish_result(
                        &active.turn,
                        &work,
                        TurnOutcome::RetryExhausted(reason.clone()),
                    );
                    self.append_and_publish(
                        "supervisor_retry_exhausted",
                        &active.turn.generation_id,
                        &reason,
                        at,
                    )?;
                }
            }
        }
        self.start_next(at, command_tx)
    }

    fn pause(&mut self, at: i64) -> Result<(), SupervisorError> {
        if self.phase == SupervisorPhase::Draining {
            return Err(SupervisorError::Draining);
        }
        if self.phase != SupervisorPhase::Paused {
            self.append_and_publish("supervisor_paused", "lifecycle", "paused", at)?;
            self.phase = SupervisorPhase::Paused;
        }
        Ok(())
    }

    fn resume(
        &mut self,
        at: i64,
        command_tx: &Sender<SupervisorCommand<Input, Output>>,
    ) -> Result<(), SupervisorError> {
        if self.phase == SupervisorPhase::Draining {
            return Err(SupervisorError::Draining);
        }
        if self.phase == SupervisorPhase::Paused {
            self.append_and_publish("supervisor_resumed", "lifecycle", "running", at)?;
            self.phase = SupervisorPhase::Running;
            self.start_next(at, command_tx)?;
        }
        Ok(())
    }

    fn cancel(
        &mut self,
        sequence: i64,
        at: i64,
        command_tx: &Sender<SupervisorCommand<Input, Output>>,
    ) -> Result<(), SupervisorError> {
        if self
            .active
            .as_ref()
            .and_then(|active| active.work.as_ref())
            .is_some_and(|work| work.notification.sequence == sequence)
        {
            let active = self.active.take().expect("active sequence checked above");
            let work = active.work.expect("normal active sequence checked above");
            self.transition_and_publish(
                &turn_fence(&active.turn),
                active.turn.state,
                "supervisor_turn_cancelled",
                &format!("sequence={sequence}"),
                at,
            )?;
            self.publish_result(&active.turn, &work, TurnOutcome::Cancelled);
            return self.start_next(at, command_tx);
        }
        let Some(index) = self
            .queued
            .iter()
            .position(|work| work.notification.sequence == sequence)
        else {
            return Err(SupervisorError::NotFound);
        };
        self.queued.remove(index);
        self.append_and_publish(
            "supervisor_queued_work_cancelled",
            &format!("notification:{sequence}"),
            &format!("sequence={sequence}"),
            at,
        )?;
        Ok(())
    }

    fn drain(
        &mut self,
        at: i64,
        command_tx: &Sender<SupervisorCommand<Input, Output>>,
    ) -> Result<(), SupervisorError> {
        if self.phase != SupervisorPhase::Draining {
            self.append_and_publish("supervisor_drain_started", "lifecycle", "draining", at)?;
            self.phase = SupervisorPhase::Draining;
        }
        self.start_next(at, command_tx)
    }

    fn reconstructed_exit(
        &mut self,
        fence: &TurnFence,
        at: i64,
        reason: &str,
        command_tx: &Sender<SupervisorCommand<Input, Output>>,
    ) -> Result<(), SupervisorError> {
        let Some(active) = self.active.as_ref() else {
            return Err(SupervisorError::StaleCommand);
        };
        if turn_fence(&active.turn) != *fence || active.work.is_some() {
            return Err(SupervisorError::StaleCommand);
        }
        let active = self
            .active
            .take()
            .expect("reconstructed turn checked above");
        self.transition_and_publish(
            fence,
            active.turn.state,
            "supervisor_reconstructed_turn_exited",
            reason,
            at,
        )?;
        self.start_next(at, command_tx)
    }

    fn commit_event(
        &mut self,
        event: &NewLifecycleEvent,
    ) -> Result<LifecycleEvent, SupervisorError> {
        let committed = self
            .repository
            .append_lifecycle_event(&self.session_id, event)?;
        self.publish_committed(&committed);
        Ok(committed)
    }

    fn append_and_publish(
        &mut self,
        event_type: &str,
        correlation_id: &str,
        payload: &str,
        at: i64,
    ) -> Result<LifecycleEvent, SupervisorError> {
        let event = NewLifecycleEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: event_type.to_owned(),
            cause_event_id: None,
            correlation_id: correlation_id.to_owned(),
            payload: payload.to_owned(),
            created_at: at,
        };
        self.commit_event(&event)
    }

    fn transition_and_publish(
        &mut self,
        fence: &TurnFence,
        from: TurnState,
        event_type: &str,
        payload: &str,
        at: i64,
    ) -> Result<LifecycleEvent, SupervisorError> {
        let event = NewLifecycleEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: event_type.to_owned(),
            cause_event_id: None,
            correlation_id: fence.generation_id.clone(),
            payload: payload.to_owned(),
            created_at: at,
        };
        let committed = self.repository.transition_turn_and_append_event(
            fence,
            from,
            TurnState::Exited,
            &event,
        )?;
        self.publish_committed(&committed);
        Ok(committed)
    }

    fn publish_committed(&mut self, event: &LifecycleEvent) {
        if self.events.send(event.clone()).is_err() {
            self.publication_failures += 1;
            return;
        }
        match self.repository.record_event_disposition(
            &event.event_id,
            &self.config.consumer_id,
            EventDisposition::Applied,
            event.created_at,
        ) {
            Ok(DispositionWrite::Recorded | DispositionWrite::AlreadyRecorded) => {
                self.published_events += 1;
            }
            Err(_) => {
                self.publication_failures += 1;
            }
        }
    }

    fn publish_result(
        &self,
        turn: &ProviderTurnGeneration,
        work: &PendingWork<Input>,
        outcome: TurnOutcome<Output>,
    ) {
        let _ = self.results.send(TurnResult {
            session_id: self.session_id.clone(),
            generation_id: turn.generation_id.clone(),
            spawn_invocation_id: turn.spawn_invocation_id.clone(),
            notification_sequence: work.notification.sequence,
            outcome,
        });
    }

    fn terminate(&mut self, reason: TerminalReason, at: i64) -> Result<(), SupervisorError> {
        if self.terminal {
            return Ok(());
        }
        if let Some(active) = self.active.take() {
            let fence = turn_fence(&active.turn);
            self.transition_and_publish(
                &fence,
                active.turn.state,
                "supervisor_turn_terminated",
                reason.event_type(),
                at,
            )?;
            if let Some(work) = active.work {
                self.publish_result(&active.turn, &work, TurnOutcome::Terminated(reason.clone()));
            }
        }
        while let Some(work) = self.queued.pop_front() {
            self.append_and_publish(
                "supervisor_queued_work_terminated",
                &format!("notification:{}", work.notification.sequence),
                reason.event_type(),
                at,
            )?;
        }
        self.append_and_publish(reason.event_type(), "lifecycle", reason.event_type(), at)?;
        self.repository
            .release_supervisor_lease(&self.session_id, &self.fence)?;
        self.terminal = true;
        Ok(())
    }

    fn snapshot(&self) -> SupervisorSnapshot {
        SupervisorSnapshot {
            session_id: self.session_id.clone(),
            fence: self.fence.clone(),
            phase: self.phase,
            active_generation: self
                .active
                .as_ref()
                .map(|active| active.turn.generation_id.clone()),
            active_sequence: self
                .active
                .as_ref()
                .and_then(|active| active.work.as_ref())
                .map(|work| work.notification.sequence),
            queued_sequences: self
                .queued
                .iter()
                .map(|work| work.notification.sequence)
                .collect(),
            child_adapter_connected: self.child_adapter_connected,
            published_events: self.published_events,
            publication_failures: self.publication_failures,
        }
    }
}

fn validate_config(config: &SupervisorConfig) -> Result<(), SupervisorStartError> {
    if config.queue_capacity == 0 {
        return Err(SupervisorStartError::InvalidConfig("queue_capacity"));
    }
    if config.consumer_id.is_empty() {
        return Err(SupervisorStartError::InvalidConfig("consumer_id"));
    }
    if config.reconstruction_limit == 0 {
        return Err(SupervisorStartError::InvalidConfig("reconstruction_limit"));
    }
    Ok(())
}

fn validate_notification<Input>(
    session_id: &str,
    notification: &SessionNotification<Input>,
) -> Result<(), SupervisorError> {
    if notification.turns.is_empty() {
        return Ok(());
    }
    if notification.turns.iter().any(|turn| {
        turn.session_id.as_deref() != Some(session_id) || turn.state != TurnState::Running
    }) {
        return Err(SupervisorError::InvalidTurnFence);
    }
    Ok(())
}

fn acquire_or_replace(
    repository: &mut dyn SessionLifecycleRepository,
    session_id: &str,
    candidate: &SupervisorFence,
    acquired_at: i64,
    observer: &dyn ProcessObserver,
    reconstruction: &SessionReconstruction,
) -> Result<(), SupervisorStartError> {
    let Some(current) = reconstruction.lease.as_ref() else {
        repository.acquire_supervisor_lease(session_id, candidate, acquired_at)?;
        return Ok(());
    };
    if current.fence == *candidate {
        match repository.acquire_supervisor_lease(session_id, candidate, acquired_at)? {
            LeaseAcquire::Acquired | LeaseAcquire::AlreadyOwned => return Ok(()),
        }
    }

    match observer.observe(&current.fence.process) {
        ProcessObservation::ExactLive => Err(SupervisorStartError::ExistingOwnerLive),
        ProcessObservation::Unknown(error) => {
            Err(SupervisorStartError::ProcessObservationUnknown(error))
        }
        ProcessObservation::Dead | ProcessObservation::Reused(_) => {
            let expected = current
                .fence
                .generation
                .checked_add(1)
                .ok_or(SupervisorStartError::ReplacementGenerationExhausted)?;
            if candidate.generation != expected {
                return Err(SupervisorStartError::ReplacementGenerationMismatch {
                    expected,
                    actual: candidate.generation,
                });
            }
            repository.replace_supervisor_lease(
                session_id,
                &current.fence,
                candidate,
                acquired_at,
            )?;
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn reconcile_reconstructed_turn<Input>(
    repository: &mut dyn SessionLifecycleRepository,
    session_id: &str,
    owner_fence: &SupervisorFence,
    observer: &dyn ProcessObserver,
    active_turn: Option<ProviderTurnGeneration>,
    at: i64,
    startup_events: &mut Vec<LifecycleEvent>,
) -> Result<Option<ActiveTurn<Input>>, SupervisorStartError> {
    let Some(turn) = active_turn else {
        return Ok(None);
    };
    match observer.observe(&turn.child) {
        ProcessObservation::ExactLive => Ok(Some(ActiveTurn { turn, work: None })),
        ProcessObservation::Unknown(error) => {
            let _ = repository.release_supervisor_lease(session_id, owner_fence);
            Err(SupervisorStartError::ProcessObservationUnknown(error))
        }
        ProcessObservation::Dead | ProcessObservation::Reused(_) => {
            let fence = turn_fence(&turn);
            let event = NewLifecycleEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                event_type: "supervisor_reconstructed_child_stale".to_owned(),
                cause_event_id: None,
                correlation_id: turn.generation_id.clone(),
                payload: "reconstructed child is dead or PID-reused".to_owned(),
                created_at: at,
            };
            let committed = repository.transition_turn_and_append_event(
                &fence,
                turn.state,
                TurnState::Exited,
                &event,
            )?;
            startup_events.push(committed);
            Ok(None)
        }
    }
}

fn turn_fence(turn: &ProviderTurnGeneration) -> TurnFence {
    TurnFence {
        session_id: turn
            .session_id
            .clone()
            .expect("supervisor turns are validated as session-bound"),
        generation_id: turn.generation_id.clone(),
        spawn_invocation_id: turn.spawn_invocation_id.clone(),
    }
}
