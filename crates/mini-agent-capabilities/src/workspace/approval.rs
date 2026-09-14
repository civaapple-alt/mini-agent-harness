use super::*;
use crate::security::{ApprovalStore, action_grant_key, is_high_risk, is_trusted_high_risk};
use mini_agent_protocol::{
    ActionGrantScope, ApprovalOutcome, ApprovalPolicy, ToolApprovalRequest, ToolApprovalResolution,
};

type ApprovalCallback =
    dyn Fn(&ToolApprovalRequest) -> Result<ToolApprovalResolution, ToolError> + Send + Sync;

#[derive(Clone, Debug, Default)]
struct ApprovalBinding {
    project_id: Option<String>,
    workspace_id: Option<String>,
    workspace_revision: Option<u64>,
    session_id: Option<String>,
}

impl ApprovalBinding {
    fn owner(&self, scope: ActionGrantScope) -> Option<String> {
        match scope {
            ActionGrantScope::Once => None,
            ActionGrantScope::Session => self.session_id.clone(),
            ActionGrantScope::Project => match (&self.project_id, &self.workspace_id) {
                (Some(project), Some(workspace)) => Some(format!("{project}\0{workspace}")),
                _ => None,
            },
        }
    }
}

#[derive(Clone)]
pub struct ApprovalController {
    approval_policy: Arc<RwLock<ApprovalPolicy>>,
    access_scope: Arc<RwLock<String>>,
    policy: Arc<RwLock<SecurityPolicy>>,
    store: ApprovalStore,
    callback: Arc<ApprovalCallback>,
    living_plan: Arc<Mutex<Option<PathBuf>>>,
    plan_scratch: Arc<Mutex<Option<PathBuf>>>,
    read_only_agent: Arc<AtomicBool>,
    goal_dir: Arc<Mutex<Option<PathBuf>>>,
    session_dir: Arc<Mutex<Option<PathBuf>>>,
    approval_binding: Arc<RwLock<ApprovalBinding>>,
}

impl ApprovalController {
    pub fn new(approval_policy: ApprovalPolicy) -> Self {
        Self::with_policy_and_callback(
            approval_policy,
            SecurityPolicy::for_preset(SecurityPreset::Default),
            terminal_approval,
        )
    }

    pub fn with_preset(approval_policy: ApprovalPolicy, preset: SecurityPreset) -> Self {
        Self::with_policy_and_callback(
            approval_policy,
            SecurityPolicy::for_preset(preset),
            terminal_approval,
        )
    }

    pub fn with_callback(
        approval_policy: ApprovalPolicy,
        callback: impl Fn(&ToolApprovalRequest) -> Result<ToolApprovalResolution, ToolError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::with_policy_and_callback(
            approval_policy,
            SecurityPolicy::for_preset(SecurityPreset::Default),
            callback,
        )
    }

    pub fn with_policy_and_callback(
        approval_policy: ApprovalPolicy,
        policy: SecurityPolicy,
        callback: impl Fn(&ToolApprovalRequest) -> Result<ToolApprovalResolution, ToolError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::with_callbacks(approval_policy, policy, Arc::new(callback))
    }

    fn with_callbacks(
        approval_policy: ApprovalPolicy,
        policy: SecurityPolicy,
        callback: Arc<ApprovalCallback>,
    ) -> Self {
        Self {
            approval_policy: Arc::new(RwLock::new(approval_policy)),
            access_scope: Arc::new(RwLock::new("project".to_string())),
            policy: Arc::new(RwLock::new(policy)),
            store: ApprovalStore::new(),
            callback,
            living_plan: Arc::new(Mutex::new(None)),
            plan_scratch: Arc::new(Mutex::new(None)),
            read_only_agent: Arc::new(AtomicBool::new(false)),
            goal_dir: Arc::new(Mutex::new(None)),
            session_dir: Arc::new(Mutex::new(None)),
            approval_binding: Arc::new(RwLock::new(ApprovalBinding::default())),
        }
    }

    pub fn preset(&self) -> SecurityPreset {
        self.policy.read().unwrap().preset
    }

    /// Replaces the policy selected by the resolved runtime profile while
    /// preserving the frontend approval callback and cached approvals.
    pub fn set_policy(&self, policy: SecurityPolicy) {
        *self.policy.write().unwrap() = policy;
    }

    pub fn ensure_not_denied(&self, action: &str) -> Result<(), ToolError> {
        if self.policy.read().unwrap().evaluate(action) == SecurityDecision::Deny {
            return Err(ToolError(format!("forbidden by security policy: {action}")));
        }
        Ok(())
    }

    pub fn approval_policy(&self) -> ApprovalPolicy {
        *self.approval_policy.read().unwrap()
    }

    pub fn set_approval_policy(&self, policy: ApprovalPolicy) {
        *self.approval_policy.write().unwrap() = policy;
    }

    pub fn set_access_scope(&self, access: impl Into<String>) {
        *self.access_scope.write().unwrap() = access.into();
    }

    /// Binds trusted Project/Workspace/Session identity used by scoped
    /// approval reuse. The Web client never supplies this identity.
    pub fn bind_approval_context(
        &self,
        project_id: Option<String>,
        workspace_id: Option<String>,
        workspace_revision: Option<u64>,
        session_id: Option<String>,
    ) {
        *self.approval_binding.write().unwrap() = ApprovalBinding {
            project_id,
            workspace_id,
            workspace_revision,
            session_id,
        };
    }

    pub fn with_approval_store(self, store: ApprovalStore) -> Self {
        Self { store, ..self }
    }

