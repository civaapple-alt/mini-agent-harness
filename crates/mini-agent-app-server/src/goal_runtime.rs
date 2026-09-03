use crate::goal_service::GoalSetOutcome;
use crate::goal_service::GoalState;
use crate::goal_service::VerifierVerdict;
use crate::notification::RuntimeNotification;
use mini_agent_app_server_protocol::ThreadGoal;
use mini_agent_app_server_protocol::ThreadGoalStatus;
use mini_agent_host::HostWorkflowStore;
use mini_agent_host::RuntimeConfig;
use mini_agent_protocol::Message;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::TurnId;
use std::io;
use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub(crate) enum GoalRuntimeEvent {
    Updated {
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
        state: Box<GoalState>,
    },
    Cleared {
        thread_id: ThreadId,
    },
}

pub(crate) struct GoalVerificationRequest {
    pub(crate) thread_id: ThreadId,
    pub(crate) goal_id: String,
    pub(crate) turn_id: TurnId,
    pub(crate) checkpoint_seq: u64,
    pub(crate) messages: Vec<Message>,
    pub(crate) criteria: String,
    pub(crate) runtime_config: RuntimeConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingVerification {
    goal_id: String,
    turn_id: TurnId,
    checkpoint_seq: u64,
}

/// Serialized owner of one Thread Goal's durable state and lifecycle actions.
///
/// This is an App Server state component, not a second turn loop. It owns
/// verifier and continuation decisions; Host remains the persistence
/// primitive.
#[derive(Clone)]
pub(crate) struct GoalRuntimeHandle {
    store: HostWorkflowStore,
    events: broadcast::Sender<GoalRuntimeEvent>,
    notifications: Option<broadcast::Sender<RuntimeNotification>>,
    verifier_config: Option<RuntimeConfig>,
    pending_verification: Option<PendingVerification>,
    scheduled_goal: Option<String>,
}

/// Domain owner for one Thread Goal. The handle contains the serialized
/// runtime behavior; this service is the App Server's mutation boundary.
pub(crate) struct GoalService {
    handle: GoalRuntimeHandle,
}

impl GoalService {
    pub(crate) fn with_notifications(
        store: HostWorkflowStore,
        events: broadcast::Sender<GoalRuntimeEvent>,
        verifier_config: Option<RuntimeConfig>,
        notifications: Option<broadcast::Sender<RuntimeNotification>>,
    ) -> Self {
        Self {
            handle: GoalRuntimeHandle::with_notifications(
                store,
                events,
                verifier_config,
                notifications,
            ),
        }
    }

    pub(crate) fn plan_active(&self) -> bool {
        self.handle.plan_active()
    }

    pub(crate) fn init_plan_mode(&self, prompt: Option<&str>) -> io::Result<std::path::PathBuf> {
        self.handle.init_plan_mode(prompt)
    }

    pub(crate) fn disable_plan_mode(&self) -> io::Result<()> {
        self.handle.disable_plan_mode()
    }

    pub(crate) fn load_goal_state(&self) -> io::Result<Option<GoalState>> {
        self.handle.load_goal_state()
    }

    pub(crate) fn goal_dir(&self) -> std::path::PathBuf {
        self.handle.goal_dir()
    }

    pub(crate) fn set_goal(
        &mut self,
        objective: Option<&str>,
        status: Option<ThreadGoalStatus>,
        token_budget: Option<Option<i64>>,
    ) -> io::Result<GoalSetOutcome> {
        let previous = self.handle.load_goal_state()?;
        let current = self
            .handle
            .apply_goal_set(objective, status, token_budget)?;
        Ok(GoalSetOutcome { previous, current })
    }

    pub(crate) fn clear_goal(&mut self) -> io::Result<bool> {
        self.handle.clear_goal()
    }

    pub(crate) fn reserve_turn(&mut self, goal_id: &str) -> bool {
        self.handle.reserve_turn(goal_id)
    }

    pub(crate) fn release_turn(&mut self, goal_id: &str) {
        self.handle.release_turn(goal_id);
    }

    pub(crate) fn mark_turn_started(
        &mut self,
        goal_id: &str,
        turn_id: &TurnId,
    ) -> io::Result<Option<GoalState>> {
        self.handle.mark_turn_started(goal_id, turn_id)
    }

