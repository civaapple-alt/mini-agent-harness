use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalMode {
    Interactive,
    Automatic,
}

type ApprovalCallback = dyn Fn(&str) -> Result<bool, ToolError> + Send + Sync;

#[derive(Clone)]
pub struct ApprovalController {
    automatic: Arc<AtomicBool>,
    policy: Arc<RwLock<SecurityPolicy>>,
    store: ApprovalStore,
    callback: Arc<ApprovalCallback>,
    living_plan: Arc<Mutex<Option<PathBuf>>>,
    read_only_agent: Arc<AtomicBool>,
    goal_dir: Arc<Mutex<Option<PathBuf>>>,
    session_dir: Arc<Mutex<Option<PathBuf>>>,
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
            living_plan: Arc::new(Mutex::new(None)),
            read_only_agent: Arc::new(AtomicBool::new(false)),
            goal_dir: Arc::new(Mutex::new(None)),
            session_dir: Arc::new(Mutex::new(None)),
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

    pub fn set_living_plan(&self, path: Option<PathBuf>) {
        *self.living_plan.lock().unwrap() =
            path.map(|path| crate::path_policy::normalize_path(&path));
    }

    pub fn living_plan(&self) -> Option<PathBuf> {
        self.living_plan.lock().unwrap().clone()
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
        match self.policy.read().unwrap().evaluate(action) {
            SecurityDecision::Deny => {
                return Err(ToolError(format!("forbidden by security policy: {action}")));
            }
            SecurityDecision::Allow => return Ok(()),
            SecurityDecision::Ask => {}
        }
        if self.store.is_approved(action) {
            return Ok(());
        }
        match self.mode() {
            ApprovalMode::Automatic => return Ok(()),
            ApprovalMode::Interactive => {}
        }
        if (self.callback)(action)? {
            self.store.remember_approval(action);
            Ok(())
        } else {
            Err(ToolError(format!("user denied: {action}")))
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
