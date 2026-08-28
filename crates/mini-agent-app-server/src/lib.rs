use mini_agent_core::EventEnvelope;
use mini_agent_core::EventSink;
use mini_agent_core::Model;
use mini_agent_core::RunControl;
use mini_agent_core::SteeringMode;
use mini_agent_core::Thread;
use mini_agent_core::ThreadCheckpoint;
use mini_agent_core::ThreadId;
use mini_agent_core::ThreadStart;
use mini_agent_core::TurnCancel;
use mini_agent_core::TurnId;
use mini_agent_core::TurnInput;
use mini_agent_core::TurnInputMode;
use mini_agent_core::TurnStart;
use mini_agent_core::TurnSubmission;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use tokio::sync::Notify;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

const EVENT_BUFFER: usize = 256;
const COMMAND_BUFFER: usize = 32;

#[derive(Clone)]
pub struct ApprovalBroker {
    state: Arc<Mutex<ApprovalState>>,
    notify: Arc<Notify>,
    next_id: Arc<AtomicU64>,
}

struct ApprovalState {
    queued: std::collections::VecDeque<ApprovalRequest>,
    responders: HashMap<String, std::sync::mpsc::Sender<bool>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub action: String,
}

/// Creates additional core Threads for a service lifecycle request.
///
/// Implementations belong to the host layer and may construct a fresh model,
/// tool registry, and policy for each identity. The app-server only invokes
/// this factory and never serializes its captured host state.
pub trait ThreadFactory<M>: Send + Sync + 'static {
    fn create(&self, thread_id: ThreadId) -> Result<Thread<M>, AppServerError>;
}

impl<M, F> ThreadFactory<M> for F
where
    F: Fn(ThreadId) -> Result<Thread<M>, AppServerError> + Send + Sync + 'static,
{
    fn create(&self, thread_id: ThreadId) -> Result<Thread<M>, AppServerError> {
        self(thread_id)
    }
}

impl ApprovalBroker {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ApprovalState {
                queued: std::collections::VecDeque::new(),
                responders: HashMap::new(),
            })),
            notify: Arc::new(Notify::new()),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Called by a synchronous host approval callback. The callback waits
    /// until the external client answers the corresponding request.
    pub fn request(&self, action: &str) -> Result<bool, String> {
        let request_id = format!("approval-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (sender, receiver) = std::sync::mpsc::channel();
        let request = ApprovalRequest {
            request_id: request_id.clone(),
            action: action.to_string(),
        };
        {
            let mut state = self.state.lock().unwrap();
            state.responders.insert(request_id, sender);
            state.queued.push_back(request);
        }
        self.notify.notify_one();
        receiver
            .recv()
            .map_err(|_| "approval client disconnected".to_string())
    }

    pub async fn next_request(&self) -> ApprovalRequest {
        loop {
            if let Some(request) = self.state.lock().unwrap().queued.pop_front() {
                return request;
            }
            self.notify.notified().await;
        }
    }

    pub fn respond(&self, request_id: &str, approved: bool) -> Result<(), String> {
        let sender = self
            .state
            .lock()
            .unwrap()
            .responders
            .remove(request_id)
            .ok_or_else(|| format!("unknown approval request: {request_id}"))?;
        sender
            .send(approved)
            .map_err(|_| "approval callback is no longer waiting".to_string())
    }
}

impl Default for ApprovalBroker {
    fn default() -> Self {
        Self::new()
    }
}

pub mod client;
pub mod json_rpc;

pub use client::LocalAppServerClient;
pub use json_rpc::AppServerConnection;
pub use json_rpc::serve_stdio;
pub use json_rpc::serve_stdio_with_approval;

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
    TurnNotFound(TurnId),
    Checkpoint(String),
    ThreadNotFound(ThreadId),
    ThreadAlreadyExists(ThreadId),
    ThreadFactoryUnavailable,
}

