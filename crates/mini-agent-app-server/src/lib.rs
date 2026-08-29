use mini_agent_core::EventEnvelope;
use mini_agent_core::Model;
use mini_agent_core::RunControl;
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
pub mod demo;
pub mod json_rpc;
pub mod local;
pub mod mentor;
pub mod runtime;
pub mod workflows;

pub use client::LocalAppServerClient;
pub use json_rpc::AppServerConnection;
pub use json_rpc::serve_stdio;
pub use json_rpc::serve_stdio_with_approval;
pub use json_rpc::serve_stdio_with_approval_and_manifest;
pub use json_rpc::serve_stdio_with_startup;
pub use runtime::capability_manifest_to_protocol;
pub use runtime::{
    AppServerRuntime, McpRetryResult, RuntimeSessionInfo, RuntimeTurnBatch, RuntimeTurnResult,
};
pub use workflows::WorkflowService;

mod worker;
use worker::{Command, worker_loop};

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

/// A host-side update applied to a settled Thread by the App Server worker.
///
/// These operations are transport-neutral. They let local frontends update
/// the same Thread that owns turn execution without retaining a second mutable
/// Harness in the frontend.
pub enum ThreadUpdate {
    ClearHistory,
    AppendContext(String),
    ReplaceConfig(mini_agent_core::HarnessConfig),
    ExtendTools(Vec<Box<dyn mini_agent_core::Tool>>),
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
    control: Arc<RunControl>,
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
            control: self.control.clone(),
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
    pub fn new(start: ThreadStart, thread: Thread<M>) -> Self {
        Self::new_with_control(start, thread, Arc::new(RunControl::new()))
    }

    /// Starts an in-process worker using caller-owned cooperative control.
    ///
    /// Local frontends can share this control with their input loop while the
    /// App Server remains the owner of turn execution and queue draining.
    pub fn new_with_control(
        start: ThreadStart,
        mut thread: Thread<M>,
        control: Arc<RunControl>,
    ) -> Self {
        thread.set_id(start.thread_id.clone());
        Self::with_threads_and_control(start, vec![thread], None, control)
    }

    /// Starts a service over several preconfigured Threads.
    ///
    /// The first supplied thread is assigned the default `start.thread_id`;
    /// additional threads retain their identities. Turns are serialized by
    /// the service worker, while lifecycle and checkpoint operations remain
    /// addressed by thread identity.
    pub fn with_threads(start: ThreadStart, threads: Vec<Thread<M>>) -> Self {
        Self::with_threads_and_control(start, threads, None, Arc::new(RunControl::new()))
    }

    fn with_threads_and_control(
        start: ThreadStart,
        mut threads: Vec<Thread<M>>,
        factory: Option<Arc<dyn ThreadFactory<M>>>,
        control: Arc<RunControl>,
    ) -> Self {
        assert!(
            !threads.is_empty(),
            "app-server requires at least one thread"
        );
        threads[0].set_id(start.thread_id.clone());
        Self::with_threads_and_factory(start, threads, factory, control)
    }

    pub fn with_thread_factory<F>(start: ThreadStart, threads: Vec<Thread<M>>, factory: F) -> Self
    where
        F: ThreadFactory<M>,
    {
        Self::with_threads_and_factory(
            start,
            threads,
            Some(Arc::new(factory)),
            Arc::new(RunControl::new()),
        )
    }

    fn with_threads_and_factory(
        start: ThreadStart,
        mut threads: Vec<Thread<M>>,
        factory: Option<Arc<dyn ThreadFactory<M>>>,
        control: Arc<RunControl>,
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
            control.clone(),
        ));
        Self {
            commands,
            events,
            control,
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

    /// Applies a host-side update after all earlier commands for this thread.
    pub async fn thread_update(&self, update: ThreadUpdate) -> Result<(), AppServerError> {
        self.thread_update_for(self.thread_id.clone(), update).await
    }

    pub async fn thread_update_for(
        &self,
        thread_id: ThreadId,
        update: ThreadUpdate,
    ) -> Result<(), AppServerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::UpdateThread {
                thread_id,
                update,
                reply,
            })
            .await
            .map_err(|_| AppServerError::Disconnected)?;
        response.await.map_err(|_| AppServerError::Disconnected)?
    }

    /// Reassigns a settled thread identity while keeping its service worker.
    pub async fn thread_reset(
        &self,
        thread_id: ThreadId,
        new_thread_id: ThreadId,
        next_turn_number: u64,
    ) -> Result<ThreadId, AppServerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::ResetThread {
                thread_id,
                new_thread_id,
                next_turn_number,
                reply,
            })
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
