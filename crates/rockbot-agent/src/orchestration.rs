//! Multi-agent orchestration: Swarm Blackboard and Graph Workflow execution.
//!
//! This module provides two coordination primitives:
//!
//! - **SwarmBlackboard**: In-memory shared state for swarm-style agent teams.
//!   Agents in the same swarm can read/write key-value pairs via blackboard tools.
//!
//! - **WorkflowExecutor**: DAG-based workflow execution. Nodes map to agents,
//!   edges encode data flow and conditional routing. Supports parallel fan-out
//!   for independent nodes in the same topological layer.

use rockbot_config::{EdgeCondition, WorkflowDefinition, WorkflowEdge, WorkflowNode};
use rockbot_tools::{AgentInvoker, BlackboardAccessor};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Swarm Blackboard
// ---------------------------------------------------------------------------

/// In-memory blackboard implementation for swarm coordination.
///
/// Keyed by `(swarm_id, entry_key)`. Thread-safe via `RwLock`.
#[derive(Debug, Clone)]
pub struct SwarmBlackboard {
    data: Arc<RwLock<HashMap<String, HashMap<String, serde_json::Value>>>>,
}

impl SwarmBlackboard {
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for SwarmBlackboard {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl BlackboardAccessor for SwarmBlackboard {
    async fn read(&self, swarm_id: &str, key: &str) -> Option<serde_json::Value> {
        let data = self.data.read().await;
        data.get(swarm_id)
            .and_then(|entries| entries.get(key))
            .cloned()
    }

    async fn write(&self, swarm_id: &str, key: &str, value: serde_json::Value) {
        let mut data = self.data.write().await;
        data.entry(swarm_id.to_string())
            .or_default()
            .insert(key.to_string(), value);
    }

    async fn read_all(&self, swarm_id: &str) -> HashMap<String, serde_json::Value> {
        let data = self.data.read().await;
        data.get(swarm_id).cloned().unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Workflow progress events
// ---------------------------------------------------------------------------

/// Progress events emitted during workflow execution.
#[derive(Debug, Clone)]
pub enum WorkflowProgressEvent {
    /// A workflow node is about to execute
    NodeStarted { node_id: String, agent_id: String },
    /// A workflow node completed successfully
    NodeCompleted {
        node_id: String,
        output_preview: String,
    },
    /// A workflow node failed
    NodeFailed { node_id: String, error: String },
}

// ---------------------------------------------------------------------------
// Workflow Executor
// ---------------------------------------------------------------------------

/// Executes a workflow DAG by invoking agents for each node.
///
/// Uses Kahn's algorithm for topological ordering and runs nodes in the same
/// layer concurrently via `tokio::task::JoinSet`.
pub struct WorkflowExecutor {
    invoker: Arc<dyn AgentInvoker>,
}

impl WorkflowExecutor {
    pub fn new(invoker: Arc<dyn AgentInvoker>) -> Self {
        Self { invoker }
    }

    /// Execute a workflow with the given input.
    ///
    /// Returns the concatenated output of all exit nodes.
    pub async fn execute(
        &self,
        workflow: &WorkflowDefinition,
        input: &str,
        session_id: &str,
        progress_tx: Option<tokio::sync::mpsc::UnboundedSender<WorkflowProgressEvent>>,
    ) -> Result<String, String> {
        // Validate the workflow
        self.validate(workflow)?;

        // Build adjacency structures
        let mut in_edges: HashMap<String, Vec<&WorkflowEdge>> = HashMap::new();
        let mut out_edges: HashMap<String, Vec<&WorkflowEdge>> = HashMap::new();
        for node in &workflow.nodes {
            in_edges.entry(node.id.clone()).or_default();
            out_edges.entry(node.id.clone()).or_default();
        }
        for edge in &workflow.edges {
            in_edges.entry(edge.to.clone()).or_default().push(edge);
            out_edges.entry(edge.from.clone()).or_default().push(edge);
        }

        // Track node outputs and completion
        let node_outputs: Arc<RwLock<HashMap<String, String>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let completed: Arc<RwLock<std::collections::HashSet<String>>> =
            Arc::new(RwLock::new(std::collections::HashSet::new()));

        // Topological execution via Kahn's algorithm
        // Start with entry nodes (nodes with no incoming edges from non-entry sources)
        let mut ready: Vec<String> = workflow.entry_nodes.clone();
        let mut executed_count = 0;
        let total_nodes = workflow.nodes.len();

        while !ready.is_empty() {
            // Execute all ready nodes concurrently
            let mut join_set = tokio::task::JoinSet::new();

            for node_id in ready.drain(..) {
                let node = workflow
                    .nodes
                    .iter()
                    .find(|n| n.id == node_id)
                    .ok_or_else(|| format!("Workflow node '{node_id}' not found"))?
                    .clone();

                let invoker = Arc::clone(&self.invoker);
                let outputs = Arc::clone(&node_outputs);
                let input = input.to_string();
                let session_id = session_id.to_string();
                let ptx = progress_tx.clone();

                join_set.spawn(async move {
                    // Notify progress
                    if let Some(ref tx) = ptx {
                        let _ = tx.send(WorkflowProgressEvent::NodeStarted {
                            node_id: node.id.clone(),
                            agent_id: node.agent_id.clone(),
                        });
                    }

                    // Build the message from template
                    let message = Self::build_node_message(&node, &input, &outputs).await;

                    debug!(
                        "Workflow node '{}' invoking agent '{}' with message: {}",
                        node.id,
                        node.agent_id,
                        if message.len() > 100 {
                            &message[..100]
                        } else {
                            &message
                        }
                    );

                    // Invoke the agent
                    match invoker
                        .invoke_agent(
                            &node.agent_id,
                            &message,
                            &session_id,
                            1, // depth=1 for workflow nodes
                        )
                        .await
                    {
                        Ok(output) => {
                            if let Some(ref tx) = ptx {
                                let preview = if output.len() > 200 {
                                    format!("{}...", &output[..200])
                                } else {
                                    output.clone()
                                };
                                let _ = tx.send(WorkflowProgressEvent::NodeCompleted {
                                    node_id: node.id.clone(),
                                    output_preview: preview,
                                });
                            }
                            Ok((node.id, output))
                        }
                        Err(e) => {
                            let error_msg = format!("{e}");
                            if let Some(ref tx) = ptx {
                                let _ = tx.send(WorkflowProgressEvent::NodeFailed {
                                    node_id: node.id.clone(),
                                    error: error_msg.clone(),
                                });
                            }
                            Err((node.id, error_msg))
                        }
                    }
                });
            }

            // Collect results from this layer
            while let Some(result) = join_set.join_next().await {
                match result {
                    Ok(Ok((node_id, output))) => {
                        info!("Workflow node '{}' completed", node_id);
                        node_outputs.write().await.insert(node_id.clone(), output);
                        completed.write().await.insert(node_id);
                        executed_count += 1;
                    }
                    Ok(Err((node_id, error))) => {
                        warn!("Workflow node '{}' failed: {}", node_id, error);
                        // Store error as output so downstream nodes can see it
                        node_outputs
                            .write()
                            .await
                            .insert(node_id.clone(), format!("[ERROR: {error}]"));
                        completed.write().await.insert(node_id);
                        executed_count += 1;
                    }
                    Err(join_error) => {
                        warn!("Workflow task panicked: {join_error}");
                        executed_count += 1;
                    }
                }
            }

            // Determine next ready nodes: nodes whose all dependencies are satisfied
            // and whose incoming edge conditions are met
            let completed_set = completed.read().await;
            let outputs = node_outputs.read().await;

            for node in &workflow.nodes {
                if completed_set.contains(&node.id) {
                    continue;
                }

                // Check if all incoming edges have their source completed AND condition met
                let incoming = in_edges
                    .get(&node.id)
                    .map_or(&[] as &[&WorkflowEdge], |v| v.as_slice());
                if incoming.is_empty() {
                    // Entry node with no incoming edges — should have been in initial ready set
                    continue;
                }

                let all_sources_done = incoming
                    .iter()
                    .all(|edge| completed_set.contains(&edge.from));
                if !all_sources_done {
                    continue;
                }

                // Check edge conditions
                let any_condition_met = incoming.iter().any(|edge| {
                    let source_output = outputs.get(&edge.from).map_or("", String::as_str);
                    Self::check_condition(&edge.condition, source_output)
                });

                if any_condition_met {
                    ready.push(node.id.clone());
                }
            }

            // Safety: prevent infinite loop if no progress is made
            if ready.is_empty() && executed_count < total_nodes {
                // Some nodes may be unreachable due to unmet conditions — that's OK
                debug!(
                    "Workflow: {} of {} nodes executed, remaining nodes have unmet conditions",
                    executed_count, total_nodes
                );
                break;
            }
        }

        // Collect exit node outputs
        let outputs = node_outputs.read().await;
        let exit_nodes = if workflow.exit_nodes.is_empty() {
            // If no exit nodes specified, use all leaf nodes (no outgoing edges)
            workflow
                .nodes
                .iter()
                .filter(|n| out_edges.get(&n.id).is_none_or(|e| e.is_empty()))
                .map(|n| n.id.clone())
                .collect::<Vec<_>>()
        } else {
            workflow.exit_nodes.clone()
        };

        let final_output: Vec<String> = exit_nodes
            .iter()
            .filter_map(|id| outputs.get(id).cloned())
            .collect();

        if final_output.is_empty() {
            Err("Workflow produced no output — no exit nodes completed".to_string())
        } else {
            Ok(final_output.join("\n\n---\n\n"))
        }
    }

    /// Validate the workflow definition (cycle detection via Kahn's algorithm).
    #[allow(clippy::unused_self)]
    fn validate(&self, workflow: &WorkflowDefinition) -> Result<(), String> {
        if workflow.nodes.is_empty() {
            return Err("Workflow has no nodes".to_string());
        }
        if workflow.entry_nodes.is_empty() {
            return Err("Workflow has no entry nodes".to_string());
        }

        // Check all referenced node IDs exist
        let node_ids: std::collections::HashSet<&str> =
            workflow.nodes.iter().map(|n| n.id.as_str()).collect();

        for entry in &workflow.entry_nodes {
            if !node_ids.contains(entry.as_str()) {
                return Err(format!("Entry node '{entry}' not found in workflow nodes"));
            }
        }
        for exit in &workflow.exit_nodes {
            if !node_ids.contains(exit.as_str()) {
                return Err(format!("Exit node '{exit}' not found in workflow nodes"));
            }
        }
        for edge in &workflow.edges {
            if !node_ids.contains(edge.from.as_str()) {
                return Err(format!(
                    "Edge source '{}' not found in workflow nodes",
                    edge.from
                ));
            }
            if !node_ids.contains(edge.to.as_str()) {
                return Err(format!(
                    "Edge target '{}' not found in workflow nodes",
                    edge.to
                ));
            }
        }

        // Cycle detection via Kahn's algorithm
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for node in &workflow.nodes {
            in_degree.insert(&node.id, 0);
        }
        for edge in &workflow.edges {
            *in_degree.entry(&edge.to).or_insert(0) += 1;
        }

        let mut queue: Vec<&str> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();
        let mut visited = 0;

        while let Some(node_id) = queue.pop() {
            visited += 1;
            for edge in &workflow.edges {
                if edge.from == node_id {
                    if let Some(deg) = in_degree.get_mut(edge.to.as_str()) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push(&edge.to);
                        }
                    }
                }
            }
        }

        if visited != workflow.nodes.len() {
            return Err("Workflow contains a cycle".to_string());
        }

        Ok(())
    }

    /// Build the message for a workflow node by expanding its template.
    async fn build_node_message(
        node: &WorkflowNode,
        input: &str,
        outputs: &Arc<RwLock<HashMap<String, String>>>,
    ) -> String {
        match &node.message_template {
            Some(template) => {
                let message = template.replace("{input}", input);

                // Replace {output:node_id} placeholders
                let outputs = outputs.read().await;
                // Find all {output:xxx} patterns
                let mut result = String::with_capacity(message.len());
                let mut remaining = message.as_str();
                while let Some(start) = remaining.find("{output:") {
                    result.push_str(&remaining[..start]);
                    let after = &remaining[start + 8..]; // skip "{output:"
                    if let Some(end) = after.find('}') {
                        let ref_id = &after[..end];
                        let replacement = outputs.get(ref_id).map_or("[no output]", String::as_str);
                        result.push_str(replacement);
                        remaining = &after[end + 1..];
                    } else {
                        // Malformed placeholder — keep literal
                        result.push_str("{output:");
                        remaining = after;
                    }
                }
                result.push_str(remaining);
                result
            }
            None => input.to_string(),
        }
    }

    /// Check if an edge condition is satisfied by the source node's output.
    fn check_condition(condition: &EdgeCondition, output: &str) -> bool {
        match condition {
            EdgeCondition::Always => true,
            EdgeCondition::Contains { keyword } => {
                output.to_lowercase().contains(&keyword.to_lowercase())
            }
            EdgeCondition::Pattern { regex } => match regex::Regex::new(regex) {
                Ok(re) => re.is_match(output),
                Err(e) => {
                    warn!("Invalid edge condition regex '{}': {}", regex, e);
                    false
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[tokio::test]
    async fn test_blackboard_read_write() {
        let bb = SwarmBlackboard::new();
        bb.write("swarm1", "task", serde_json::json!("research"))
            .await;
        bb.write("swarm1", "status", serde_json::json!("in_progress"))
            .await;

        assert_eq!(
            bb.read("swarm1", "task").await,
            Some(serde_json::json!("research"))
        );
        assert_eq!(bb.read("swarm1", "missing").await, None);
        assert_eq!(bb.read("other_swarm", "task").await, None);

        let all = bb.read_all("swarm1").await;
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_workflow_validation_empty() {
        let executor = WorkflowExecutor::new(Arc::new(MockInvoker));
        let wf = WorkflowDefinition {
            nodes: vec![],
            edges: vec![],
            entry_nodes: vec![],
            exit_nodes: vec![],
        };
        assert!(executor.validate(&wf).is_err());
    }

    #[test]
    fn test_workflow_validation_cycle() {
        let executor = WorkflowExecutor::new(Arc::new(MockInvoker));
        let wf = WorkflowDefinition {
            nodes: vec![
                WorkflowNode {
                    id: "a".into(),
                    agent_id: "a1".into(),
                    message_template: None,
                },
                WorkflowNode {
                    id: "b".into(),
                    agent_id: "b1".into(),
                    message_template: None,
                },
            ],
            edges: vec![
                WorkflowEdge {
                    from: "a".into(),
                    to: "b".into(),
                    condition: EdgeCondition::Always,
                },
                WorkflowEdge {
                    from: "b".into(),
                    to: "a".into(),
                    condition: EdgeCondition::Always,
                },
            ],
            entry_nodes: vec!["a".into()],
            exit_nodes: vec!["b".into()],
        };
        let result = executor.validate(&wf);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cycle"));
    }

    #[test]
    fn test_workflow_validation_valid() {
        let executor = WorkflowExecutor::new(Arc::new(MockInvoker));
        let wf = WorkflowDefinition {
            nodes: vec![
                WorkflowNode {
                    id: "a".into(),
                    agent_id: "a1".into(),
                    message_template: None,
                },
                WorkflowNode {
                    id: "b".into(),
                    agent_id: "b1".into(),
                    message_template: None,
                },
                WorkflowNode {
                    id: "c".into(),
                    agent_id: "c1".into(),
                    message_template: None,
                },
            ],
            edges: vec![
                WorkflowEdge {
                    from: "a".into(),
                    to: "c".into(),
                    condition: EdgeCondition::Always,
                },
                WorkflowEdge {
                    from: "b".into(),
                    to: "c".into(),
                    condition: EdgeCondition::Always,
                },
            ],
            entry_nodes: vec!["a".into(), "b".into()],
            exit_nodes: vec!["c".into()],
        };
        assert!(executor.validate(&wf).is_ok());
    }

    #[test]
    fn test_edge_condition_contains() {
        assert!(WorkflowExecutor::check_condition(
            &EdgeCondition::Contains {
                keyword: "error".into()
            },
            "There was an Error in processing"
        ));
        assert!(!WorkflowExecutor::check_condition(
            &EdgeCondition::Contains {
                keyword: "error".into()
            },
            "All good"
        ));
    }

    #[test]
    fn test_edge_condition_pattern() {
        assert!(WorkflowExecutor::check_condition(
            &EdgeCondition::Pattern {
                regex: r"\d{3}".into()
            },
            "Status code 404 returned"
        ));
        assert!(!WorkflowExecutor::check_condition(
            &EdgeCondition::Pattern {
                regex: r"\d{3}".into()
            },
            "No numbers here"
        ));
    }

    #[tokio::test]
    async fn test_workflow_linear_execution() {
        let executor = WorkflowExecutor::new(Arc::new(MockInvoker));
        let wf = WorkflowDefinition {
            nodes: vec![
                WorkflowNode {
                    id: "step1".into(),
                    agent_id: "echo".into(),
                    message_template: Some("Step1: {input}".into()),
                },
                WorkflowNode {
                    id: "step2".into(),
                    agent_id: "echo".into(),
                    message_template: Some("Step2: {output:step1}".into()),
                },
            ],
            edges: vec![WorkflowEdge {
                from: "step1".into(),
                to: "step2".into(),
                condition: EdgeCondition::Always,
            }],
            entry_nodes: vec!["step1".into()],
            exit_nodes: vec!["step2".into()],
        };

        let result = executor.execute(&wf, "hello", "test-session", None).await;
        assert!(result.is_ok());
        let output = result.unwrap();
        // MockInvoker echoes the message back
        assert!(output.contains("Step2:"));
    }

    /// Mock invoker that echoes the message back
    struct MockInvoker;

    #[async_trait::async_trait]
    impl AgentInvoker for MockInvoker {
        async fn invoke_agent(
            &self,
            _agent_id: &str,
            message: &str,
            _session_id: &str,
            _depth: u32,
        ) -> std::result::Result<String, rockbot_tools::ToolError> {
            Ok(format!("echo: {message}"))
        }
    }
}
