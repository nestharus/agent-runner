use std::collections::VecDeque;
use std::fmt;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionNotification<Input> {
    pub sequence: u64,
    pub input: Input,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedNotification {
    pub sequence: u64,
    pub active_generation: Option<u64>,
    pub queued_notifications: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SupervisorSnapshot {
    pub session_id: String,
    pub active_generation: Option<u64>,
    pub active_sequence: Option<u64>,
    pub queued_sequences: Vec<u64>,
}

#[derive(Debug)]
pub struct TurnRequest<Input, Output> {
    pub session_id: String,
    pub generation: u64,
    pub notification: SessionNotification<Input>,
    pub completion: TurnCompletion<Input, Output>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnResult<Output> {
    pub session_id: String,
    pub generation: u64,
    pub notification_sequence: u64,
    pub output: Output,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SupervisorError {
    Closed,
    Busy,
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("session supervisor is closed"),
            Self::Busy => formatter.write_str("session supervisor still has active or queued work"),
        }
    }
}

impl std::error::Error for SupervisorError {}

#[derive(Debug)]
pub struct TurnCompletion<Input, Output> {
    generation: u64,
    commands: Sender<SupervisorCommand<Input, Output>>,
}

impl<Input, Output> TurnCompletion<Input, Output> {
    pub fn complete(self, output: Output) -> Result<(), SupervisorError> {
        self.commands
            .send(SupervisorCommand::ChildExited {
                generation: self.generation,
                output,
            })
            .map_err(|_| SupervisorError::Closed)
    }
}

pub struct SessionSupervisor<Input, Output> {
    commands: Sender<SupervisorCommand<Input, Output>>,
    owner: Option<thread::JoinHandle<()>>,
}

impl<Input, Output> SessionSupervisor<Input, Output>
where
    Input: Send + 'static,
    Output: Send + 'static,
{
    pub fn start(
        session_id: impl Into<String>,
        turns: Sender<TurnRequest<Input, Output>>,
    ) -> (Self, Receiver<TurnResult<Output>>) {
        let session_id = session_id.into();
        let (command_tx, command_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let owner_commands = command_tx.clone();
        let owner = thread::spawn(move || {
            run_owner(session_id, command_rx, owner_commands, turns, result_tx);
        });

        (
            Self {
                commands: command_tx,
                owner: Some(owner),
            },
            result_rx,
        )
    }

    pub fn notify(
        &self,
        notification: SessionNotification<Input>,
    ) -> Result<AcceptedNotification, SupervisorError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(0);
        self.commands
            .send(SupervisorCommand::Notification {
                notification,
                reply: reply_tx,
            })
            .map_err(|_| SupervisorError::Closed)?;
        reply_rx.recv().map_err(|_| SupervisorError::Closed)
    }

    pub fn status(&self) -> Result<SupervisorSnapshot, SupervisorError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(0);
        self.commands
            .send(SupervisorCommand::Status { reply: reply_tx })
            .map_err(|_| SupervisorError::Closed)?;
        reply_rx.recv().map_err(|_| SupervisorError::Closed)
    }

    pub fn shutdown(mut self) -> Result<(), SupervisorError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(0);
        self.commands
            .send(SupervisorCommand::Shutdown { reply: reply_tx })
            .map_err(|_| SupervisorError::Closed)?;
        let stopped = reply_rx.recv().map_err(|_| SupervisorError::Closed)?;
        if !stopped {
            return Err(SupervisorError::Busy);
        }
        if let Some(owner) = self.owner.take() {
            owner.join().map_err(|_| SupervisorError::Closed)?;
        }
        Ok(())
    }
}

enum SupervisorCommand<Input, Output> {
    Notification {
        notification: SessionNotification<Input>,
        reply: SyncSender<AcceptedNotification>,
    },
    ChildExited {
        generation: u64,
        output: Output,
    },
    Status {
        reply: SyncSender<SupervisorSnapshot>,
    },
    Shutdown {
        reply: SyncSender<bool>,
    },
}

struct ActiveTurn {
    generation: u64,
    notification_sequence: u64,
}

fn run_owner<Input, Output>(
    session_id: String,
    commands: Receiver<SupervisorCommand<Input, Output>>,
    command_tx: Sender<SupervisorCommand<Input, Output>>,
    turns: Sender<TurnRequest<Input, Output>>,
    results: Sender<TurnResult<Output>>,
) where
    Input: Send + 'static,
    Output: Send + 'static,
{
    let mut queued = VecDeque::new();
    let mut active = None;
    let mut next_generation = 1;

    while let Ok(command) = commands.recv() {
        match command {
            SupervisorCommand::Notification {
                notification,
                reply,
            } => {
                let sequence = notification.sequence;
                queued.push_back(notification);
                start_next_turn(
                    &session_id,
                    &mut queued,
                    &mut active,
                    &mut next_generation,
                    &command_tx,
                    &turns,
                );
                let _ = reply.send(AcceptedNotification {
                    sequence,
                    active_generation: active.as_ref().map(|turn| turn.generation),
                    queued_notifications: queued.len(),
                });
            }
            SupervisorCommand::ChildExited { generation, output } => {
                let Some(completed) = active.as_ref() else {
                    continue;
                };
                if completed.generation != generation {
                    continue;
                }
                let completed = active.take().expect("active turn checked above");
                let _ = results.send(TurnResult {
                    session_id: session_id.clone(),
                    generation,
                    notification_sequence: completed.notification_sequence,
                    output,
                });
                start_next_turn(
                    &session_id,
                    &mut queued,
                    &mut active,
                    &mut next_generation,
                    &command_tx,
                    &turns,
                );
            }
            SupervisorCommand::Status { reply } => {
                let _ = reply.send(snapshot(&session_id, active.as_ref(), &queued));
            }
            SupervisorCommand::Shutdown { reply } => {
                let stopped = active.is_none() && queued.is_empty();
                let _ = reply.send(stopped);
                if stopped {
                    break;
                }
            }
        }
    }
}

fn start_next_turn<Input, Output>(
    session_id: &str,
    queued: &mut VecDeque<SessionNotification<Input>>,
    active: &mut Option<ActiveTurn>,
    next_generation: &mut u64,
    commands: &Sender<SupervisorCommand<Input, Output>>,
    turns: &Sender<TurnRequest<Input, Output>>,
) {
    if active.is_some() {
        return;
    }
    let Some(notification) = queued.pop_front() else {
        return;
    };

    let generation = *next_generation;
    *next_generation += 1;
    let notification_sequence = notification.sequence;
    let request = TurnRequest {
        session_id: session_id.to_owned(),
        generation,
        notification,
        completion: TurnCompletion {
            generation,
            commands: commands.clone(),
        },
    };
    match turns.send(request) {
        Ok(()) => {
            *active = Some(ActiveTurn {
                generation,
                notification_sequence,
            });
        }
        Err(error) => queued.push_front(error.0.notification),
    }
}

fn snapshot<Input>(
    session_id: &str,
    active: Option<&ActiveTurn>,
    queued: &VecDeque<SessionNotification<Input>>,
) -> SupervisorSnapshot {
    SupervisorSnapshot {
        session_id: session_id.to_owned(),
        active_generation: active.map(|turn| turn.generation),
        active_sequence: active.map(|turn| turn.notification_sequence),
        queued_sequences: queued
            .iter()
            .map(|notification| notification.sequence)
            .collect(),
    }
}
