//! Read-only runtime state projection.

pub(crate) type RuntimeStateSnapshot = (bool, Option<mini_agent_host::GoalState>, Vec<String>);
