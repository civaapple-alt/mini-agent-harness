use mini_agent_core::RunControl;
use mini_agent_core::Thread;
use mini_agent_core::ThreadCheckpoint;
use mini_agent_protocol::EventEnvelope;
use mini_agent_protocol::Model;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStart;
use mini_agent_protocol::ToolApprovalRequest;
use mini_agent_protocol::TurnCancel;
use mini_agent_protocol::TurnId;
use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnInputMode;
use mini_agent_protocol::TurnStart;
use mini_agent_protocol::TurnSubmission;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::thread;
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
    resolved: std::collections::VecDeque<ApprovalResolution>,
    responders: HashMap<String, (ApprovalRequest, std::sync::mpsc::Sender<bool>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub action: String,
    pub call_id: Option<String>,
    pub thread_id: Option<ThreadId>,
    pub turn_id: Option<TurnId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalResolution {
    pub request_id: String,
    pub action: String,
    pub call_id: Option<String>,
    pub thread_id: Option<ThreadId>,
    pub turn_id: Option<TurnId>,
    pub approved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalEvent {
    Requested(ApprovalRequest),
    Resolved(ApprovalResolution),
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
                resolved: std::collections::VecDeque::new(),
                responders: HashMap::new(),
            })),
            notify: Arc::new(Notify::new()),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Called by a synchronous host approval callback. The callback waits
    /// until the external client answers the corresponding request.
    pub fn request(&self, action: &str) -> Result<bool, String> {
        self.request_with_context(&ToolApprovalRequest::legacy(action))
    }

    /// Called by a synchronous Host approval callback with tool identity.
    ///
    /// The broker assigns `request_id`; the caller-provided Thread, Turn, and
    /// call IDs remain attached to both request and resolution events.
    pub fn request_with_context(&self, approval: &ToolApprovalRequest) -> Result<bool, String> {
        let request_id = format!("approval-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (sender, receiver) = std::sync::mpsc::channel();
        let request = ApprovalRequest {
            request_id: request_id.clone(),
            action: approval.action.clone(),
            call_id: approval.call_id.clone(),
            thread_id: approval.thread_id.clone(),
            turn_id: approval.turn_id.clone(),
        };
        {
            let mut state = self.state.lock().unwrap();
            state
                .responders
                .insert(request_id, (request.clone(), sender));
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

    pub async fn next_event(&self) -> ApprovalEvent {
        loop {
            let event = {
                let mut state = self.state.lock().unwrap();
                state
                    .queued
                    .pop_front()
                    .map(ApprovalEvent::Requested)
                    .or_else(|| state.resolved.pop_front().map(ApprovalEvent::Resolved))
            };
            if let Some(event) = event {
                return event;
            }
            self.notify.notified().await;
        }
    }

    pub fn respond(&self, request_id: &str, approved: bool) -> Result<(), String> {
        let (request, sender) = self
            .state
            .lock()
            .unwrap()
            .responders
            .remove(request_id)
            .ok_or_else(|| format!("unknown approval request: {request_id}"))?;
        sender
            .send(approved)
            .map_err(|_| "approval callback is no longer waiting".to_string())?;
        self.state
            .lock()
            .unwrap()
            .resolved
            .push_back(ApprovalResolution {
                request_id: request_id.to_string(),
                action: request.action,
                call_id: request.call_id,
                thread_id: request.thread_id,
                turn_id: request.turn_id,
                approved,
            });
        self.notify.notify_one();
        Ok(())
    }
}

impl Default for ApprovalBroker {
    fn default() -> Self {
        Self::new()
    }
}

mod action;
pub mod client;
pub mod frontend;
mod goal_runtime;
mod goal_service;
pub mod json_rpc;
pub mod local;
pub mod management;
mod notification;
pub mod runtime;
mod runtime_actor;
mod runtime_command;
mod runtime_state;
mod thread_settings;
pub mod trace;
pub mod verifier;

pub use client::LocalAppServerClient;
pub use goal_service::GoalService;
pub use json_rpc::AppServerConnection;
pub use json_rpc::RuntimeServices;
pub use json_rpc::StartupServices;
pub use json_rpc::serve_stdio_with_approval_and_manifest;
pub use json_rpc::serve_stdio_with_startup_and_services;
pub use management::RuntimeManagementService;
pub use mini_agent_app_server_protocol::{
    McpRetryResult as ProtocolMcpRetryResult, McpStatusResult, SessionInfoResult,
    WorldRefreshResult, WorldSetExecutionResult, WorldStateResult,
};
pub(crate) use notification::RuntimeNotification;
pub use runtime::capability_manifest_to_protocol;
pub use runtime::{
    AppServerRuntime, McpRetryResult, RuntimeSessionInfo, RuntimeStartOptions, RuntimeTurnBatch,
    RuntimeTurnResult, SessionRequest,
};
pub use thread_settings::ThreadSettingsService;
pub use trace::{JsonlTrace, TraceRecord};

mod worker;
use action::ActionFailure;
use action::ActionResponse;
use action::ActionResult;
use action::RuntimeRevision;
use management::RuntimeActorState;
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
    RevisionConflict { expected: u64, actual: u64 },
    ThreadNotFound(ThreadId),
    ThreadAlreadyExists(ThreadId),
    ThreadFactoryUnavailable,
    RuntimeUnavailable,
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
    ExtendTools(Vec<Box<dyn mini_agent_protocol::Tool>>),
}

/// A settled turn record retained by the service for inspection.
#[derive(Clone, Debug, PartialEq)]
pub struct SettledTurn {
    pub id: TurnId,
    pub status: mini_agent_protocol::TurnStatus,
    pub outcome: Option<mini_agent_core::RunOutcome>,
    pub error: Option<String>,
}

#[cfg(test)]
#[path = "tests.rs"]
pub(crate) mod tests;

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
            Self::RevisionConflict { expected, actual } => write!(
                formatter,
                "runtime revision conflict: expected {expected}, actual {actual}"
            ),
            Self::ThreadNotFound(thread_id) => {
                write!(formatter, "thread {} is not available", thread_id.as_str())
            }
            Self::ThreadAlreadyExists(thread_id) => {
                write!(formatter, "thread {} already exists", thread_id.as_str())
            }
            Self::ThreadFactoryUnavailable => formatter.write_str("thread factory is unavailable"),
            Self::RuntimeUnavailable => formatter.write_str("runtime state is unavailable"),
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
    notifications: broadcast::Sender<RuntimeNotification>,
    control: Arc<RunControl>,
    thread_id: ThreadId,
    thread_ids: Arc<Mutex<Vec<ThreadId>>>,
    runtime_revision: Arc<AtomicU64>,
    factory: Option<Arc<dyn ThreadFactory<M>>>,
    _model: std::marker::PhantomData<fn() -> M>,
}