    pub(crate) fn mark_turn_settled(
        &self,
        goal_id: &str,
        turn_id: &TurnId,
    ) -> io::Result<Option<GoalState>> {
        self.handle.mark_turn_settled(goal_id, turn_id)
    }

    pub(crate) fn prepare_verification(
        &mut self,
        thread_id: ThreadId,
        goal_id: &str,
        checkpoint_seq: u64,
        messages: Vec<Message>,
    ) -> io::Result<Option<GoalVerificationRequest>> {
        self.handle
            .prepare_verification(thread_id, goal_id, checkpoint_seq, messages)
    }

    pub(crate) fn complete_verification(
        &mut self,
        goal_id: &str,
        turn_id: &TurnId,
        checkpoint_seq: u64,
        current_checkpoint_seq: u64,
        result: Result<(String, VerifierVerdict), String>,
    ) -> io::Result<Option<GoalState>> {
        self.handle.complete_verification(
            goal_id,
            turn_id,
            checkpoint_seq,
            current_checkpoint_seq,
            result,
        )
    }

    pub(crate) fn notify_updated(
        &self,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
        state: GoalState,
    ) {
        self.handle.notify_updated(thread_id, turn_id, state);
    }

    pub(crate) fn notify_cleared(&self, thread_id: ThreadId) {
        self.handle.notify_cleared(thread_id);
    }

    pub(crate) fn fail_goal_with_reason(&self, reason: &str) -> io::Result<GoalState> {
        self.handle.fail_goal_with_reason(reason)
    }

    pub(crate) fn limit_turn(
        &self,
        goal_id: &str,
        turn_id: &TurnId,
        status: mini_agent_host::GoalStatus,
        reason: &str,
    ) -> io::Result<Option<GoalState>> {
        self.handle.limit_turn(goal_id, turn_id, status, reason)
    }

    pub(crate) fn record_turn_usage(
        &self,
        goal_id: &str,
        turn_id: &TurnId,
        tokens: u64,
    ) -> io::Result<Option<GoalState>> {
        self.handle.record_turn_usage(goal_id, turn_id, tokens)
    }
}

impl GoalRuntimeHandle {
    pub(crate) fn with_notifications(
        store: HostWorkflowStore,
        events: broadcast::Sender<GoalRuntimeEvent>,
        verifier_config: Option<RuntimeConfig>,
        notifications: Option<broadcast::Sender<RuntimeNotification>>,
    ) -> Self {
        Self {
            store,
            events,
            notifications,
            verifier_config,
            pending_verification: None,
            scheduled_goal: None,
        }
    }

    pub(crate) fn plan_active(&self) -> bool {
        self.store.plan_active()
    }

    pub(crate) fn init_plan_mode(&self, prompt: Option<&str>) -> io::Result<std::path::PathBuf> {
        self.store.init_plan_mode(prompt)
    }

    pub(crate) fn disable_plan_mode(&self) -> io::Result<()> {
        self.store.disable_plan_mode()
    }

    pub(crate) fn load_goal_state(&self) -> io::Result<Option<GoalState>> {
        self.store.load_goal_state()
    }

    pub(crate) fn goal_dir(&self) -> std::path::PathBuf {
        self.store.goal_dir()
    }