    pub fn set_living_plan(&self, path: Option<PathBuf>) {
        let normalized = path.map(|path| crate::path_policy::normalize_path(&path));
        let scratch = normalized
            .as_ref()
            .and_then(|path| path.parent())
            .map(|path| crate::path_policy::normalize_path(&path.join("scratch")));
        *self.living_plan.lock().unwrap() = normalized;
        *self.plan_scratch.lock().unwrap() = scratch;
    }

    pub fn living_plan(&self) -> Option<PathBuf> {
        self.living_plan.lock().unwrap().clone()
    }

    pub fn plan_scratch(&self) -> Option<PathBuf> {
        self.plan_scratch.lock().unwrap().clone()
    }

    pub fn set_read_only_agent(&self, read_only: bool) {
        self.read_only_agent.store(read_only, Ordering::Release);
    }

    pub fn read_only_agent(&self) -> bool {
        self.read_only_agent.load(Ordering::Acquire)
    }

    pub fn set_goal_dir(&self, path: Option<PathBuf>) {
        *self.goal_dir.lock().unwrap() = path.map(|path| crate::path_policy::normalize_path(&path));
    }

    pub fn goal_dir(&self) -> Option<PathBuf> {
        self.goal_dir.lock().unwrap().clone()
    }

    pub fn bind_session_file(&self, session_jsonl: &Path) {
        *self.session_dir.lock().unwrap() = session_jsonl
            .parent()
            .map(crate::path_policy::normalize_path);
    }

    pub fn session_dir(&self) -> Option<PathBuf> {
        self.session_dir.lock().unwrap().clone()
    }

    pub fn ensure_plan_mode_unlocked(&self) -> Result<(), ToolError> {
        if self.read_only_agent() {
            return Err(ToolError(
                "workspace mutations disabled by the active agent profile".to_string(),
            ));
        }
        match self.living_plan() {
            Some(living) => Err(ToolError(format!(
                "workspace mutations locked in Plan Mode; living plan is {}",
                living.display()
            ))),
            None => Ok(()),
        }
    }

    pub fn approve(&self, action: &str) -> Result<(), ToolError> {
        self.approve_request(&ToolApprovalRequest::legacy(action))
    }

    pub fn approve_request(&self, request: &ToolApprovalRequest) -> Result<(), ToolError> {
        let mut request = request.clone();
        let binding = self.approval_binding.read().unwrap().clone();
        if binding.project_id.is_some() {
            request.project_id = binding.project_id.clone();
        }
        if binding.workspace_id.is_some() {
            request.workspace_id = binding.workspace_id.clone();
        }
        if binding.workspace_revision.is_some() {
            request.workspace_revision = binding.workspace_revision;
        }
        if binding.session_id.is_some() {
            request.session_id = binding.session_id.clone();
        }
        match self.policy.read().unwrap().evaluate(&request.action) {
            SecurityDecision::Deny => {
                return Err(ToolError(format!(
                    "forbidden by security policy: {}",
                    request.action
                )));
            }
            SecurityDecision::Allow => return Ok(()),
            SecurityDecision::Ask => {}
        }
        let requires_approval = match self.approval_policy() {
            ApprovalPolicy::Interactive => true,
            ApprovalPolicy::Automatic => is_high_risk(&request),
            ApprovalPolicy::Trusted => is_trusted_high_risk(&request),
        };
        if !requires_approval {
            return Ok(());
        }
        let key = action_grant_key(&request, &self.access_scope.read().unwrap());
        let project_owner = binding.owner(ActionGrantScope::Project);
        let session_owner = binding
            .owner(ActionGrantScope::Session)
            .or_else(|| self.session_dir().map(|path| path.display().to_string()));
        if let Some(key) = &key
            && let Some(owner) = &session_owner
            && self.store.contains(ActionGrantScope::Session, owner, key)
        {
            return Ok(());
        }
        if let Some(key) = &key
            && let Some(owner) = &project_owner
            && self.store.contains(ActionGrantScope::Project, owner, key)
        {
            return Ok(());
        }
        let resolution = (self.callback)(&request)?;
        if resolution.outcome != ApprovalOutcome::Approved {
            return Err(ToolError(format!(
                "user denied: {}",
                resolution
                    .reason
                    .as_deref()
                    .unwrap_or(request.action.as_str())
            )));
        }
        if resolution.grant_scope != ActionGrantScope::Once {
            let key = key.ok_or_else(|| {
                ToolError("approval grant requires a complete action key".to_string())
            })?;
            let owner = binding
                .owner(resolution.grant_scope)
                .or_else(|| {
                    (resolution.grant_scope == ActionGrantScope::Session)
                        .then(|| self.session_dir().map(|path| path.display().to_string()))
                        .flatten()
                })
                .ok_or_else(|| ToolError("approval grant owner is unavailable".to_string()))?;
            self.store.insert(resolution.grant_scope, &owner, &key);
        }
        Ok(())
    }
}

fn terminal_approval(request: &ToolApprovalRequest) -> Result<ToolApprovalResolution, ToolError> {
    let action = request.action.as_str();
    if !io::stdin().is_terminal() {
        return Err(ToolError(format!(
            "denied non-interactive action: {action}"
        )));
    }
    eprint!("approve {action}? [y/N] ");
    io::stderr()
        .flush()
        .map_err(|error| ToolError(error.to_string()))?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|error| ToolError(error.to_string()))?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(ToolApprovalResolution::once(ApprovalOutcome::Approved))
    } else {
        Ok(ToolApprovalResolution {
            outcome: ApprovalOutcome::Denied,
            grant_scope: ActionGrantScope::Once,
            reason: Some("user denied the action".to_string()),
        })
    }
}