impl<M> Clone for AppServer<M> {
    fn clone(&self) -> Self {
        Self {
            commands: self.commands.clone(),
            events: self.events.clone(),
            notifications: self.notifications.clone(),
            control: self.control.clone(),
            thread_id: self.thread_id.clone(),
            thread_ids: self.thread_ids.clone(),
            runtime_revision: self.runtime_revision.clone(),
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
        let runtime_revision = Arc::new(AtomicU64::new(0));
        let (commands, command_receiver) = mpsc::channel(COMMAND_BUFFER);
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let (notifications, _) = broadcast::channel(EVENT_BUFFER);
        let worker_events = events.clone();
        let worker_notifications = notifications.clone();
        let worker_thread_ids = thread_ids.clone();
        let worker_revision = runtime_revision.clone();
        let worker_factory = factory.clone();
        let worker_control = control.clone();
        thread::Builder::new()
            .name("mini-agent-app-server".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("app-server worker runtime must be available");
                runtime.block_on(worker_loop(
                    threads,
                    command_receiver,
                    worker_events,
                    worker_notifications,
                    worker_thread_ids,
                    worker_revision,
                    worker_factory,
                    worker_control,
                ));
            })
            .expect("app-server worker thread must start");
        Self {
            commands,
            events,
            notifications,
            control,
            thread_id: start.thread_id,
            thread_ids,
            runtime_revision,
            factory,
            _model: std::marker::PhantomData,
        }
    }

    pub(crate) fn command_sender(&self) -> mpsc::Sender<Command> {
        self.commands.clone()
    }

    pub(crate) fn notifications(&self) -> broadcast::Sender<RuntimeNotification> {
        self.notifications.clone()
    }

