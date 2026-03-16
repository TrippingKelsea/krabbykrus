//! A2A (Agent-to-Agent) protocol implementation.
//!
//! Implements Google's A2A protocol for inter-agent communication:
//! - `/.well-known/agent.json` — Agent card discovery
//! - `POST /a2a` — JSON-RPC 2.0 dispatch for task management
//!
//! Task lifecycle: submitted → working → completed | failed | canceled

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Agent Card (/.well-known/agent.json)
// ---------------------------------------------------------------------------

/// Agent card served at `/.well-known/agent.json` for discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    pub name: String,
    pub description: String,
    pub url: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub skills: Vec<AgentSkill>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation_url: Option<String>,
    pub capabilities: AgentCapabilities,
}

/// Capabilities advertised in the agent card.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    #[serde(default)]
    pub streaming: bool,
    #[serde(default)]
    pub push_notifications: bool,
}

/// A skill (capability) that the agent can perform.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0
// ---------------------------------------------------------------------------

/// Incoming JSON-RPC 2.0 request.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// Outgoing JSON-RPC 2.0 response.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<serde_json::Value>, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// Standard JSON-RPC error codes
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
pub const TASK_NOT_FOUND: i64 = -32001;

// ---------------------------------------------------------------------------
// A2A Tasks
// ---------------------------------------------------------------------------

/// Task status in the A2A lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Submitted,
    Working,
    Completed,
    Failed,
    Canceled,
}

/// An A2A task representing a unit of work.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2ATask {
    pub id: String,
    pub agent_id: String,
    pub status: TaskStatus,
    pub messages: Vec<A2AMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub artifacts: Vec<Artifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// A message within an A2A task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2AMessage {
    pub role: String,
    pub parts: Vec<Part>,
}

/// Content parts in an A2A message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Part {
    Text {
        text: String,
    },
    File {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<String>,
    },
    Data {
        data: serde_json::Value,
    },
}

/// An artifact produced by a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parts: Vec<Part>,
}

// ---------------------------------------------------------------------------
// Task Store
// ---------------------------------------------------------------------------

/// In-memory store for A2A tasks.
pub struct TaskStore {
    tasks: RwLock<HashMap<String, A2ATask>>,
}

