use mini_agent_app_server_protocol::{
    AccessScope, ActionGrantKey, ActionGrantScope, ApprovalDecision, ApprovalOutcome,
    ApprovalPolicy, ApprovalRespondParams,
};
use mini_agent_capabilities::action_grant_key;
use mini_agent_core::{RunControl, Thread, ThreadCheckpoint};
use mini_agent_protocol::{
    EventEnvelope, Model, ThreadId, ThreadStart, ToolApprovalRequest, TurnCancel, TurnId,
    TurnInput, TurnInputMode, TurnStart, TurnSubmission,
};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use tokio::sync::{Notify, broadcast, mpsc, oneshot};

const EVENT_BUFFER: usize = 256;
const EVENT_REPLAY_BUFFER: usize = 512;
const COMMAND_BUFFER: usize = 32;
static NEXT_BROKER_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct ApprovalBroker {
    state: Arc<Mutex<ApprovalState>>,
    notify: Arc<Notify>,
    broker_id: u64,
    next_id: Arc<AtomicU64>,
    execution: Arc<RwLock<ApprovalExecution>>,
    trace: approval_trace::ApprovalTrace,
}

#[derive(Clone, Copy)]
struct ApprovalExecution {
    access: AccessScope,
    policy: ApprovalPolicy,
}

struct ApprovalState {
    queued: std::collections::VecDeque<ApprovalRequest>,
    resolved: std::collections::VecDeque<ApprovalResolution>,
    responders: HashMap<String, (ApprovalRequest, std::sync::mpsc::Sender<ApprovalResolution>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub action: String,
    pub action_summary: String,
    pub project_id: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_revision: Option<u64>,
    pub session_id: Option<String>,
    pub call_id: Option<String>,
    pub thread_id: Option<ThreadId>,
    pub turn_id: Option<TurnId>,
    pub tool_name: Option<String>,
    pub action_class: String,
    pub action_key: Option<ActionGrantKey>,
    pub access: AccessScope,
    pub policy: ApprovalPolicy,
    pub allowed_grant_scopes: Vec<ActionGrantScope>,
    pub target_paths: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalResolution {
    pub request_id: String,
    pub action: String,
    pub action_summary: String,
    pub project_id: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_revision: Option<u64>,
    pub session_id: Option<String>,
    pub call_id: Option<String>,
    pub thread_id: Option<ThreadId>,
    pub turn_id: Option<TurnId>,
    pub tool_name: Option<String>,
    pub action_class: String,
    pub grant_scope: Option<ActionGrantScope>,
    pub outcome: ApprovalOutcome,
    pub reason: Option<String>,
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
            broker_id: NEXT_BROKER_ID.fetch_add(1, Ordering::Relaxed),
            next_id: Arc::new(AtomicU64::new(1)),
            execution: Arc::new(RwLock::new(ApprovalExecution {
                access: AccessScope::Project,
                policy: ApprovalPolicy::Interactive,
            })),
            trace: Default::default(),
        }
    }

    /// Binds a Thread to its independent approval evidence sidecar.
    ///
    /// Trace failures are reported to the caller and never affect approval
    /// or tool execution. The caller may choose to keep the runtime alive
    /// without evidence when the sidecar cannot be opened.
    pub fn bind_thread_trace(&self, thread_id: String, session_file: &std::path::Path) {
        self.trace.bind_thread_file(thread_id, session_file);
    }

    pub fn set_execution_scope(&self, access: AccessScope, policy: ApprovalPolicy) {
        *self.execution.write().unwrap() = ApprovalExecution { access, policy };
    }

    pub fn execution_scope(&self) -> (AccessScope, ApprovalPolicy) {
        let execution = *self.execution.read().unwrap();
        (execution.access, execution.policy)
    }