    pub(crate) fn runtime_revision(&self) -> RuntimeRevision {
        self.runtime_revision.load(Ordering::SeqCst).into()
    }

    pub(crate) fn runtime_revision_handle(&self) -> Arc<AtomicU64> {
        self.runtime_revision.clone()
    }

    pub(crate) fn install_runtime_state(
        &self,
        state: RuntimeActorState,
    ) -> Result<(), AppServerError> {
        let revision = state.revision().value();
        self.commands
            .try_send(Command::InstallRuntime {
                state: Box::new(state),
            })
            .map_err(|_| AppServerError::Disconnected)?;
        self.runtime_revision.store(revision, Ordering::SeqCst);
        Ok(())
    }

    async fn request_action<T, F>(&self, build: F) -> Result<ActionResponse<T>, ActionFailure>
    where
        F: FnOnce(oneshot::Sender<ActionResult<T>>) -> Command,
    {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(build(reply))
            .await
            .map_err(|_| ActionFailure::without_receipt(AppServerError::Disconnected))?;
        response
            .await
            .map_err(|_| ActionFailure::without_receipt(AppServerError::Disconnected))?
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

    /// Returns the settled checkpoint for a thread.
    pub async fn thread_read_for(
        &self,
        thread_id: ThreadId,
    ) -> Result<ThreadCheckpoint, AppServerError> {
        self.thread_read_action(thread_id)
            .await
            .map(ActionResponse::into_value)
            .map_err(ActionFailure::into_error)
    }

    pub(crate) async fn thread_read_action(
        &self,
        thread_id: ThreadId,
    ) -> Result<ActionResponse<ThreadCheckpoint>, ActionFailure> {
        self.request_action(|reply| Command::ReadThread { thread_id, reply })
            .await
    }

    /// Applies a host-side update after all earlier commands for a thread.
    pub async fn thread_update_for(
        &self,
        thread_id: ThreadId,
        update: ThreadUpdate,
    ) -> Result<(), AppServerError> {
        self.thread_update_action(thread_id, update)
            .await
            .map(ActionResponse::into_value)
            .map_err(ActionFailure::into_error)
    }

    pub(crate) async fn thread_update_action(
        &self,
        thread_id: ThreadId,
        update: ThreadUpdate,
    ) -> Result<ActionResponse<()>, ActionFailure> {
        self.request_action(|reply| Command::UpdateThread {
            thread_id,
            update,
            reply,
        })
        .await
    }

    /// Reassigns a settled thread identity while keeping its service worker.
    pub async fn thread_reset(
        &self,
        thread_id: ThreadId,
        new_thread_id: ThreadId,
        next_turn_number: u64,
    ) -> Result<ThreadId, AppServerError> {
        self.thread_reset_action(thread_id, new_thread_id, next_turn_number)
            .await
            .map(ActionResponse::into_value)
            .map_err(ActionFailure::into_error)
    }

    pub(crate) async fn thread_reset_action(
        &self,
        thread_id: ThreadId,
        new_thread_id: ThreadId,
        next_turn_number: u64,
    ) -> Result<ActionResponse<ThreadId>, ActionFailure> {
        self.request_action(|reply| Command::ResetThread {
            thread_id,
            new_thread_id,
            next_turn_number,
            reply,
        })
        .await
    }

    /// Closes a thread after all active work has settled.
    pub async fn thread_close_for(&self, thread_id: ThreadId) -> Result<(), AppServerError> {
        self.thread_close_action(thread_id)
            .await
            .map(ActionResponse::into_value)
            .map_err(ActionFailure::into_error)
    }

    pub(crate) async fn thread_close_action(
        &self,
        thread_id: ThreadId,
    ) -> Result<ActionResponse<()>, ActionFailure> {
        self.request_action(|reply| Command::CloseThread { thread_id, reply })
            .await
    }

    pub async fn thread_start(&self, thread_id: ThreadId) -> Result<ThreadId, AppServerError> {
        self.thread_start_action(thread_id)
            .await
            .map(ActionResponse::into_value)
            .map_err(ActionFailure::into_error)
    }

    pub(crate) async fn thread_start_action(
        &self,
        thread_id: ThreadId,
    ) -> Result<ActionResponse<ThreadId>, ActionFailure> {
        self.request_action(|reply| Command::CreateThread { thread_id, reply })
            .await
    }

    pub async fn thread_fork(
        &self,
        source_thread_id: ThreadId,
        new_thread_id: ThreadId,
    ) -> Result<ThreadId, AppServerError> {
        self.thread_fork_action(source_thread_id, new_thread_id)
            .await
            .map(ActionResponse::into_value)
            .map_err(ActionFailure::into_error)
    }

    pub(crate) async fn thread_fork_action(
        &self,
        source_thread_id: ThreadId,
        new_thread_id: ThreadId,
    ) -> Result<ActionResponse<ThreadId>, ActionFailure> {
        self.request_action(|reply| Command::ForkThread {
            source_thread_id,
            new_thread_id,
            reply,
        })
        .await
    }

    pub async fn thread_resume(
        &self,
        thread_id: ThreadId,
        checkpoint: ThreadCheckpoint,
    ) -> Result<ThreadId, AppServerError> {
        self.thread_resume_action(thread_id, checkpoint)
            .await
            .map(ActionResponse::into_value)
            .map_err(ActionFailure::into_error)
    }

    pub(crate) async fn thread_resume_action(
        &self,
        thread_id: ThreadId,
        checkpoint: ThreadCheckpoint,
    ) -> Result<ActionResponse<ThreadId>, ActionFailure> {
        self.request_action(|reply| Command::ResumeThread {
            thread_id,
            checkpoint,
            reply,
        })
        .await
    }

    /// Returns a completed turn result retained by the service.
    pub async fn turn_read(&self, turn_id: TurnId) -> Result<SettledTurn, AppServerError> {
        let missing_id = turn_id.clone();
        self.turn_read_action(turn_id)
            .await
            .map(ActionResponse::into_value)
            .map_err(ActionFailure::into_error)
            .and_then(|result| result.ok_or(AppServerError::TurnNotFound(missing_id)))
    }

    pub(crate) async fn turn_read_action(
        &self,
        turn_id: TurnId,
    ) -> Result<ActionResponse<Option<SettledTurn>>, ActionFailure> {
        self.request_action(|reply| Command::ReadTurn { turn_id, reply })
            .await
    }

    /// Subscribes to the ordered event stream emitted by the core Thread.
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.events.subscribe()
    }