/// A settled turn record retained by the service for inspection.
#[derive(Clone, Debug, PartialEq)]
pub struct SettledTurn {
    pub id: TurnId,
    pub status: mini_agent_core::TurnStatus,
    pub outcome: Option<mini_agent_core::RunOutcome>,
    pub error: Option<String>,
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
            Self::TurnNotFound(turn_id) => {
                write!(formatter, "turn {} is not available", turn_id.as_str())
            }
            Self::Checkpoint(error) => write!(formatter, "checkpoint unavailable: {error}"),
            Self::ThreadNotFound(thread_id) => {
                write!(formatter, "thread {} is not available", thread_id.as_str())
            }
            Self::ThreadAlreadyExists(thread_id) => {
                write!(formatter, "thread {} already exists", thread_id.as_str())
            }
            Self::ThreadFactoryUnavailable => formatter.write_str("thread factory is unavailable"),
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
    thread_ids: Arc<Mutex<Vec<ThreadId>>>,
    factory: Option<Arc<dyn ThreadFactory<M>>>,
    _model: std::marker::PhantomData<fn() -> M>,
}

impl<M> Clone for AppServer<M> {
    fn clone(&self) -> Self {
        Self {
            commands: self.commands.clone(),
            events: self.events.clone(),
            thread_id: self.thread_id.clone(),
            thread_ids: self.thread_ids.clone(),
            factory: self.factory.clone(),
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
        Self::with_threads(start, vec![thread])
    }

    /// Starts a service over several preconfigured Threads.
    ///
    /// The first supplied thread is assigned the default `start.thread_id`;
    /// additional threads retain their identities. Turns are serialized by
    /// the service worker, while lifecycle and checkpoint operations remain
    /// addressed by thread identity.
    pub fn with_threads(start: ThreadStart, mut threads: Vec<Thread<M>>) -> Self {
        assert!(
            !threads.is_empty(),
            "app-server requires at least one thread"
        );
        threads[0].set_id(start.thread_id.clone());
        Self::with_threads_and_factory(start, threads, None)
    }

    pub fn with_thread_factory<F>(start: ThreadStart, threads: Vec<Thread<M>>, factory: F) -> Self
    where
        F: ThreadFactory<M>,
    {
        Self::with_threads_and_factory(start, threads, Some(Arc::new(factory)))
    }

    fn with_threads_and_factory(
        start: ThreadStart,
        mut threads: Vec<Thread<M>>,
        factory: Option<Arc<dyn ThreadFactory<M>>>,
    ) -> Self {
        assert!(
            !threads.is_empty(),
            "app-server requires at least one thread"
        );
        threads[0].set_id(start.thread_id.clone());
        let thread_ids = threads
            .iter()
            .map(|thread| thread.id().clone())
            .collect::<Vec<_>>();
        assert!(
            thread_ids
                .iter()
                .enumerate()
                .all(|(index, id)| !thread_ids[..index].contains(id)),
            "app-server thread identities must be unique"
        );
        let thread_ids = Arc::new(Mutex::new(thread_ids));
        let (commands, command_receiver) = mpsc::channel(COMMAND_BUFFER);
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        tokio::spawn(worker_loop(
            threads,
            command_receiver,
            events.clone(),
            thread_ids.clone(),
            factory.clone(),
        ));
        Self {
            commands,
            events,
            thread_id: start.thread_id,
            thread_ids,
            factory,
            _model: std::marker::PhantomData,
        }
    }

    pub fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    pub fn thread_ids(&self) -> Vec<ThreadId> {
        self.thread_ids.lock().unwrap().clone()
    }

    pub fn has_thread(&self, thread_id: &ThreadId) -> bool {
        self.thread_ids
            .lock()
            .unwrap()
            .iter()
            .any(|known| known == thread_id)
    }

    pub fn supports_thread_factory(&self) -> bool {
        self.factory.is_some()
    }

    /// Returns the settled checkpoint for the configured thread.
    pub async fn thread_read(&self) -> Result<ThreadCheckpoint, AppServerError> {
        self.thread_read_for(self.thread_id.clone()).await
    }

    pub async fn thread_read_for(
        &self,
        thread_id: ThreadId,
    ) -> Result<ThreadCheckpoint, AppServerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::ReadThread { thread_id, reply })
            .await
            .map_err(|_| AppServerError::Disconnected)?;
        response.await.map_err(|_| AppServerError::Disconnected)?
    }

    /// Closes the configured thread after all active work has settled.
    pub async fn thread_close(&self) -> Result<(), AppServerError> {
        self.thread_close_for(self.thread_id.clone()).await
    }

    pub async fn thread_close_for(&self, thread_id: ThreadId) -> Result<(), AppServerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::CloseThread { thread_id, reply })
            .await
            .map_err(|_| AppServerError::Disconnected)?;
        response.await.map_err(|_| AppServerError::Disconnected)?
    }

    pub async fn thread_start(&self, thread_id: ThreadId) -> Result<ThreadId, AppServerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::CreateThread { thread_id, reply })
            .await
            .map_err(|_| AppServerError::Disconnected)?;
        response.await.map_err(|_| AppServerError::Disconnected)?
    }

    pub async fn thread_fork(
        &self,
        source_thread_id: ThreadId,
        new_thread_id: ThreadId,
    ) -> Result<ThreadId, AppServerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::ForkThread {
                source_thread_id,
                new_thread_id,
                reply,
            })
            .await
            .map_err(|_| AppServerError::Disconnected)?;
        response.await.map_err(|_| AppServerError::Disconnected)?
    }

    pub async fn thread_resume(
        &self,
        thread_id: ThreadId,
        checkpoint: ThreadCheckpoint,
    ) -> Result<ThreadId, AppServerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::ResumeThread {
                thread_id,
                checkpoint,
                reply,
            })
            .await
            .map_err(|_| AppServerError::Disconnected)?;
        response.await.map_err(|_| AppServerError::Disconnected)?
    }

    /// Returns a completed turn result retained by the service.
    pub async fn turn_read(&self, turn_id: TurnId) -> Result<SettledTurn, AppServerError> {
        let missing_id = turn_id.clone();
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::ReadTurn { turn_id, reply })
            .await
            .map_err(|_| AppServerError::Disconnected)?;
        response
            .await
            .map_err(|_| AppServerError::Disconnected)?
            .and_then(|result| result.ok_or(AppServerError::TurnNotFound(missing_id)))
    }

    /// Subscribes to the ordered event stream emitted by the core Thread.
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.events.subscribe()
    }

    /// Starts, steers, or queues a turn according to the typed input mode.
    pub async fn turn_start(&self, request: TurnStart) -> Result<TurnSubmission, AppServerError> {
        self.turn_start_for(self.thread_id.clone(), request).await
    }

    pub async fn turn_start_for(
        &self,
        thread_id: ThreadId,
        request: TurnStart,
    ) -> Result<TurnSubmission, AppServerError> {
        self.submit_start_for(thread_id, request, None).await
    }

    /// Steers the active turn when `turn_id` still identifies that turn.
    pub async fn turn_steer(
        &self,
        turn_id: TurnId,
        text: impl Into<String>,
    ) -> Result<TurnSubmission, AppServerError> {
        self.turn_steer_for(self.thread_id.clone(), turn_id, text)
            .await
    }

    pub async fn turn_steer_for(
        &self,
        thread_id: ThreadId,
        turn_id: TurnId,
        text: impl Into<String>,
    ) -> Result<TurnSubmission, AppServerError> {
        self.submit_start_for(
            thread_id,
            TurnStart::new(TurnInput::new(TurnInputMode::Steer, text)),
            Some(turn_id),
        )
        .await
    }

    async fn submit_start_for(
        &self,
        thread_id: ThreadId,
        request: TurnStart,
        expected_turn_id: Option<TurnId>,
    ) -> Result<TurnSubmission, AppServerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Start {
                thread_id,
                request,
                expected_turn_id,
                reply,
            })
            .await
            .map_err(|_| AppServerError::Disconnected)?;
        response.await.map_err(|_| AppServerError::Disconnected)?
    }

    /// Requests cooperative cancellation of the active turn.
    pub async fn turn_cancel(&self, request: TurnCancel) -> Result<(), AppServerError> {
        self.turn_cancel_for(self.thread_id.clone(), request).await
    }

    pub async fn turn_cancel_for(
        &self,
        thread_id: ThreadId,
        request: TurnCancel,
    ) -> Result<(), AppServerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Cancel {
                thread_id,
                request,
                reply,
            })
            .await
            .map_err(|_| AppServerError::Disconnected)?;
        response.await.map_err(|_| AppServerError::Disconnected)?
    }
}

