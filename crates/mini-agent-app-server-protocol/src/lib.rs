//! Versioned client/server contracts for exposing a mini-agent Thread.
//!
//! These types are deliberately separate from the transport-neutral core
//! protocol. They describe JSON-RPC method names, request correlation, and
//! server notifications; they do not define Harness execution semantics.

use mini_agent_protocol::EventEnvelope;
use mini_agent_protocol::Message;
use mini_agent_protocol::StopReason;
use mini_agent_protocol::ThreadId;
use mini_agent_protocol::ThreadStatus;
use mini_agent_protocol::TurnId;
use mini_agent_protocol::TurnInput;
use mini_agent_protocol::TurnStatus;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

pub const JSONRPC_VERSION: &str = "2.0";
pub const PROTOCOL_VERSION: u32 = 1;

pub const METHOD_INITIALIZE: &str = "initialize";
pub const METHOD_INITIALIZED: &str = "initialized";
pub const METHOD_THREAD_START: &str = "thread/start";
pub const METHOD_THREAD_LIST: &str = "thread/list";
pub const METHOD_THREAD_FORK: &str = "thread/fork";
pub const METHOD_THREAD_RESUME: &str = "thread/resume";
pub const METHOD_THREAD_READ: &str = "thread/read";
pub const METHOD_THREAD_CLOSE: &str = "thread/close";
pub const METHOD_TURN_START: &str = "turn/start";
pub const METHOD_TURN_READ: &str = "turn/read";
pub const METHOD_TURN_STEER: &str = "turn/steer";
pub const METHOD_TURN_INTERRUPT: &str = "turn/interrupt";
pub const METHOD_TURN_EVENT: &str = "turn/event";
pub const METHOD_APPROVAL_REQUEST: &str = "approval/request";
pub const METHOD_APPROVAL_RESPOND: &str = "approval/respond";

/// A JSON-RPC request or notification received by the app-server.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JsonRpcRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jsonrpc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn request(id: impl Into<Value>, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: Some(JSONRPC_VERSION.to_string()),
            id: Some(id.into()),
            method: method.into(),
            params: Some(params),
        }
    }

    pub fn notification(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: Some(JSONRPC_VERSION.to_string()),
            id: None,
            method: method.into(),
            params,
        }
    }

    pub fn decode_params<T: for<'de> Deserialize<'de>>(&self) -> Result<T, JsonRpcError> {
        self.params
            .clone()
            .ok_or_else(|| JsonRpcError::invalid_params("params are required"))
            .and_then(|params| {
                serde_json::from_value(params)
                    .map_err(|error| JsonRpcError::invalid_params(error.to_string()))
            })
    }
}

/// A JSON-RPC response with either a result or an error.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn result(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<Value>, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// JSON-RPC error object returned by the app-server.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::new(-32700, message)
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(-32600, message)
    }

    pub fn method_not_found(message: impl Into<String>) -> Self {
        Self::new(-32601, message)
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message)
    }

    pub fn server_error(message: impl Into<String>) -> Self {
        Self::new(-32000, message)
    }

    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    #[serde(default)]
    pub approvals: bool,
    #[serde(default)]
    pub notifications: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: u32,
    pub client_name: String,
    pub client_version: String,
    #[serde(default)]
    pub capabilities: ClientCapabilities,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    #[serde(default)]
    pub approvals: bool,
    #[serde(default)]
    pub steering: bool,
    #[serde(default)]
    pub thread_resume: bool,
    #[serde(default)]
    pub thread_fork: bool,
    #[serde(default)]
    pub thread_read: bool,
    #[serde(default)]
    pub thread_close: bool,
    #[serde(default)]
    pub turn_read: bool,
    #[serde(default)]
    pub thread_list: bool,
    #[serde(default)]
    pub approval_requests: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: u32,
    pub server_name: String,
    pub server_version: String,
    pub capabilities: ServerCapabilities,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<ThreadId>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartResult {
    pub thread_id: ThreadId,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListParams {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadListResult {
    pub data: Vec<ThreadId>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkParams {
    pub source_thread_id: ThreadId,
    pub new_thread_id: ThreadId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadForkResult {
    pub thread_id: ThreadId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeParams {
    pub thread_id: ThreadId,
    pub checkpoint: ThreadReadResult,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeResult {
    pub thread_id: ThreadId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadParams {
    pub thread_id: ThreadId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadResult {
    pub thread_id: ThreadId,
    pub status: ThreadStatus,
    pub messages: Vec<Message>,
    pub context_revision: u64,
    pub next_turn_number: u64,
    pub last_turn_id: Option<TurnId>,
    pub next_event_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadCloseParams {
    pub thread_id: ThreadId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    pub thread_id: ThreadId,
    pub input: TurnInput,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartResult {
    pub turn_id: TurnId,
    pub status: TurnStatus,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnReadParams {
    pub turn_id: TurnId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnReadResult {
    pub turn_id: TurnId,
    pub status: TurnStatus,
    pub stop_reason: Option<StopReason>,
    pub final_text: Option<String>,
    pub steps: usize,
    pub messages: Vec<Message>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerParams {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequestNotification {
    pub request_id: String,
    pub action: String,
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRespondParams {
    pub request_id: String,
    pub approved: bool,
    #[serde(default)]
    pub remember: bool,
}

/// A server-to-client notification carrying an ordered core event.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnEventNotification {
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
    pub sequence: u64,
    pub event: mini_agent_protocol::Event,
}

impl From<EventEnvelope> for TurnEventNotification {
    fn from(event: EventEnvelope) -> Self {
        Self {
            thread_id: event.thread_id,
            turn_id: event.turn_id,
            sequence: event.sequence,
            event: event.event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_agent_protocol::Event;

    #[test]
    fn request_and_response_round_trip() {
        let request = JsonRpcRequest::request(
            7,
            METHOD_TURN_START,
            serde_json::json!({"thread_id": "thread-1"}),
        );
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: JsonRpcRequest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, request);

        let response =
            JsonRpcResponse::result(Some(serde_json::json!(7)), serde_json::json!({"ok": true}));
        let decoded: JsonRpcResponse =
            serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
        assert_eq!(decoded, response);
    }

    #[test]
    fn notifications_omit_request_id() {
        let request = JsonRpcRequest::notification(METHOD_INITIALIZED, None);
        let value = serde_json::to_value(request).unwrap();
        assert!(value.get("id").is_none());
    }

    #[test]
    fn app_server_params_use_camel_case() {
        let value = serde_json::to_value(InitializeParams {
            protocol_version: PROTOCOL_VERSION,
            client_name: "test".to_string(),
            client_version: "0".to_string(),
            capabilities: ClientCapabilities::default(),
        })
        .unwrap();
        assert!(value.get("protocolVersion").is_some());
        assert!(value.get("protocol_version").is_none());
    }

    #[test]
    fn event_projection_preserves_identity_and_sequence() {
        let envelope = EventEnvelope::new(
            ThreadId::new("thread-1"),
            Some(TurnId::new("turn-1")),
            4,
            Event::RunFinished {
                stop_reason: mini_agent_protocol::StopReason::Completed,
                steps: 2,
            },
        );
        let notification = TurnEventNotification::from(envelope);
        assert_eq!(notification.thread_id, ThreadId::new("thread-1"));
        assert_eq!(notification.turn_id, Some(TurnId::new("turn-1")));
        assert_eq!(notification.sequence, 4);
    }
}