    /// Starts, steers, or queues a turn according to the typed input mode.
    pub async fn turn_start_for(
        &self,
        thread_id: ThreadId,
        request: TurnStart,
    ) -> Result<TurnSubmission, AppServerError> {
        self.submit_start_action(thread_id, request, None)
            .await
            .map(ActionResponse::into_value)
            .map_err(ActionFailure::into_error)
    }

    /// Steers the active turn when `turn_id` still identifies that turn.
    pub async fn turn_steer_for(
        &self,
        thread_id: ThreadId,
        turn_id: TurnId,
        text: impl Into<String>,
    ) -> Result<TurnSubmission, AppServerError> {
        self.submit_start_action(
            thread_id,
            TurnStart::new(TurnInput::new(TurnInputMode::Steer, text)),
            Some(turn_id),
        )
        .await
        .map(ActionResponse::into_value)
        .map_err(ActionFailure::into_error)
    }

    pub(crate) async fn submit_start_action(
        &self,
        thread_id: ThreadId,
        request: TurnStart,
        expected_turn_id: Option<TurnId>,
    ) -> Result<ActionResponse<TurnSubmission>, ActionFailure> {
        self.request_action(|reply| Command::Start {
            thread_id,
            request,
            expected_turn_id,
            origin: crate::worker::TurnOrigin::Client,
            reply,
        })
        .await
    }

    /// Requests cooperative cancellation of the active turn.
    pub async fn turn_cancel_for(
        &self,
        thread_id: ThreadId,
        request: TurnCancel,
    ) -> Result<(), AppServerError> {
        self.turn_cancel_action(thread_id, request)
            .await
            .map(ActionResponse::into_value)
            .map_err(ActionFailure::into_error)
    }

    pub(crate) async fn turn_cancel_action(
        &self,
        thread_id: ThreadId,
        request: TurnCancel,
    ) -> Result<ActionResponse<()>, ActionFailure> {
        self.request_action(|reply| Command::Cancel {
            thread_id,
            request,
            reply,
        })
        .await
    }
}