enum Command {
    Start {
        thread_id: ThreadId,
        request: TurnStart,
        expected_turn_id: Option<TurnId>,
        reply: oneshot::Sender<Result<TurnSubmission, AppServerError>>,
    },
    Cancel {
        thread_id: ThreadId,
        request: TurnCancel,
        reply: oneshot::Sender<Result<(), AppServerError>>,
    },
    ReadThread {
        thread_id: ThreadId,
        reply: oneshot::Sender<Result<ThreadCheckpoint, AppServerError>>,
    },
    CloseThread {
        thread_id: ThreadId,
        reply: oneshot::Sender<Result<(), AppServerError>>,
    },
    ReadTurn {
        turn_id: TurnId,
        reply: oneshot::Sender<Result<Option<SettledTurn>, AppServerError>>,
    },
    CreateThread {
        thread_id: ThreadId,
        reply: oneshot::Sender<Result<ThreadId, AppServerError>>,
    },
    ForkThread {
        source_thread_id: ThreadId,
        new_thread_id: ThreadId,
        reply: oneshot::Sender<Result<ThreadId, AppServerError>>,
    },
    ResumeThread {
        thread_id: ThreadId,
        checkpoint: ThreadCheckpoint,
        reply: oneshot::Sender<Result<ThreadId, AppServerError>>,
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
    threads: Vec<Thread<M>>,
    mut commands: mpsc::Receiver<Command>,
    events: broadcast::Sender<EventEnvelope>,
    thread_ids: Arc<Mutex<Vec<ThreadId>>>,
    factory: Option<Arc<dyn ThreadFactory<M>>>,
) where
    M: Model + Send + 'static,
{
    let control = Arc::new(RunControl::new());
    let mut threads = threads
        .into_iter()
        .map(|thread| (thread.id().as_str().to_string(), thread))
        .collect::<HashMap<_, _>>();
    let mut settled_turns = HashMap::new();
    while let Some(command) = commands.recv().await {
        match command {
            Command::Start {
                thread_id,
                request,
                expected_turn_id,
                reply,
            } => {
                if expected_turn_id.is_some() {
                    let _ = reply.send(Err(AppServerError::NoActiveTurn));
                    continue;
                }
                let key = thread_id.as_str().to_string();
                let Some(mut thread) = threads.remove(&key) else {
                    let _ = reply.send(Err(AppServerError::ThreadNotFound(thread_id)));
                    continue;
                };
                if !matches!(
                    request.input.mode,
                    TurnInputMode::Start | TurnInputMode::StartIfIdle
                ) {
                    let _ = reply.send(Err(AppServerError::InvalidInputMode(request.input.mode)));
                    threads.insert(key, thread);
                    continue;
                }
                if thread.status() == mini_agent_core::ThreadStatus::Closed {
                    let _ = reply.send(Err(AppServerError::Closed));
                    threads.insert(key, thread);
                    continue;
                }
                if thread.status() == mini_agent_core::ThreadStatus::Running {
                    let _ = reply.send(Err(AppServerError::Busy));
                    threads.insert(key, thread);
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
                    let turn_result = loop {
                        tokio::select! {
                            result = &mut turn => break result,
                            Some(command) = commands.recv() => handle_running_command(command, &control, &thread_id, &turn_id),
                            else => {
                                drop(turn);
                                threads.insert(key.clone(), thread);
                                return;
                            },
                        }
                    };
                    match turn_result {
                        Ok(result) => {
                            settled_turns.insert(
                                result.id.as_str().to_string(),
                                SettledTurn {
                                    id: result.id,
                                    status: result.status,
                                    outcome: Some(result.outcome),
                                    error: None,
                                },
                            );
                        }
                        Err(error) => {
                            settled_turns.insert(
                                turn_id.as_str().to_string(),
                                SettledTurn {
                                    id: turn_id.clone(),
                                    status: mini_agent_core::TurnStatus::Failed,
                                    outcome: None,
                                    error: Some(error.to_string()),
                                },
                            );
                        }
                    }
                    next_input = control
                        .take_steer_input()
                        .or_else(|| control.take_follow_up_input());
                    if next_input.is_none() {
                        break;
                    }
                }
                threads.insert(key, thread);
            }
            Command::Cancel { reply, .. } => {
                let _ = reply.send(Err(AppServerError::NoActiveTurn));
            }
            Command::ReadThread { thread_id, reply } => {
                let result = threads
                    .get(thread_id.as_str())
                    .ok_or(AppServerError::ThreadNotFound(thread_id))
                    .and_then(|thread| {
                        thread
                            .checkpoint()
                            .map_err(|error| AppServerError::Checkpoint(error.to_string()))
                    });
                let _ = reply.send(result);
            }
            Command::CloseThread { thread_id, reply } => {
                let result = threads
                    .get_mut(thread_id.as_str())
                    .ok_or(AppServerError::ThreadNotFound(thread_id))
                    .and_then(|thread| {
                        thread
                            .close()
                            .map_err(|error| AppServerError::Checkpoint(error.to_string()))
                    });
                let _ = reply.send(result);
            }
            Command::ReadTurn { turn_id, reply } => {
                let _ = reply.send(Ok(settled_turns.get(turn_id.as_str()).cloned()));
            }
            Command::CreateThread { thread_id, reply } => {
                let result = create_thread(&mut threads, &thread_ids, factory.as_ref(), thread_id);
                let _ = reply.send(result);
            }
            Command::ForkThread {
                source_thread_id,
                new_thread_id,
                reply,
            } => {
                let result = fork_thread(
                    &mut threads,
                    &thread_ids,
                    factory.as_ref(),
                    source_thread_id,
                    new_thread_id,
                );
                let _ = reply.send(result);
            }
            Command::ResumeThread {
                thread_id,
                checkpoint,
                reply,
            } => {
                let result = resume_thread(
                    &mut threads,
                    &thread_ids,
                    factory.as_ref(),
                    thread_id,
                    checkpoint,
                );
                let _ = reply.send(result);
            }
        }
    }
}

fn create_thread<M>(
    threads: &mut HashMap<String, Thread<M>>,
    thread_ids: &Arc<Mutex<Vec<ThreadId>>>,
    factory: Option<&Arc<dyn ThreadFactory<M>>>,
    thread_id: ThreadId,
) -> Result<ThreadId, AppServerError>
where
    M: Model + 'static,
{
    let key = thread_id.as_str().to_string();
    if threads.contains_key(&key) {
        return Err(AppServerError::ThreadAlreadyExists(thread_id));
    }
    let factory = factory.ok_or(AppServerError::ThreadFactoryUnavailable)?;
    let mut thread = factory.create(thread_id.clone())?;
    thread.set_id(thread_id.clone());
    threads.insert(key, thread);
    thread_ids.lock().unwrap().push(thread_id.clone());
    Ok(thread_id)
}

fn fork_thread<M>(
    threads: &mut HashMap<String, Thread<M>>,
    thread_ids: &Arc<Mutex<Vec<ThreadId>>>,
    factory: Option<&Arc<dyn ThreadFactory<M>>>,
    source_thread_id: ThreadId,
    new_thread_id: ThreadId,
) -> Result<ThreadId, AppServerError>
where
    M: Model + 'static,
{
    let new_key = new_thread_id.as_str().to_string();
    if threads.contains_key(&new_key) {
        return Err(AppServerError::ThreadAlreadyExists(new_thread_id));
    }
    let checkpoint = threads
        .get(source_thread_id.as_str())
        .ok_or_else(|| AppServerError::ThreadNotFound(source_thread_id.clone()))?
        .checkpoint()
        .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
    let factory = factory.ok_or(AppServerError::ThreadFactoryUnavailable)?;
    let mut fork = factory.create(new_thread_id.clone())?;
    let mut checkpoint = checkpoint;
    checkpoint.thread_id = new_thread_id.clone();
    fork.restore_checkpoint(checkpoint)
        .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
    threads.insert(new_key, fork);
    thread_ids.lock().unwrap().push(new_thread_id.clone());
    Ok(new_thread_id)
}

fn resume_thread<M>(
    threads: &mut HashMap<String, Thread<M>>,
    thread_ids: &Arc<Mutex<Vec<ThreadId>>>,
    factory: Option<&Arc<dyn ThreadFactory<M>>>,
    thread_id: ThreadId,
    mut checkpoint: ThreadCheckpoint,
) -> Result<ThreadId, AppServerError>
where
    M: Model + 'static,
{
    checkpoint.thread_id = thread_id.clone();
    let key = thread_id.as_str().to_string();
    if let Some(thread) = threads.get_mut(&key) {
        thread
            .restore_checkpoint(checkpoint)
            .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
        return Ok(thread_id);
    }
    let factory = factory.ok_or(AppServerError::ThreadFactoryUnavailable)?;
    let mut thread = factory.create(thread_id.clone())?;
    thread
        .restore_checkpoint(checkpoint)
        .map_err(|error| AppServerError::Checkpoint(error.to_string()))?;
    threads.insert(key, thread);
    thread_ids.lock().unwrap().push(thread_id.clone());
    Ok(thread_id)
}

fn handle_running_command(
    command: Command,
    control: &RunControl,
    active_thread_id: &ThreadId,
    turn_id: &TurnId,
) {
    match command {
        Command::Start {
            thread_id,
            request,
            expected_turn_id,
            reply,
        } => {
            if thread_id != *active_thread_id {
                let _ = reply.send(Err(AppServerError::Busy));
                return;
            }
            if let Some(expected_turn_id) = expected_turn_id
                && expected_turn_id != *turn_id
            {
                let _ = reply.send(Err(AppServerError::TurnNotActive(expected_turn_id)));
                return;
            }
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
        Command::Cancel {
            thread_id,
            request,
            reply,
        } => {
            if thread_id != *active_thread_id {
                let _ = reply.send(Err(AppServerError::Busy));
                return;
            }
            let result = if request.turn_id == *turn_id {
                control.request_cancel();
                Ok(())
            } else {
                Err(AppServerError::TurnNotActive(request.turn_id))
            };
            let _ = reply.send(result);
        }
        Command::ReadThread { reply, .. } => {
            let _ = reply.send(Err(AppServerError::Busy));
        }
        Command::CloseThread { reply, .. } => {
            let _ = reply.send(Err(AppServerError::Busy));
        }
        Command::ReadTurn { reply, .. } => {
            let _ = reply.send(Err(AppServerError::Busy));
        }
        Command::CreateThread { reply, .. } => {
            let _ = reply.send(Err(AppServerError::Busy));
        }
        Command::ForkThread { reply, .. } => {
            let _ = reply.send(Err(AppServerError::Busy));
        }
        Command::ResumeThread { reply, .. } => {
            let _ = reply.send(Err(AppServerError::Busy));
        }
    }
}
