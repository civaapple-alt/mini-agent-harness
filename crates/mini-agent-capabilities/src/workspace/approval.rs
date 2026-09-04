use super::*;
use crate::security::ApprovalScope;
use crate::security::ApprovalStore;
use mini_agent_protocol::ToolApprovalRequest;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalMode {
    Interactive,
    Automatic,
}

type ApprovalCallback = dyn Fn(&str) -> Result<bool, ToolError> + Send + Sync;
type ContextualApprovalCallback =
    dyn Fn(&ToolApprovalRequest) -> Result<bool, ToolError> + Send + Sync;

#[derive(Clone, Debug, Default)]
struct ApprovalBinding {
    project_id: Option<String>,
    workspace_id: Option<String>,
    workspace_revision: Option<u64>,
    session_id: Option<String>,
}

impl ApprovalBinding {
    fn owner(&self, scope: ApprovalScope) -> Option<String> {
        match scope {
            ApprovalScope::PerAction => None,
            ApprovalScope::CurrentSession => self.session_id.clone(),
            ApprovalScope::CurrentProject => match (&self.project_id, &self.workspace_id) {
                (Some(project), Some(workspace)) => Some(format!("{project}\0{workspace}")),
                _ => None,
            },
        }
    }
}

#[derive(Clone)]
pub struct ApprovalController {
    automatic: Arc<AtomicBool>,
    policy: Arc<RwLock<SecurityPolicy>>,
    store: ApprovalStore,
    callback: Arc<ApprovalCallback>,
    context_callback: Option<Arc<ContextualApprovalCallback>>,
    living_plan: Arc<Mutex<Option<PathBuf>>>,
    plan_scratch: Arc<Mutex<Option<PathBuf>>>,
    read_only_agent: Arc<AtomicBool>,
    goal_dir: Arc<Mutex<Option<PathBuf>>>,
    session_dir: Arc<Mutex<Option<PathBuf>>>,
    approval_scope: Arc<RwLock<ApprovalScope>>,
    approval_binding: Arc<RwLock<ApprovalBinding>>,
}

impl ApprovalController {
    pub fn new(mode: ApprovalMode) -> Self {
        Self::with_policy_and_callback(
            mode,
            SecurityPolicy::for_preset(SecurityPreset::Default),
            terminal_approval,
        )
    }

    pub fn with_preset(mode: ApprovalMode, preset: SecurityPreset) -> Self {
        Self::with_policy_and_callback(mode, SecurityPolicy::for_preset(preset), terminal_approval)
    }

    pub fn with_callback(
        mode: ApprovalMode,
        callback: impl Fn(&str) -> Result<bool, ToolError> + Send + Sync + 'static,
    ) -> Self {
        Self::with_policy_and_callback(
            mode,
            SecurityPolicy::for_preset(SecurityPreset::Default),
            callback,
        )
    }

    pub fn with_policy_and_callback(
        mode: ApprovalMode,
        policy: SecurityPolicy,
        callback: impl Fn(&str) -> Result<bool, ToolError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            automatic: Arc::new(AtomicBool::new(matches!(mode, ApprovalMode::Automatic))),
            policy: Arc::new(RwLock::new(policy)),
            store: ApprovalStore::new(),
            callback: Arc::new(callback),
            context_callback: None,
            living_plan: Arc::new(Mutex::new(None)),
            plan_scratch: Arc::new(Mutex::new(None)),
            read_only_agent: Arc::new(AtomicBool::new(false)),
            goal_dir: Arc::new(Mutex::new(None)),
            session_dir: Arc::new(Mutex::new(None)),
            approval_scope: Arc::new(RwLock::new(ApprovalScope::PerAction)),
            approval_binding: Arc::new(RwLock::new(ApprovalBinding::default())),
        }
    }

    pub fn with_policy_and_context_callback(
        mode: ApprovalMode,
        policy: SecurityPolicy,
        callback: impl Fn(&ToolApprovalRequest) -> Result<bool, ToolError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            automatic: Arc::new(AtomicBool::new(matches!(mode, ApprovalMode::Automatic))),
            policy: Arc::new(RwLock::new(policy)),
            store: ApprovalStore::new(),
            callback: Arc::new(terminal_approval),
            context_callback: Some(Arc::new(callback)),
            living_plan: Arc::new(Mutex::new(None)),
            plan_scratch: Arc::new(Mutex::new(None)),
            read_only_agent: Arc::new(AtomicBool::new(false)),
            goal_dir: Arc::new(Mutex::new(None)),
            session_dir: Arc::new(Mutex::new(None)),
            approval_scope: Arc::new(RwLock::new(ApprovalScope::PerAction)),
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

    pub fn mode(&self) -> ApprovalMode {
        if self.automatic.load(Ordering::Relaxed) {
            ApprovalMode::Automatic
        } else {
            ApprovalMode::Interactive
        }
    }

    pub fn set_mode(&self, mode: ApprovalMode) {
        self.automatic
            .store(matches!(mode, ApprovalMode::Automatic), Ordering::Relaxed);
    }

    pub fn approval_scope(&self) -> ApprovalScope {
        *self.approval_scope.read().unwrap()
    }

    pub fn set_approval_scope(&self, scope: ApprovalScope) {
        *self.approval_scope.write().unwrap() = scope;
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
        match self.mode() {
            ApprovalMode::Automatic => return Ok(()),
            ApprovalMode::Interactive => {}
        }
        let scope = self.approval_scope();
        let owner = binding.owner(scope).or_else(|| {
            (scope == ApprovalScope::CurrentSession)
                .then(|| self.session_dir().map(|path| path.display().to_string()))
                .flatten()
        });
        if scope != ApprovalScope::PerAction && owner.is_none() {
            return Err(ToolError(
                "scoped approval requires a trusted Project/Session identity".to_string(),
            ));
        }
        let revision = request.workspace_revision.unwrap_or(0);
        if scope != ApprovalScope::PerAction
            && self.store.is_approved_for(
                scope,
                owner.as_deref().expect("scoped approval owner checked"),
                revision,
                &request.action,
            )
        {
            return Ok(());
        }
        let approved = match self.context_callback.as_ref() {
            Some(callback) => callback(&request)?,
            None => (self.callback)(&request.action)?,
        };
        if approved {
            if scope != ApprovalScope::PerAction {
                self.store.remember_approval_for(
                    scope,
                    owner.as_deref().expect("scoped approval owner checked"),
                    revision,
                    &request.action,
                );
            }
            Ok(())
        } else {
            Err(ToolError(format!("user denied: {}", request.action)))
        }
    }
}

fn terminal_approval(action: &str) -> Result<bool, ToolError> {
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
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}
