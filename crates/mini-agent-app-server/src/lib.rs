use mini_agent_core::EventEnvelope;
use mini_agent_core::EventSink;
use mini_agent_core::Model;
use mini_agent_core::RunControl;
use mini_agent_core::SteeringMode;
use mini_agent_core::Thread;
use mini_agent_core::ThreadId;
use mini_agent_core::ThreadStart;
use mini_agent_core::TurnCancel;
use mini_agent_core::TurnId;
use mini_agent_core::TurnInput;
use mini_agent_core::TurnInputMode;
use mini_agent_core::TurnStart;
use mini_agent_core::TurnSubmission;
use std::fmt;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

const EVENT_BUFFER: usize = 256;
const COMMAND_BUFFER: usize = 32;

/// A bounded error returned by the in-process control-plane adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppServerError {
    Closed,
    Busy,
    NoActiveTurn,
    TurnNotActive(TurnId),
    InvalidInputMode(TurnInputMode),
    InputQueue(String),
    Disconnected,
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

impl fmt::Display for AppServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("thread is closed"),
            Self::Busy => formatter.write_str("thread already has an active turn"),
            Self::NoActiveTurn => formatter.write_str("thread has no active turn"),
            Self::TurnNotActive(turn_id) => {
                write!(formatter, "turn {} is not active", turn_id.as_str())
            }
            Self::InvalidInputMode(mode) => write!(formatter, "cannot start turn with {mode:?}"),
            Self::InputQueue(error) => write!(formatter, "cannot submit turn input: {error}"),
            Self::Disconnected => formatter.write_str("app-server worker is unavailable"),
        }
    }
}

impl std::error::Error for AppServerError {}

/// A thin in-process app-server facade over one core-owned Thread.
///
/// The adapter owns command serialization and event fan-out only. Model
/// inference, tool execution, context management, turn identity, and stop
/// semantics remain implemented by `mini-agent-core::Thread`.
pub struct AppServer<M> {
    commands: mpsc::Sender<Command>,
    events: broadcast::Sender<EventEnvelope>,
    thread_id: ThreadId,
    _model: std::marker::PhantomData<fn() -> M>,
}

impl<M> Clone for AppServer<M> {
    fn clone(&self) -> Self {
        Self {
            commands: self.commands.clone(),
            events: self.events.clone(),
            thread_id: self.thread_id.clone(),
            _model: std::marker::PhantomData,
        }
    }
}

impl<M> AppServer<M>
where
    M: Model + Send + 'static,
{
    /// Starts an in-process worker for `thread`.
    ///
    /// The caller must construct the adapter inside an active Tokio runtime.
    /// The worker owns the Thread exclusively, so every command is serialized
    /// before it reaches the core execution kernel.
    pub fn new(start: ThreadStart, mut thread: Thread<M>) -> Self {
        thread.set_id(start.thread_id.clone());
        let (commands, command_receiver) = mpsc::channel(COMMAND_BUFFER);
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        tokio::spawn(worker_loop(thread, command_receiver, events.clone()));
        Self {
            commands,
            events,
            thread_id: start.thread_id,
            _model: std::marker::PhantomData,
        }
    }

    pub fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Subscribes to the ordered event stream emitted by the core Thread.
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.events.subscribe()
    }

    /// Starts, steers, or queues a turn according to the typed input mode.
    pub async fn turn_start(&self, request: TurnStart) -> Result<TurnSubmission, AppServerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Start { request, reply })
            .await
            .map_err(|_| AppServerError::Disconnected)?;
        response.await.map_err(|_| AppServerError::Disconnected)?
    }

    /// Requests cooperative cancellation of the active turn.
    pub async fn turn_cancel(&self, request: TurnCancel) -> Result<(), AppServerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Cancel { request, reply })
            .await
            .map_err(|_| AppServerError::Disconnected)?;
        response.await.map_err(|_| AppServerError::Disconnected)?
    }
}

enum Command {
    Start {
        request: TurnStart,
        reply: oneshot::Sender<Result<TurnSubmission, AppServerError>>,
    },
    Cancel {
        request: TurnCancel,
        reply: oneshot::Sender<Result<(), AppServerError>>,
    },
}

struct BroadcastSink {
    events: broadcast::Sender<EventEnvelope>,
}

impl EventSink for BroadcastSink {
    fn emit(&mut self, event: EventEnvelope) {
        let _ = self.events.send(event);
    }
}

async fn worker_loop<M>(
    mut thread: Thread<M>,
    mut commands: mpsc::Receiver<Command>,
    events: broadcast::Sender<EventEnvelope>,
) where
    M: Model + Send + 'static,
{
    let control = Arc::new(RunControl::new());
    while let Some(command) = commands.recv().await {
        match command {
            Command::Start { request, reply } => {
                if !matches!(
                    request.input.mode,
                    TurnInputMode::Start | TurnInputMode::StartIfIdle
                ) {
                    let _ = reply.send(Err(AppServerError::InvalidInputMode(request.input.mode)));
                    continue;
                }
                if thread.status() == mini_agent_core::ThreadStatus::Closed {
                    let _ = reply.send(Err(AppServerError::Closed));
                    continue;
                }
                if thread.status() == mini_agent_core::ThreadStatus::Running {
                    let _ = reply.send(Err(AppServerError::Busy));
                    continue;
                }

                let mut next_input = Some(request.input);
                let mut initial_reply = Some(reply);
                loop {
                    let input = next_input
                        .take()
                        .expect("app-server turn input must exist before execution");
                    let turn_id = thread.next_turn_id();
                    if let Some(reply) = initial_reply.take() {
                        let _ = reply.send(Ok(TurnSubmission::Started {
                            turn_id: turn_id.clone(),
                        }));
                    }
                    let input = TurnInput::new(TurnInputMode::Start, input.text);
                    let mut sink = BroadcastSink {
                        events: events.clone(),
                    };
                    let mut turn = Box::pin(thread.run_turn_with_events(
                        input,
                        &mut sink,
                        &control,
                        SteeringMode::StopAtCheckpoint,
                    ));
                    loop {
                        tokio::select! {
                            _result = &mut turn => break,
                            Some(command) = commands.recv() => handle_running_command(command, &control, &turn_id),
                            else => return,
                        }
                    }
                    next_input = control
                        .take_steer_input()
                        .or_else(|| control.take_follow_up_input());
                    if next_input.is_none() {
                        break;
                    }
                }
            }
            Command::Cancel { reply, .. } => {
                let _ = reply.send(Err(AppServerError::NoActiveTurn));
            }
        }
    }
}

fn handle_running_command(command: Command, control: &RunControl, turn_id: &TurnId) {
    match command {
        Command::Start { request, reply } => {
            let result = match request.input.mode {
                TurnInputMode::Steer => control
                    .submit(request.input)
                    .map(|()| TurnSubmission::Steered {
                        turn_id: turn_id.clone(),
                    })
                    .map_err(|error| AppServerError::InputQueue(error.to_string())),
                TurnInputMode::FollowUp => control
                    .submit(request.input)
                    .map(|()| TurnSubmission::Queued)
                    .map_err(|error| AppServerError::InputQueue(error.to_string())),
                mode => Ok(TurnSubmission::NotSubmitted {
                    reason: format!("thread is busy; cannot submit {mode:?}"),
                }),
            };
            let _ = reply.send(result);
        }
        Command::Cancel { request, reply } => {
            let result = if request.turn_id == *turn_id {
                control.request_cancel();
                Ok(())
            } else {
                Err(AppServerError::TurnNotActive(request.turn_id))
            };
            let _ = reply.send(result);
        }
    }
}
