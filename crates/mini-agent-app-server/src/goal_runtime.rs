use crate::workflows::{GoalState, VerifierVerdict};
use mini_agent_app_server_protocol::ThreadGoal;
use mini_agent_app_server_protocol::ThreadGoalStatus;
use mini_agent_host::HostWorkflowStore;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::TurnId;
use std::io;
use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub(crate) enum GoalRuntimeEvent {
    Updated {
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
        state: GoalState,
    },
    Cleared {
        thread_id: ThreadId,
    },
}

/// Serialized owner of one Thread Goal's durable state and lifecycle actions.
///
/// This is an App Server state component, not a second turn loop. It will own
/// verifier and continuation decisions as those phases move out of the legacy
/// workflow facade; Host remains the persistence primitive.
#[derive(Clone, Debug)]
pub(crate) struct GoalRuntime {
    store: HostWorkflowStore,
    events: broadcast::Sender<GoalRuntimeEvent>,
}

impl GoalRuntime {
    pub(crate) fn new(
        store: HostWorkflowStore,
        events: broadcast::Sender<GoalRuntimeEvent>,
    ) -> Self {
        Self { store, events }
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

    pub(crate) fn init_goal(&self, objective: &str) -> io::Result<GoalState> {
        self.store.init_goal(objective)
    }

    pub(crate) fn set_goal(
        &self,
        objective: Option<&str>,
        status: Option<ThreadGoalStatus>,
        token_budget: Option<Option<i64>>,
    ) -> io::Result<GoalState> {
        let current = self.store.load_goal_state()?;
        if current
            .as_ref()
            .is_some_and(|goal| goal.status == mini_agent_host::GoalStatus::Running)
        {
            if objective.is_none()
                && status.is_none_or(|status| status == ThreadGoalStatus::Active)
                && token_budget.is_none()
            {
                return current
                    .as_ref()
                    .cloned()
                    .ok_or_else(|| io::Error::other("goal state disappeared"));
            }
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "running goal must be cleared before replacement",
            ));
        }
        if status.is_some_and(|status| status != ThreadGoalStatus::Active) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a new goal must start with active status",
            ));
        }
        let objective = objective
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "objective is required when creating a goal",
                )
            })?;
        self.store.set_goal(objective, token_budget.flatten())
    }

    pub(crate) fn clear_goal(&self) -> io::Result<bool> {
        self.store.clear_goal()
    }

    pub(crate) fn notify_updated(
        &self,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
        state: GoalState,
    ) {
        let _ = self.events.send(GoalRuntimeEvent::Updated {
            thread_id,
            turn_id,
            state,
        });
    }

    pub(crate) fn notify_cleared(&self, thread_id: ThreadId) {
        let _ = self.events.send(GoalRuntimeEvent::Cleared { thread_id });
    }

    pub(crate) fn verification_criteria(&self) -> io::Result<String> {
        self.store.verification_criteria()
    }

    pub(crate) fn record_verifier_verdict(
        &self,
        checkpoint_seq: u64,
        output: &str,
    ) -> io::Result<()> {
        self.store.record_verifier_verdict(checkpoint_seq, output)
    }

    pub(crate) fn advance_goal(&self, verdict: Option<VerifierVerdict>) -> io::Result<GoalState> {
        self.store.advance_goal(verdict)
    }

    pub(crate) fn pause_goal(&self) -> io::Result<()> {
        self.store.pause_goal()
    }

    pub(crate) fn fail_goal(&self) -> io::Result<GoalState> {
        self.store.fail_goal()
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
        },
        token_budget: state.token_budget,
        tokens_used: 0,
        time_used_seconds: if state.created_at_ms == 0 {
            0
        } else {
            (state.updated_at_ms.saturating_sub(state.created_at_ms) / 1000) as i64
        },
        created_at: (state.created_at_ms / 1000) as i64,
        updated_at: (state.updated_at_ms / 1000) as i64,
    }
}