impl Default for TaskStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskStore {
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new task and return its ID.
    pub async fn create_task(&self, agent_id: &str, message: A2AMessage) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let task = A2ATask {
            id: id.clone(),
            agent_id: agent_id.to_string(),
            status: TaskStatus::Submitted,
            messages: vec![message],
            artifacts: Vec::new(),
            metadata: None,
        };
        self.tasks.write().await.insert(id.clone(), task);
        id
    }

    /// Get a task by ID.
    pub async fn get_task(&self, id: &str) -> Option<A2ATask> {
        self.tasks.read().await.get(id).cloned()
    }

    /// Update task status.
    pub async fn update_status(&self, id: &str, status: TaskStatus) -> bool {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(id) {
            task.status = status;
            true
        } else {
            false
        }
    }

    /// Add a message to a task.
    pub async fn add_message(&self, id: &str, message: A2AMessage) -> bool {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(id) {
            task.messages.push(message);
            true
        } else {
            false
        }
    }

    /// Add an artifact to a task.
    pub async fn add_artifact(&self, id: &str, artifact: Artifact) -> bool {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.get_mut(id) {
            task.artifacts.push(artifact);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// A2A Dispatcher
// ---------------------------------------------------------------------------

/// Dispatches JSON-RPC requests to the appropriate A2A handlers.
pub struct A2ADispatcher {
    task_store: Arc<TaskStore>,
    agent_invoker: Option<Arc<dyn rockbot_tools::AgentInvoker>>,
}

impl A2ADispatcher {
    pub fn new(task_store: Arc<TaskStore>) -> Self {
        Self {
            task_store,
            agent_invoker: None,
        }
    }

    /// Create a dispatcher with an agent invoker for processing tasks.
    pub fn with_invoker(
        task_store: Arc<TaskStore>,
        invoker: Arc<dyn rockbot_tools::AgentInvoker>,
    ) -> Self {
        Self {
            task_store,
            agent_invoker: Some(invoker),
        }
    }

    /// Dispatch a JSON-RPC request and return the response.
    pub async fn dispatch(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        match request.method.as_str() {
            "tasks/send" => self.handle_tasks_send(request.id, request.params).await,
            "tasks/get" => self.handle_tasks_get(request.id, request.params).await,
            "tasks/cancel" => self.handle_tasks_cancel(request.id, request.params).await,
            _ => JsonRpcResponse::error(
                request.id,
                METHOD_NOT_FOUND,
                format!("Method '{}' not found", request.method),
            ),
        }
    }

    async fn handle_tasks_send(
        &self,
        id: Option<serde_json::Value>,
        params: serde_json::Value,
    ) -> JsonRpcResponse {
        let agent_id = params
            .get("agentId")
            .or_else(|| params.get("agent_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("default");

        let message_text = params.get("message").and_then(|v| v.as_str()).unwrap_or("");

        let message = A2AMessage {
            role: "user".to_string(),
            parts: vec![Part::Text {
                text: message_text.to_string(),
            }],
        };

        let task_id = self.task_store.create_task(agent_id, message).await;

        self.task_store
            .update_status(&task_id, TaskStatus::Working)
            .await;

        // If we have an agent invoker, process the message synchronously
        if let Some(ref invoker) = self.agent_invoker {
            let session_id = format!("a2a:{task_id}");
            match invoker
                .invoke_agent(agent_id, message_text, &session_id, 0)
                .await
            {
                Ok(response_text) => {
                    let response_msg = A2AMessage {
                        role: "agent".to_string(),
                        parts: vec![Part::Text {
                            text: response_text,
                        }],
                    };
                    self.task_store.add_message(&task_id, response_msg).await;
                    self.task_store
                        .update_status(&task_id, TaskStatus::Completed)
                        .await;

                    let task = self.task_store.get_task(&task_id).await;
                    return JsonRpcResponse::success(
                        id,
                        serde_json::to_value(&task).unwrap_or_default(),
                    );
                }
                Err(e) => {
                    let error_msg = A2AMessage {
                        role: "agent".to_string(),
                        parts: vec![Part::Text {
                            text: format!("Error: {e}"),
                        }],
                    };
                    self.task_store.add_message(&task_id, error_msg).await;
                    self.task_store
                        .update_status(&task_id, TaskStatus::Failed)
                        .await;

                    let task = self.task_store.get_task(&task_id).await;
                    return JsonRpcResponse::success(
                        id,
                        serde_json::to_value(&task).unwrap_or_default(),
                    );
                }
            }
        }

        // No invoker — return task in working state (caller must poll tasks/get)
        JsonRpcResponse::success(
            id,
            serde_json::json!({
                "taskId": task_id,
                "status": "working",
            }),
        )
    }

    async fn handle_tasks_get(
        &self,
        id: Option<serde_json::Value>,
        params: serde_json::Value,
    ) -> JsonRpcResponse {
        let Some(task_id) = params
            .get("taskId")
            .or_else(|| params.get("task_id"))
            .and_then(|v| v.as_str())
        else {
            return JsonRpcResponse::error(id, INVALID_PARAMS, "Missing 'taskId' parameter");
        };

        match self.task_store.get_task(task_id).await {
            Some(task) => {
                JsonRpcResponse::success(id, serde_json::to_value(&task).unwrap_or_default())
            }
            None => {
                JsonRpcResponse::error(id, TASK_NOT_FOUND, format!("Task '{task_id}' not found"))
            }
        }
    }

    async fn handle_tasks_cancel(
        &self,
        id: Option<serde_json::Value>,
        params: serde_json::Value,
    ) -> JsonRpcResponse {
        let Some(task_id) = params
            .get("taskId")
            .or_else(|| params.get("task_id"))
            .and_then(|v| v.as_str())
        else {
            return JsonRpcResponse::error(id, INVALID_PARAMS, "Missing 'taskId' parameter");
        };

        if self
            .task_store
            .update_status(task_id, TaskStatus::Canceled)
            .await
        {
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "taskId": task_id,
                    "status": "canceled",
                }),
            )
        } else {
            JsonRpcResponse::error(id, TASK_NOT_FOUND, format!("Task '{task_id}' not found"))
        }
    }
}

/// Build an [`AgentCard`] from agent configuration.
pub fn build_agent_card(
    agent_id: &str,
    description: &str,
    base_url: &str,
    streaming: bool,
) -> AgentCard {
    AgentCard {
        name: agent_id.to_string(),
        description: description.to_string(),
        url: format!("{base_url}/a2a"),
        skills: Vec::new(),
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        documentation_url: None,
        capabilities: AgentCapabilities {
            streaming,
            push_notifications: false,
        },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn test_agent_card_serialization() {
        let card = build_agent_card("test-agent", "A test agent", "http://localhost:18080", true);
        let json = serde_json::to_string(&card).unwrap();
        assert!(json.contains("test-agent"));
        assert!(json.contains("\"streaming\":true"));
        assert!(json.contains("/a2a"));
    }

    #[test]
    fn test_json_rpc_response_success() {
        let resp =
            JsonRpcResponse::success(Some(serde_json::json!(1)), serde_json::json!({"ok": true}));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"result\""));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn test_json_rpc_response_error() {
        let resp =
            JsonRpcResponse::error(Some(serde_json::json!(1)), METHOD_NOT_FOUND, "not found");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("-32601"));
        assert!(json.contains("not found"));
    }

    #[tokio::test]
    async fn test_task_lifecycle() {
        let store = TaskStore::new();
        let msg = A2AMessage {
            role: "user".to_string(),
            parts: vec![Part::Text {
                text: "Hello".to_string(),
            }],
        };

        let task_id = store.create_task("agent1", msg).await;
        let task = store.get_task(&task_id).await.unwrap();
        assert_eq!(task.status, TaskStatus::Submitted);
        assert_eq!(task.messages.len(), 1);

        store.update_status(&task_id, TaskStatus::Working).await;
        let task = store.get_task(&task_id).await.unwrap();
        assert_eq!(task.status, TaskStatus::Working);

        store.update_status(&task_id, TaskStatus::Completed).await;
        let task = store.get_task(&task_id).await.unwrap();
        assert_eq!(task.status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn test_dispatcher_tasks_send() {
        let store = Arc::new(TaskStore::new());
        let dispatcher = A2ADispatcher::new(store);

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(1)),
            method: "tasks/send".to_string(),
            params: serde_json::json!({
                "agentId": "agent1",
                "message": "Do something"
            }),
        };

        let response = dispatcher.dispatch(request).await;
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert_eq!(result.get("status").unwrap().as_str().unwrap(), "working");
        assert!(result.get("taskId").is_some());
    }

    #[tokio::test]
    async fn test_dispatcher_tasks_get() {
        let store = Arc::new(TaskStore::new());
        let msg = A2AMessage {
            role: "user".to_string(),
            parts: vec![Part::Text {
                text: "Hello".to_string(),
            }],
        };
        let task_id = store.create_task("agent1", msg).await;

        let dispatcher = A2ADispatcher::new(store);
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(2)),
            method: "tasks/get".to_string(),
            params: serde_json::json!({"taskId": task_id}),
        };

        let response = dispatcher.dispatch(request).await;
        assert!(response.result.is_some());
    }

    #[tokio::test]
    async fn test_dispatcher_tasks_cancel() {
        let store = Arc::new(TaskStore::new());
        let msg = A2AMessage {
            role: "user".to_string(),
            parts: vec![Part::Text {
                text: "Hello".to_string(),
            }],
        };
        let task_id = store.create_task("agent1", msg).await;

        let dispatcher = A2ADispatcher::new(store);
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(3)),
            method: "tasks/cancel".to_string(),
            params: serde_json::json!({"taskId": task_id}),
        };

        let response = dispatcher.dispatch(request).await;
        assert!(response.result.is_some());
        let result = response.result.unwrap();
        assert_eq!(result.get("status").unwrap().as_str().unwrap(), "canceled");
    }

    #[tokio::test]
    async fn test_dispatcher_method_not_found() {
        let store = Arc::new(TaskStore::new());
        let dispatcher = A2ADispatcher::new(store);

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(4)),
            method: "unknown/method".to_string(),
            params: serde_json::json!({}),
        };

        let response = dispatcher.dispatch(request).await;
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_task_not_found() {
        let store = Arc::new(TaskStore::new());
        let dispatcher = A2ADispatcher::new(store);

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(serde_json::json!(5)),
            method: "tasks/get".to_string(),
            params: serde_json::json!({"taskId": "nonexistent"}),
        };

        let response = dispatcher.dispatch(request).await;
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, TASK_NOT_FOUND);
    }

    #[test]
    fn test_part_serialization() {
        let text = Part::Text {
            text: "hello".to_string(),
        };
        let json = serde_json::to_string(&text).unwrap();
        assert!(json.contains("\"type\":\"text\""));

        let data = Part::Data {
            data: serde_json::json!({"key": "value"}),
        };
        let json = serde_json::to_string(&data).unwrap();
        assert!(json.contains("\"type\":\"data\""));
    }
}
