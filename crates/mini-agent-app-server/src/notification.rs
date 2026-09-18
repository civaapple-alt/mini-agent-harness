use crate::goal_runtime::GoalRuntimeEvent;
use crate::management::SettingsRuntimeEvent;
use mini_agent_app_server_protocol::ItemCompletedNotification;
use mini_agent_app_server_protocol::ItemStartedNotification;
use mini_agent_app_server_protocol::NotebookUpdatedNotification;
use mini_agent_app_server_protocol::{RuntimeStatus, WorkflowLifecycleNotification};
use mini_agent_protocol::EventEnvelope;

#[derive(Clone, Debug)]
pub(crate) enum WorkflowRuntimeEvent {
    CheckpointCommitted(WorkflowLifecycleNotification),
    GoalVerificationStarted(WorkflowLifecycleNotification),
    GoalVerificationCompleted(WorkflowLifecycleNotification),
    GoalVerificationFailed(WorkflowLifecycleNotification),
    GoalContinuationQueued(WorkflowLifecycleNotification),
    GoalContinuationStarted(WorkflowLifecycleNotification),
    PlanUpdated(WorkflowLifecycleNotification),
    PlanCleanupStarted(WorkflowLifecycleNotification),
    PlanCleanupCompleted(WorkflowLifecycleNotification),
    PlanCleanupFailed(WorkflowLifecycleNotification),
}

/// One ordered runtime notification stream for the App Server wire adapter.
///
/// Core, Goal, and settings producers all run on the serialized runtime
/// worker. Keeping their public notifications on one broadcast channel makes
/// their send order observable without creating a second history store.
#[derive(Clone, Debug)]
pub(crate) enum RuntimeNotification {
    Event(EventEnvelope),
    ItemStarted(ItemStartedNotification),
    ItemCompleted(ItemCompletedNotification),
    NotebookUpdated(NotebookUpdatedNotification),
    Goal(GoalRuntimeEvent),
    Settings(SettingsRuntimeEvent),
    Status(RuntimeStatus),
    Workflow(WorkflowRuntimeEvent),
}