    /// Called by a synchronous Host approval callback with tool identity.
    ///
    /// The broker assigns a process/broker/call-scoped `request_id`; the
    /// caller-provided Thread, Turn, and call IDs remain attached to both
    /// request and resolution events. Including the broker and call identity
    /// prevents two runtime instances from reusing a local `approval-1` while
    /// a Gateway is still waiting on both requests.
    pub fn request_resolution(
        &self,
        approval: &ToolApprovalRequest,
    ) -> Result<ApprovalResolution, String> {
        let sequence = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request_id = match approval.call_id.as_deref() {
            Some(call_id) if !call_id.is_empty() => format!(
                "approval-{}-{}-{}-{}",
                std::process::id(),
                self.broker_id,
                sequence,
                call_id
            ),
            _ => format!(
                "approval-{}-{}-{}",
                std::process::id(),
                self.broker_id,
                sequence
            ),
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let execution = *self.execution.read().unwrap();
        let access_scope = match execution.access {
            AccessScope::Project => "project",
            AccessScope::FullMachine => "full_machine",
        };
        let action_key = action_grant_key(approval, access_scope);
        let request = ApprovalRequest {
            request_id: request_id.clone(),
            action: approval.action.clone(),
            action_summary: approval
                .action_summary
                .clone()
                .unwrap_or_else(|| approval.action.clone()),
            project_id: approval.project_id.clone(),
            workspace_id: approval.workspace_id.clone(),
            workspace_revision: approval.workspace_revision,
            session_id: approval.session_id.clone(),
            call_id: approval.call_id.clone(),
            thread_id: approval.thread_id.clone(),
            turn_id: approval.turn_id.clone(),
            tool_name: approval.tool_name.clone(),
            action_class: approval
                .tool_name
                .clone()
                .unwrap_or_else(|| "tool_action".to_string()),
            action_key: action_key.clone(),
            access: execution.access,
            policy: execution.policy,
            allowed_grant_scopes: action_key
                .is_some()
                .then_some(vec![
                    ActionGrantScope::Once,
                    ActionGrantScope::Session,
                    ActionGrantScope::Project,
                ])
                .unwrap_or_else(|| vec![ActionGrantScope::Once]),
            target_paths: approval.target_paths.clone(),
        };
        {
            let mut state = self.state.lock().unwrap();
            state
                .responders
                .insert(request_id, (request.clone(), sender));
            state.queued.push_back(request.clone());
        }
        self.trace.requested(&request);
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

    pub fn respond(&self, response: ApprovalRespondParams) -> Result<(), String> {
        let request_id = response.request_id.as_str();
        let (request, sender) = {
            let mut state = self.state.lock().unwrap();
            let request = state
                .responders
                .get(request_id)
                .map(|(request, _)| request.clone())
                .ok_or_else(|| format!("unknown approval request: {request_id}"))?;
            let grant_scope = response.grant_scope;
            if response.decision == ApprovalDecision::Approve
                && !grant_scope.is_some_and(|scope| request.allowed_grant_scopes.contains(&scope))
            {
                return Err("approval response exceeds the requested scope".to_string());
            }
            if response.decision == ApprovalDecision::Deny && grant_scope.is_some() {
                return Err("denied approval cannot grant a scope".to_string());
            }
            state
                .responders
                .remove(request_id)
                .expect("approval responder was checked above")
        };
        let outcome = match response.decision {
            ApprovalDecision::Approve => ApprovalOutcome::Approved,
            ApprovalDecision::Deny => ApprovalOutcome::Denied,
        };
        let trace_request = request.clone();
        let resolution = ApprovalResolution {
            request_id: request.request_id.clone(),
            action: request.action.clone(),
            action_summary: request.action_summary.clone(),
            project_id: request.project_id.clone(),
            workspace_id: request.workspace_id,
            workspace_revision: request.workspace_revision,
            session_id: request.session_id,
            call_id: request.call_id,
            thread_id: request.thread_id,
            turn_id: request.turn_id,
            tool_name: request.tool_name,
            action_class: request.action_class,
            grant_scope: response.grant_scope,
            outcome,
            reason: response.reason,
        };
        sender
            .send(resolution.clone())
            .map_err(|_| "approval callback is no longer waiting".to_string())?;
        self.trace.resolved(&trace_request, &resolution);
        self.state.lock().unwrap().resolved.push_back(resolution);
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
mod approval_trace;
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
mod thread_manager;
mod thread_settings;
pub mod trace;
pub mod verifier;

pub use client::LocalAppServerClient;
pub use goal_service::ThreadGoalRequestProcessor;
pub use json_rpc::AppServerConnection;
pub use json_rpc::RuntimeServices;
pub use json_rpc::StartupServices;
pub use json_rpc::serve_stdio_with_approval_and_manifest;
pub use json_rpc::serve_stdio_with_startup_and_services;
pub use management::RuntimeManagementService;
pub use mini_agent_app_server_protocol::{
    ForkCompactionMethod, ForkContextPolicy, McpRetryResult as ProtocolMcpRetryResult,
    McpStatusResult, SessionForkResult, SessionInfoResult, WorldRefreshResult,
    WorldSetExecutionResult, WorldStateResult,
};
pub(crate) use notification::RuntimeNotification;
pub use runtime::capability_manifest_to_protocol;
pub use runtime::{
    AppServerRuntime, McpRetryResult, RuntimeSessionInfo, RuntimeStartOptions, RuntimeTurnBatch,
    RuntimeTurnResult, SessionRequest,
};
pub use thread_settings::ThreadSettingsService;
pub use trace::{JsonlTrace, TraceRecord};

mod status;
mod worker;
use action::ActionFailure;
use action::ActionResponse;
use action::ActionResult;
use action::ActionSequencer;
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
    InvalidItemCursor(String),
    Checkpoint(String),
    SessionForkConflict(mini_agent_capabilities::SessionForkConflict),
    RevisionConflict { expected: u64, actual: u64 },
    ThreadNotFound(ThreadId),
    ThreadAlreadyExists(ThreadId),
    ThreadFactoryUnavailable,
    RuntimeUnavailable,
    GoalOwnsContinuationMode,
    SkillActivation(String),
}

/// A host-side update applied to a settled Thread by the App Server worker.
///
/// These operations are transport-neutral. They let local frontends update
/// the same Thread that owns turn execution without retaining a second mutable
/// Harness in the frontend.
pub enum ThreadUpdate {
    ClearHistory,
    AppendContext(String),
    ReplaceContext { slot: String, text: String },
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EventReplaySnapshot {
    pub(crate) events: Vec<EventEnvelope>,
    pub(crate) oldest_sequence: Option<u64>,
    pub(crate) has_gap: bool,
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
            Self::InvalidItemCursor(cursor) => {
                write!(formatter, "invalid ThreadItem cursor: {cursor}")
            }
            Self::Checkpoint(error) => write!(formatter, "checkpoint unavailable: {error}"),
            Self::SessionForkConflict(conflict) => conflict.fmt(formatter),
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
            Self::GoalOwnsContinuationMode => formatter.write_str(
                "active Goal Runtime owns continuation mode; pause or finish the Goal before changing it",
            ),
            Self::SkillActivation(error) => write!(formatter, "skill activation failed: {error}"),
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
    event_replay: Arc<Mutex<VecDeque<EventEnvelope>>>,
    runtime_status: Arc<Mutex<mini_agent_app_server_protocol::RuntimeStatus>>,
    control: Arc<RunControl>,
    action_sequencer: ActionSequencer,
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
            event_replay: self.event_replay.clone(),
            runtime_status: self.runtime_status.clone(),
            control: self.control.clone(),
            action_sequencer: self.action_sequencer.clone(),
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
        Self::with_threads_and_factory(start, vec![thread], None, control)
    }

    /// Starts a service over several preconfigured Threads.
    ///
    /// The first supplied thread is assigned the default `start.thread_id`;
    /// additional threads retain their identities. Turns are serialized by
    /// the service worker, while lifecycle and checkpoint operations remain
    /// addressed by thread identity.
    pub fn with_threads(start: ThreadStart, threads: Vec<Thread<M>>) -> Self {
        Self::with_threads_and_factory(start, threads, None, Arc::new(RunControl::new()))
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
        let event_replay = Arc::new(Mutex::new(VecDeque::with_capacity(EVENT_REPLAY_BUFFER)));
        let runtime_status = Arc::new(Mutex::new(mini_agent_app_server_protocol::RuntimeStatus {
            phase: mini_agent_app_server_protocol::RuntimePhase::Idle,
            thread_id: start.thread_id.clone(),
            turn_id: None,
            operation_id: None,
            checkpoint_seq: None,
            state_revision: 0,
            timestamp_ms: crate::status::timestamp_ms(),
            error: None,
        }));
        let worker_events = events.clone();
        let worker_notifications = notifications.clone();
        let worker_event_replay = event_replay.clone();
        let worker_runtime_status = runtime_status.clone();
        let worker_thread_ids = thread_ids.clone();
        let worker_revision = runtime_revision.clone();
        let worker_factory = factory.clone();
        let worker_control = control.clone();
        let action_sequencer = ActionSequencer::new();
        let worker_action_sequencer = action_sequencer.clone();
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
                    worker_event_replay,
                    worker_runtime_status,
                    worker_thread_ids,
                    worker_revision,
                    worker_factory,
                    worker_action_sequencer,
                    worker_control,
                ));
            })
            .expect("app-server worker thread must start");
        Self {
            commands,
            events,
            notifications,
            event_replay,
            runtime_status,
            control,
            action_sequencer,
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

    pub(crate) fn runtime_status_handle(&self) -> crate::status::RuntimeStatusHandle {
        self.runtime_status.clone()
    }

    /// Stops the worker after all earlier commands have settled.
    ///
    /// An active turn rejects shutdown with `Busy`; callers must first
    /// interrupt and wait for its settled result. The explicit seam is also
    /// what releases a SessionStore lock for an in-process restart.
    pub async fn shutdown(&self) -> Result<(), AppServerError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Shutdown { reply })
            .await
            .map_err(|_| AppServerError::Disconnected)?;
        response.await.map_err(|_| AppServerError::Disconnected)?
    }

    pub(crate) fn notifications(&self) -> broadcast::Sender<RuntimeNotification> {
        self.notifications.clone()
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

    async fn request_value<T, F>(&self, action: F) -> Result<T, AppServerError>
    where
        F: Future<Output = Result<ActionResponse<T>, ActionFailure>>,
    {
        action
            .await
            .map(ActionResponse::into_value)
            .map_err(ActionFailure::into_error)
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
        self.request_value(self.thread_read_action(thread_id)).await
    }

    pub(crate) async fn thread_read_action(
        &self,
        thread_id: ThreadId,
    ) -> Result<ActionResponse<ThreadCheckpoint>, ActionFailure> {
        self.request_action(|reply| Command::ReadThread { thread_id, reply })
            .await
    }

    pub async fn thread_items(
        &self,
        params: mini_agent_app_server_protocol::ThreadItemsListParams,
    ) -> Result<mini_agent_app_server_protocol::ThreadItemsListResult, AppServerError> {
        self.request_value(self.thread_items_action(params)).await
    }

    pub(crate) async fn thread_items_action(
        &self,
        params: mini_agent_app_server_protocol::ThreadItemsListParams,
    ) -> Result<ActionResponse<mini_agent_app_server_protocol::ThreadItemsListResult>, ActionFailure>
    {
        self.request_action(|reply| Command::ReadItems { params, reply })
            .await
    }

    /// Applies a host-side update after all earlier commands for a thread.
    pub async fn thread_update_for(
        &self,
        thread_id: ThreadId,
        update: ThreadUpdate,
    ) -> Result<(), AppServerError> {
        self.request_value(self.thread_update_action(thread_id, update))
            .await
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
        self.request_value(self.thread_reset_action(thread_id, new_thread_id, next_turn_number))
            .await
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
        self.request_value(self.thread_close_action(thread_id))
            .await
    }

    pub(crate) async fn thread_close_action(
        &self,
        thread_id: ThreadId,
    ) -> Result<ActionResponse<()>, ActionFailure> {
        self.request_action(|reply| Command::CloseThread { thread_id, reply })
            .await
    }

    pub async fn thread_start(&self, thread_id: ThreadId) -> Result<ThreadId, AppServerError> {
        self.request_value(self.thread_start_action(thread_id))
            .await
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
        self.request_value(self.thread_fork_action(source_thread_id, new_thread_id))
            .await
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
        self.request_value(self.thread_resume_action(thread_id, checkpoint))
            .await
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
        self.request_value(self.turn_read_action(turn_id))
            .await
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

    pub(crate) fn replay_events(
        &self,
        thread_id: &ThreadId,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<crate::EventReplaySnapshot, AppServerError> {
        if !self.has_thread(thread_id) {
            return Err(AppServerError::ThreadNotFound(thread_id.clone()));
        }
        let after_sequence = after_sequence.unwrap_or_default();
        let replay = self.event_replay.lock().unwrap();
        let oldest_sequence = replay
            .iter()
            .filter(|event| event.thread_id == *thread_id)
            .map(|event| event.sequence)
            .min();
        let has_gap =
            oldest_sequence.is_some_and(|oldest| after_sequence.saturating_add(1) < oldest);
        let events = replay
            .iter()
            .filter(|event| event.thread_id == *thread_id && event.sequence > after_sequence)
            .take(limit)
            .cloned()
            .collect();
        Ok(crate::EventReplaySnapshot {
            events,
            oldest_sequence,
            has_gap,
        })
    }

    pub(crate) fn runtime_status(&self) -> mini_agent_app_server_protocol::RuntimeStatus {
        self.runtime_status.lock().unwrap().clone()
    }

    /// Starts, steers, or queues a turn according to the typed input mode.
    pub async fn turn_start_for(
        &self,
        thread_id: ThreadId,
        request: TurnStart,
    ) -> Result<TurnSubmission, AppServerError> {
        self.request_value(self.submit_start_action(thread_id, request, None))
            .await
    }

    /// Steers the active turn when `turn_id` still identifies that turn.
    pub async fn turn_steer_for(
        &self,
        thread_id: ThreadId,
        turn_id: TurnId,
        text: impl Into<String>,
    ) -> Result<TurnSubmission, AppServerError> {
        self.request_value(self.submit_start_action(
            thread_id,
            TurnStart::new(TurnInput::new(TurnInputMode::Steer, text)),
            Some(turn_id),
        ))
        .await
    }

    pub(crate) async fn submit_start_action(
        &self,
        thread_id: ThreadId,
        request: TurnStart,
        expected_turn_id: Option<TurnId>,
    ) -> Result<ActionResponse<TurnSubmission>, ActionFailure> {
        if request.input.mode == TurnInputMode::Steer
            && let Some(expected_turn_id) = expected_turn_id.as_ref()
            && self.active_turn_matches(&thread_id, expected_turn_id)
        {
            let turn_id = expected_turn_id.clone();
            self.control.submit(request.input).map_err(|error| {
                ActionFailure::without_receipt(AppServerError::InputQueue(error.to_string()))
            })?;
            let receipt = self.action_sequencer.receipt(self.runtime_revision.clone());
            let state_revision = receipt.current_revision();
            return Ok(ActionResponse {
                value: TurnSubmission::Steered { turn_id },
                receipt,
                state_revision,
            });
        }
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
        self.request_value(self.turn_cancel_action(thread_id, request))
            .await
    }

    pub(crate) async fn turn_cancel_action(
        &self,
        thread_id: ThreadId,
        request: TurnCancel,
    ) -> Result<ActionResponse<()>, ActionFailure> {
        if self.active_turn_matches(&thread_id, &request.turn_id) {
            // The worker owns command ordering, but a blocking tool may keep
            // it from receiving this command. Set the shared stop token first
            // so cancellation-aware tools can terminate immediately.
            self.control.request_cancel();
            let (reply, _response) = oneshot::channel();
            let _ = self.commands.try_send(Command::Cancel {
                thread_id,
                request,
                reply,
            });
            let receipt = self.action_sequencer.receipt(self.runtime_revision.clone());
            let state_revision = receipt.current_revision();
            return Ok(ActionResponse {
                value: (),
                receipt,
                state_revision,
            });
        }
        self.request_action(|reply| Command::Cancel {
            thread_id,
            request,
            reply,
        })
        .await
    }

    fn active_turn_matches(&self, thread_id: &ThreadId, turn_id: &TurnId) -> bool {
        let status = self.runtime_status.lock().unwrap();
        status.thread_id == *thread_id
            && status.turn_id.as_ref() == Some(turn_id)
            && !matches!(
                status.phase,
                mini_agent_app_server_protocol::RuntimePhase::Idle
                    | mini_agent_app_server_protocol::RuntimePhase::Completed
                    | mini_agent_app_server_protocol::RuntimePhase::Failed
                    | mini_agent_app_server_protocol::RuntimePhase::Stopping
            )
    }
}
