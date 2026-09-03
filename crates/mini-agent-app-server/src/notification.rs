use crate::goal_runtime::GoalRuntimeEvent;
use crate::management::SettingsRuntimeEvent;
use mini_agent_app_server_protocol::ItemCompletedNotification;
use mini_agent_app_server_protocol::ItemStartedNotification;
use mini_agent_protocol::EventEnvelope;

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
    Goal(GoalRuntimeEvent),
    Settings(SettingsRuntimeEvent),
}