    fn apply_goal_set(
        &mut self,
        objective: Option<&str>,
        status: Option<ThreadGoalStatus>,
        token_budget: Option<Option<i64>>,
    ) -> io::Result<GoalState> {
        let current = self.store.load_goal_state()?;
        if let Some(current) = current {
            let default_status = match current.status {
                mini_agent_host::GoalStatus::Running => ThreadGoalStatus::Active,
                mini_agent_host::GoalStatus::UserPaused => ThreadGoalStatus::Paused,
                _ => {
                    return self.create_goal(objective, status, token_budget);
                }
            };
            let requested_status = status.unwrap_or(default_status);
            if !matches!(
                requested_status,
                ThreadGoalStatus::Active | ThreadGoalStatus::Paused
            ) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Goal can only transition to active or paused",
                ));
            }
            let mut goal = if objective.is_some() || token_budget.is_some() {
                self.store
                    .update_goal(objective, token_budget)?
                    .ok_or_else(|| io::Error::other("goal state disappeared"))?
            } else {
                current
            };
            match (goal.status, requested_status) {
                (mini_agent_host::GoalStatus::Running, ThreadGoalStatus::Paused) => {
                    goal = self
                        .store
                        .pause_goal()?
                        .ok_or_else(|| io::Error::other("goal state disappeared"))?;
                }
                (mini_agent_host::GoalStatus::UserPaused, ThreadGoalStatus::Active) => {
                    goal = self
                        .store
                        .resume_goal()?
                        .ok_or_else(|| io::Error::other("goal state disappeared"))?;
                }
                _ => {}
            }
            if goal.status != mini_agent_host::GoalStatus::Running {
                self.scheduled_goal = None;
            }
            return Ok(goal);
        }
        self.create_goal(objective, status, token_budget)
    }

    fn create_goal(
        &mut self,
        objective: Option<&str>,
        status: Option<ThreadGoalStatus>,
        token_budget: Option<Option<i64>>,
    ) -> io::Result<GoalState> {
        if status.is_some_and(|status| status != ThreadGoalStatus::Active) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a new Goal must start with active status",
            ));
        }
        let objective = objective
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "objective is required when creating a Goal",
                )
            })?;
        self.scheduled_goal = None;
        self.store.set_goal(objective, token_budget.flatten())
    }

    pub(crate) fn clear_goal(&mut self) -> io::Result<bool> {
        self.pending_verification = None;
        self.scheduled_goal = None;
        self.store.clear_goal()
    }

    pub(crate) fn reserve_turn(&mut self, goal_id: &str) -> bool {
        if self.scheduled_goal.as_deref() == Some(goal_id) {
            return false;
        }
        self.scheduled_goal = Some(goal_id.to_string());
        true
    }

    pub(crate) fn release_turn(&mut self, goal_id: &str) {
        if self.scheduled_goal.as_deref() == Some(goal_id) {
            self.scheduled_goal = None;
        }
    }

    pub(crate) fn mark_turn_started(
        &mut self,
        goal_id: &str,
        turn_id: &TurnId,
    ) -> io::Result<Option<GoalState>> {
        self.scheduled_goal = None;
        self.store.mark_goal_turn_started(goal_id, turn_id.as_str())
    }

    pub(crate) fn mark_turn_settled(
        &self,
        goal_id: &str,
        turn_id: &TurnId,
    ) -> io::Result<Option<GoalState>> {
        self.store.mark_goal_turn_settled(goal_id, turn_id.as_str())
    }

    pub(crate) fn prepare_verification(
        &mut self,
        thread_id: ThreadId,
        goal_id: &str,
        checkpoint_seq: u64,
        messages: Vec<Message>,
    ) -> io::Result<Option<GoalVerificationRequest>> {
        if self.pending_verification.is_some() {
            return Ok(None);
        }
        let Some(state) = self.store.load_goal_state()? else {
            return Ok(None);
        };
        let Some(turn_id) = state.active_turn_id.clone().map(TurnId::new) else {
            return Ok(None);
        };
        if state.goal_id != goal_id
            || state.status != mini_agent_host::GoalStatus::Running
            || !state.active_turn_settled
        {
            return Ok(None);
        }
        let Some(runtime_config) = self.verifier_config.clone() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "goal verifier is not configured",
            ));
        };
        let criteria = self.store.verification_criteria()?;
        self.pending_verification = Some(PendingVerification {
            goal_id: state.goal_id.clone(),
            turn_id: turn_id.clone(),
            checkpoint_seq,
        });
        Ok(Some(GoalVerificationRequest {
            thread_id,
            goal_id: state.goal_id,
            turn_id,
            checkpoint_seq,
            messages,
            criteria,
            runtime_config,
        }))
    }

    pub(crate) fn complete_verification(
        &mut self,
        goal_id: &str,
        turn_id: &TurnId,
        checkpoint_seq: u64,
        current_checkpoint_seq: u64,
        result: Result<(String, VerifierVerdict), String>,
    ) -> io::Result<Option<GoalState>> {
        let Some(pending) = self.pending_verification.as_ref() else {
            return Ok(None);
        };
        if pending.goal_id != goal_id || pending.turn_id != *turn_id {
            return Ok(None);
        }
        let pending_checkpoint_seq = pending.checkpoint_seq;
        let Some(state) = self.store.load_goal_state()? else {
            self.pending_verification = None;
            return Ok(None);
        };
        if state.goal_id != goal_id
            || state.status != mini_agent_host::GoalStatus::Running
            || state.active_turn_id.as_deref() != Some(turn_id.as_str())
            || !state.active_turn_settled
        {
            self.pending_verification = None;
            return Ok(None);
        }
        if pending_checkpoint_seq != checkpoint_seq
            || pending_checkpoint_seq != current_checkpoint_seq
        {
            self.pending_verification = None;
            return self
                .store
                .fail_goal_with_reason("goal verifier result was stale for the settled checkpoint")
                .map(Some);
        }
        self.pending_verification = None;
        let next = match result {
            Ok((output, verdict)) => {
                match self.store.record_verifier_verdict(checkpoint_seq, &output) {
                    Ok(()) => self.store.advance_goal(Some(verdict)),
                    Err(error) => self.store.fail_goal_with_reason(&error.to_string()),
                }
            }
            Err(error) => self.store.fail_goal_with_reason(&error),
        }?;
        Ok(Some(next))
    }

    pub(crate) fn notify_updated(
        &self,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
        state: GoalState,
    ) {
        let _ = self.events.send(GoalRuntimeEvent::Updated {
            thread_id: thread_id.clone(),
            turn_id: turn_id.clone(),
            state: Box::new(state.clone()),
        });
        if let Some(notifications) = &self.notifications {
            let _ = notifications.send(RuntimeNotification::Goal(GoalRuntimeEvent::Updated {
                thread_id,
                turn_id,
                state: Box::new(state),
            }));
        }
    }

    pub(crate) fn notify_cleared(&self, thread_id: ThreadId) {
        let event = GoalRuntimeEvent::Cleared {
            thread_id: thread_id.clone(),
        };
        let _ = self.events.send(event.clone());
        if let Some(notifications) = &self.notifications {
            let _ = notifications.send(RuntimeNotification::Goal(event));
        }
    }

    pub(crate) fn fail_goal_with_reason(&self, reason: &str) -> io::Result<GoalState> {
        self.store.fail_goal_with_reason(reason)
    }

    pub(crate) fn limit_turn(
        &self,
        goal_id: &str,
        turn_id: &TurnId,
        status: mini_agent_host::GoalStatus,
        reason: &str,
    ) -> io::Result<Option<GoalState>> {
        self.store
            .limit_goal_turn(goal_id, turn_id.as_str(), status, reason)
    }

    pub(crate) fn record_turn_usage(
        &self,
        goal_id: &str,
        turn_id: &TurnId,
        tokens: u64,
    ) -> io::Result<Option<GoalState>> {
        self.store
            .record_goal_usage(goal_id, turn_id.as_str(), tokens)
    }
}

pub(crate) fn project_goal(thread_id: ThreadId, state: GoalState) -> ThreadGoal {
    ThreadGoal {
        thread_id,
        objective: state.objective,
        status: match state.status {
            mini_agent_host::GoalStatus::Running => ThreadGoalStatus::Active,
            mini_agent_host::GoalStatus::Converged => ThreadGoalStatus::Complete,
            mini_agent_host::GoalStatus::Failed => ThreadGoalStatus::Blocked,
            mini_agent_host::GoalStatus::UserPaused => ThreadGoalStatus::Paused,
            mini_agent_host::GoalStatus::UsageLimited => ThreadGoalStatus::UsageLimited,
            mini_agent_host::GoalStatus::BudgetLimited => ThreadGoalStatus::BudgetLimited,
        },
        token_budget: state.token_budget,
        tokens_used: state.tokens_used,
        time_used_seconds: if state.created_at_ms == 0 {
            0
        } else {
            (state.updated_at_ms.saturating_sub(state.created_at_ms) / 1000) as i64
        },
        created_at: (state.created_at_ms / 1000) as i64,
        updated_at: (state.updated_at_ms / 1000) as i64,
    }
}

#[cfg(test)]
#[path = "goal_runtime_tests.rs"]
mod tests;
