use super::ToolOrchestrator;
use mini_agent_capabilities::ApprovalController;
use mini_agent_capabilities::ApprovalMode;
use mini_agent_protocol::Tool;
use mini_agent_protocol::ToolAdmission;
use mini_agent_protocol::ToolExecutionDelegate;
use mini_agent_protocol::ToolExecutionOutcome;
use mini_agent_protocol::ToolExecutionRequest;
use mini_agent_protocol::ToolSpec;
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

struct AdmittedTool {
    executed: Arc<AtomicBool>,
}

impl Tool for AdmittedTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "sensitive".to_string(),
            description: "sensitive".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    fn execute(&self, _arguments: &Value) -> Result<String, mini_agent_protocol::ToolError> {
        Ok("legacy execution".to_string())
    }

    fn admission(
        &self,
        _request: &ToolExecutionRequest,
    ) -> Result<ToolAdmission, mini_agent_protocol::ToolError> {
        Ok(ToolAdmission::ApprovalRequired {
            action: "sensitive action".to_string(),
        })
    }

    fn execute_after_admission(&self, _request: &ToolExecutionRequest) -> ToolExecutionOutcome {
        self.executed.store(true, Ordering::SeqCst);
        ToolExecutionOutcome::completed("admitted execution")
    }
}

#[test]
fn approval_precedes_admitted_execution() {
    let executed = Arc::new(AtomicBool::new(false));
    let tool = AdmittedTool {
        executed: Arc::clone(&executed),
    };
    let orchestrator = ToolOrchestrator::new(ApprovalController::with_callback(
        ApprovalMode::Interactive,
        |action| {
            assert_eq!(action, "sensitive action");
            Ok(true)
        },
    ));

    let outcome = orchestrator.execute(
        &tool,
        &ToolExecutionRequest::new("call-1", "sensitive", serde_json::json!({})),
    );

    assert_eq!(
        outcome,
        ToolExecutionOutcome::completed("admitted execution")
    );
    assert!(executed.load(Ordering::SeqCst));
}

#[test]
fn denied_admission_does_not_execute_the_tool() {
    let executed = Arc::new(AtomicBool::new(false));
    let tool = AdmittedTool {
        executed: Arc::clone(&executed),
    };
    let orchestrator = ToolOrchestrator::new(ApprovalController::with_callback(
        ApprovalMode::Interactive,
        |_| Ok(false),
    ));

    let outcome = orchestrator.execute(
        &tool,
        &ToolExecutionRequest::new("call-2", "sensitive", serde_json::json!({})),
    );

    assert_eq!(
        outcome.status,
        mini_agent_protocol::ToolExecutionStatus::NeedsApproval
    );
    assert!(outcome.content.starts_with("user denied: sensitive action"));
    assert!(!executed.load(Ordering::SeqCst));
}
