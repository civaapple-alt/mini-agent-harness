//! Deterministic provider-free App Server smoke path used by the CLI.

use crate::AppServer;
use crate::AppServerConnection;
use crate::LocalAppServerClient;
use mini_agent_core::Event;
use mini_agent_core::EventSink;
use mini_agent_core::Harness;
use mini_agent_core::HarnessConfig;
use mini_agent_core::Message;
use mini_agent_core::Model;
use mini_agent_core::ModelEventSink;
use mini_agent_core::ModelRequest;
use mini_agent_core::ModelResponse;
use mini_agent_core::Thread;
use mini_agent_core::ThreadId;
use mini_agent_core::ThreadStart;
use mini_agent_core::Tool;
use mini_agent_core::ToolCall;
use mini_agent_core::ToolError;
use mini_agent_core::ToolRegistry;
use mini_agent_core::ToolSpec;
use mini_agent_core::TurnInput;
use mini_agent_core::TurnInputMode;
use mini_agent_core::TurnStatus;
use mini_agent_host::RunObserver;
use serde_json::Value;
use serde_json::json;
use std::convert::Infallible;

pub async fn run(prompt: String) -> Result<(), String> {
    let thread_id = ThreadId::new("demo");
    let thread = Thread::new(
        thread_id.clone(),
        Harness::new(
            DemoModel { turn: 0 },
            ToolRegistry::new(vec![Box::new(Uppercase)]),
            HarnessConfig::default(),
        ),
    );
    let server = AppServer::new(ThreadStart::new(thread_id.clone()), thread);
    let mut client = LocalAppServerClient::new(AppServerConnection::new(server));
    client
        .initialize("mini-agent-demo", env!("CARGO_PKG_VERSION"))
        .await
        .map_err(|error| format!("cannot initialize app server: {}", error.message))?;
    let mut observer = RunObserver::new();
    let turn_id = match client
        .start_turn(thread_id, TurnInput::new(TurnInputMode::Start, prompt))
        .await
        .map_err(|error| format!("cannot start turn: {}", error.message))?
    {
        mini_agent_core::TurnSubmission::Started { turn_id } => turn_id,
        other => return Err(format!("turn was not started: {other:?}")),
    };
    loop {
        let event = client
            .next_event()
            .await
            .map_err(|error| format!("event stream failed: {}", error.message))?;
        let finished = matches!(event.event, Event::TurnFinished { .. });
        observer.emit(event);
        if finished {
            break;
        }
    }
    observer.finish();
    let result = client
        .read_turn(turn_id)
        .await
        .map_err(|error| format!("cannot read demo turn: {}", error.message))?;
    if result.status == TurnStatus::Completed {
        Ok(())
    } else {
        Err(format!("demo turn ended with {:?}", result.status))
    }
}

struct DemoModel {
    turn: usize,
}

impl Model for DemoModel {
    type Error = Infallible;

    async fn respond<'a>(
        &'a mut self,
        request: ModelRequest<'a>,
        _events: &'a mut (dyn ModelEventSink + Send),
    ) -> Result<ModelResponse, Self::Error> {
        self.turn += 1;
        if self.turn == 1 {
            let prompt = request
                .messages
                .iter()
                .find_map(|message| match message {
                    Message::User { text } => Some(text.as_str()),
                    Message::Context { .. } | Message::Assistant { .. } | Message::Tool { .. } => {
                        None
                    }
                })
                .unwrap_or_default();
            return Ok(ModelResponse {
                reasoning: String::new(),
                text: "I will run one tool.".to_string(),
                tool_calls: vec![ToolCall {
                    id: "demo-call".to_string(),
                    name: "uppercase".to_string(),
                    arguments: json!({ "text": prompt }),
                }],
                usage: None,
            });
        }
        let result = request
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::Tool { content, .. } => Some(content.as_str()),
                Message::Context { .. } | Message::User { .. } | Message::Assistant { .. } => None,
            })
            .unwrap_or("no tool result");
        Ok(ModelResponse {
            reasoning: String::new(),
            text: format!("The tool returned: {result}"),
            tool_calls: Vec::new(),
            usage: None,
        })
    }
}

struct Uppercase;

impl Tool for Uppercase {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "uppercase".to_string(),
            description: "Convert text to uppercase".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"],
                "additionalProperties": false
            }),
        }
    }

    fn execute(&self, arguments: &Value) -> Result<String, ToolError> {
        let text = arguments
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError("text must be a string".to_string()))?;
        Ok(text.to_ascii_uppercase())
    }
}
