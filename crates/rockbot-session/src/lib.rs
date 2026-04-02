//! Session management for RockBot backed by redb.
//!
//! This module provides session tracking, message history management, and
//! persistent storage for agent conversations.

use chrono::{DateTime, Utc};
use rockbot_config::{Message, SessionError};
use rockbot_storage::{tables, Store};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{debug, info};
use uuid::Uuid;

/// Errors from session operations
#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Session(#[from] SessionError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Storage error: {0}")]
    Storage(#[from] anyhow::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Session identifier
pub type SessionId = String;

/// A conversation session with an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub agent_id: String,
    pub session_key: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub token_stats: TokenStats,
    #[serde(default)]
    pub memory: SessionMemory,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    pub state: SessionState,
}

/// Token usage statistics for a session
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenStats {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

/// Session-level working memory used to keep long-running autonomous work coherent.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionMemory {
    /// Rolling summary of the current task state.
    pub working_summary: String,
    /// Latest user intent or task request.
    pub last_user_intent: Option<String>,
    /// Recently used tools, newest last.
    pub recent_tools: Vec<String>,
    /// Last context budget snapshot recorded before an LLM call.
    pub context_budget: Option<ContextBudgetSnapshot>,
}

/// Snapshot of estimated request budget for the active session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBudgetSnapshot {
    pub estimated_input_tokens: u64,
    pub max_context_tokens: u64,
    pub utilization_percent: u8,
    pub status: ContextBudgetStatus,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextBudgetStatus {
    Normal,
    Warning,
    Critical,
}

/// Current state of a session
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    #[default]
    Active,
    Paused,
    Archived,
    Error {
        message: String,
    },
}

/// Session manager handles multiple concurrent sessions
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<SessionId, Session>>>,
    store: Arc<Store>,
    max_sessions: usize,
}

/// Session query parameters for searching/filtering
#[derive(Debug, Default)]
pub struct SessionQuery {
    pub agent_id: Option<String>,
    pub session_key: Option<String>,
    pub state: Option<SessionState>,
    pub exclude_archived: bool,
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// Message history for a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageHistory {
    pub messages: Vec<StoredMessage>,
    pub total_count: usize,
}

/// A message stored in the database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: i64,
    pub session_id: SessionId,
    pub message: Message,
}

fn session_messages_prefix(session_id: &str) -> String {
    format!("{session_id}\0")
}

fn session_messages_range(session_id: &str) -> (String, String) {
    (
        session_messages_prefix(session_id),
        format!("{session_id}\x01"),
    )
}

fn session_message_key(session_id: &str, message: &Message) -> String {
    format!(
        "{}{:020}\0{}",
        session_messages_prefix(session_id),
        message.created_at.timestamp_millis(),
        message.id
    )
}

impl Session {
    pub fn new<S1, S2>(agent_id: S1, session_key: S2) -> Self
    where
        S1: Into<String>,
        S2: Into<String>,
    {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            agent_id: agent_id.into(),
            session_key: session_key.into(),
            created_at: now,
            updated_at: now,
            token_stats: TokenStats::default(),
            memory: SessionMemory::default(),
            metadata: HashMap::new(),
            state: SessionState::Active,
        }
    }

    pub fn add_tokens(&mut self, input: u64, output: u64) {
        self.token_stats.input_tokens += input;
        self.token_stats.output_tokens += output;
        self.token_stats.total_tokens += input + output;
        self.updated_at = Utc::now();
    }

    pub fn set_state(&mut self, state: SessionState) {
        self.state = state;
        self.updated_at = Utc::now();
    }

    pub fn set_metadata<K, V>(&mut self, key: K, value: V)
    where
        K: Into<String>,
        V: Serialize,
    {
        self.metadata.insert(
            key.into(),
            serde_json::to_value(value).unwrap_or(serde_json::Value::Null),
        );
        self.updated_at = Utc::now();
    }

    pub fn update_working_memory(
        &mut self,
        working_summary: String,
        last_user_intent: Option<String>,
        recent_tools: Vec<String>,
    ) {
        self.memory.working_summary = working_summary;
        self.memory.last_user_intent = last_user_intent;
        self.memory.recent_tools = recent_tools;
        self.updated_at = Utc::now();
    }

    pub fn update_context_budget(&mut self, estimated_input_tokens: u64, max_context_tokens: u64) {
        let utilization_percent = if max_context_tokens == 0 {
            0
        } else {
            ((estimated_input_tokens.saturating_mul(100)) / max_context_tokens).min(100) as u8
        };
        let status = if utilization_percent >= 90 {
            ContextBudgetStatus::Critical
        } else if utilization_percent >= 75 {
            ContextBudgetStatus::Warning
        } else {
            ContextBudgetStatus::Normal
        };

        self.memory.context_budget = Some(ContextBudgetSnapshot {
            estimated_input_tokens,
            max_context_tokens,
            utilization_percent,
            status,
            updated_at: Utc::now(),
        });
        self.updated_at = Utc::now();
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state, SessionState::Active)
    }
}

impl SessionManager {
    pub async fn new<P: AsRef<Path>>(db_path: P, max_sessions: usize) -> Result<Self> {
        Self::new_with_key(db_path, max_sessions, None).await
    }

    pub async fn new_with_key<P: AsRef<Path>>(
        db_path: P,
        max_sessions: usize,
        key: Option<[u8; 32]>,
    ) -> Result<Self> {
        let path = db_path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let encrypted = key.is_some();
        let store = Arc::new(Store::open_with_optional_key(path, key)?);
        info!(
            "Session manager initialized with {} redb store at {:?}",
            if encrypted { "encrypted" } else { "plaintext" },
            path
        );
        Ok(Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            store,
            max_sessions,
        })
    }

    pub async fn new_with_store(
        store: Arc<Store>,
        max_sessions: usize,
        descriptor: &str,
    ) -> Result<Self> {
        info!("Session manager initialized with {descriptor}");
        Ok(Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            store,
            max_sessions,
        })
    }

    fn load_all_sessions(&self) -> Result<Vec<Session>> {
        self.store
            .list(tables::SESSIONS)?
            .into_iter()
            .map(|(_, bytes)| serde_json::from_slice(&bytes).map_err(Into::into))
            .collect()
    }

    fn persist_session(&self, session: &Session) -> Result<()> {
        let bytes = serde_json::to_vec(session)?;
        self.store.put(tables::SESSIONS, &session.id, &bytes)?;
        Ok(())
    }

    pub async fn create_session<S1, S2>(&self, agent_id: S1, session_key: S2) -> Result<Session>
    where
        S1: Into<String>,
        S2: Into<String>,
    {
        let existing_count = self.load_all_sessions()?.len();
        if existing_count >= self.max_sessions {
            return Err(SessionError::LimitExceeded.into());
        }

        let session = Session::new(agent_id, session_key);
        self.persist_session(&session)?;

        let mut sessions = self.sessions.write().await;
        sessions.insert(session.id.clone(), session.clone());

        info!(
            "Created session {} for agent {}",
            session.id, session.agent_id
        );
        Ok(session)
    }

    pub async fn get_session(&self, session_id: &str) -> Result<Option<Session>> {
        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(session_id) {
                return Ok(Some(session.clone()));
            }
        }

        let session = self
            .store
            .get(tables::SESSIONS, session_id)?
            .map(|bytes| serde_json::from_slice::<Session>(&bytes))
            .transpose()?;

        if let Some(ref session) = session {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session.id.clone(), session.clone());
        }

        Ok(session)
    }

    pub async fn find_by_session_key(&self, session_key: &str) -> Result<Option<Session>> {
        let mut sessions: Vec<Session> = self
            .load_all_sessions()?
            .into_iter()
            .filter(|session| session.session_key == session_key)
            .collect();
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        let session = sessions.into_iter().next();

        if let Some(ref session) = session {
            let mut cache = self.sessions.write().await;
            cache.insert(session.id.clone(), session.clone());
        }

        Ok(session)
    }

    pub async fn update_session(&self, session: &Session) -> Result<()> {
        self.persist_session(session)?;
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.id.clone(), session.clone());
        Ok(())
    }

    pub async fn update_working_memory(
        &self,
        session_id: &str,
        working_summary: String,
        last_user_intent: Option<String>,
        recent_tools: Vec<String>,
    ) -> Result<()> {
        let mut session =
            self.get_session(session_id)
                .await?
                .ok_or_else(|| SessionError::NotFound {
                    session_id: session_id.to_string(),
                })?;
        session.update_working_memory(working_summary, last_user_intent, recent_tools);
        self.update_session(&session).await
    }

    pub async fn update_context_budget(
        &self,
        session_id: &str,
        estimated_input_tokens: u64,
        max_context_tokens: u64,
    ) -> Result<()> {
        let mut session =
            self.get_session(session_id)
                .await?
                .ok_or_else(|| SessionError::NotFound {
                    session_id: session_id.to_string(),
                })?;
        session.update_context_budget(estimated_input_tokens, max_context_tokens);
        self.update_session(&session).await
    }

    pub async fn add_message(&self, session_id: &str, message: Message) -> Result<()> {
        let key = session_message_key(session_id, &message);
        let bytes = serde_json::to_vec(&message)?;
        self.store.put(tables::SESSION_MESSAGES, &key, &bytes)?;
        debug!("Added message {} to session {}", message.id, session_id);
        Ok(())
    }

    pub async fn get_message_history(
        &self,
        session_id: &str,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<MessageHistory> {
        let (start, end) = session_messages_range(session_id);
        let records = self.store.range(tables::SESSION_MESSAGES, &start, &end)?;
        let total_count = records.len();
        let limit = limit.unwrap_or(100);
        let offset = offset.unwrap_or(0);

        let messages = records
            .into_iter()
            .skip(offset)
            .take(limit)
            .enumerate()
            .map(|(idx, (_, bytes))| {
                let message: Message = serde_json::from_slice(&bytes)?;
                Ok(StoredMessage {
                    id: (offset + idx + 1) as i64,
                    session_id: session_id.to_string(),
                    message,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(MessageHistory {
            messages,
            total_count,
        })
    }

    pub async fn query_sessions(&self, query: SessionQuery) -> Result<Vec<Session>> {
        let mut sessions = self.load_all_sessions()?;
        sessions.retain(|session| {
            if let Some(ref agent_id) = query.agent_id {
                if &session.agent_id != agent_id {
                    return false;
                }
            }
            if let Some(ref session_key) = query.session_key {
                if &session.session_key != session_key {
                    return false;
                }
            }
            if let Some(ref state) = query.state {
                if serde_json::to_string(&session.state).ok() != serde_json::to_string(state).ok() {
                    return false;
                }
            }
            if query.exclude_archived && matches!(session.state, SessionState::Archived) {
                return false;
            }
            if let Some(created_after) = query.created_after {
                if session.created_at < created_after {
                    return false;
                }
            }
            if let Some(created_before) = query.created_before {
                if session.created_at > created_before {
                    return false;
                }
            }
            true
        });

        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));

        let offset = query.offset.unwrap_or(0);
        let limit = query.limit.unwrap_or(sessions.len());
        Ok(sessions.into_iter().skip(offset).take(limit).collect())
    }

    pub async fn archive_session(&self, session_id: &str) -> Result<()> {
        if let Some(mut session) = self.get_session(session_id).await? {
            session.set_state(SessionState::Archived);
            self.update_session(&session).await?;

            let mut sessions = self.sessions.write().await;
            sessions.remove(session_id);
            info!("Archived session {}", session_id);
            Ok(())
        } else {
            Err(SessionError::NotFound {
                session_id: session_id.to_string(),
            }
            .into())
        }
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        let deleted = self.store.delete(tables::SESSIONS, session_id)?;
        if !deleted {
            return Err(SessionError::NotFound {
                session_id: session_id.to_string(),
            }
            .into());
        }

        let (start, end) = session_messages_range(session_id);
        for (key, _) in self.store.range(tables::SESSION_MESSAGES, &start, &end)? {
            let _ = self.store.delete(tables::SESSION_MESSAGES, &key)?;
        }

        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id);
        info!("Deleted session {}", session_id);
        Ok(())
    }

    pub async fn get_statistics(&self) -> Result<SessionStatistics> {
        let sessions = self.load_all_sessions()?;
        let total_messages = self.store.list(tables::SESSION_MESSAGES)?.len() as u64;
        let total_tokens = sessions
            .iter()
            .map(|session| session.token_stats.total_tokens)
            .sum();

        Ok(SessionStatistics {
            total_sessions: sessions.len() as u64,
            active_sessions: sessions
                .iter()
                .filter(|session| session.is_active())
                .count() as u64,
            total_messages,
            total_tokens,
        })
    }
}

/// Session usage statistics
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionStatistics {
    pub total_sessions: u64,
    pub active_sessions: u64,
    pub total_messages: u64,
    pub total_tokens: u64,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_session_creation() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("sessions.redb");
        let manager = SessionManager::new(&db_path, 100).await.unwrap();

        let session = manager
            .create_session("test-agent", "test-key")
            .await
            .unwrap();

        assert!(!session.id.is_empty());
        assert_eq!(session.agent_id, "test-agent");
        assert_eq!(session.session_key, "test-key");
        assert!(session.is_active());
    }

    #[tokio::test]
    async fn test_session_retrieval() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("sessions.redb");
        let manager = SessionManager::new(&db_path, 100).await.unwrap();

        let session = manager
            .create_session("test-agent", "test-key")
            .await
            .unwrap();

        let retrieved = manager.get_session(&session.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, session.id);

        let by_key = manager.find_by_session_key("test-key").await.unwrap();
        assert!(by_key.is_some());
        assert_eq!(by_key.unwrap().id, session.id);
    }

    #[tokio::test]
    async fn test_message_history() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("sessions.redb");
        let manager = SessionManager::new(&db_path, 100).await.unwrap();

        let session = manager
            .create_session("test-agent", "test-key")
            .await
            .unwrap();

        let message1 = Message::text("Hello");
        let message2 = Message::text("World");

        manager.add_message(&session.id, message1).await.unwrap();
        manager.add_message(&session.id, message2).await.unwrap();

        let history = manager
            .get_message_history(&session.id, None, None)
            .await
            .unwrap();

        assert_eq!(history.total_count, 2);
        assert_eq!(history.messages.len(), 2);
    }

    #[tokio::test]
    async fn test_update_working_memory_persists() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("sessions.redb");
        let manager = SessionManager::new(&db_path, 100).await.unwrap();

        let session = manager
            .create_session("test-agent", "test-key")
            .await
            .unwrap();
        manager
            .update_working_memory(
                &session.id,
                "Investigating a failing test".to_string(),
                Some("Fix the failing startup test".to_string()),
                vec!["read".to_string(), "test".to_string()],
            )
            .await
            .unwrap();

        let updated = manager.get_session(&session.id).await.unwrap().unwrap();
        assert!(updated.memory.working_summary.contains("Investigating"));
        assert_eq!(
            updated.memory.last_user_intent.as_deref(),
            Some("Fix the failing startup test")
        );
        assert_eq!(updated.memory.recent_tools, vec!["read", "test"]);
    }

    #[tokio::test]
    async fn test_update_context_budget_sets_warning_status() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("sessions.redb");
        let manager = SessionManager::new(&db_path, 100).await.unwrap();

        let session = manager
            .create_session("test-agent", "test-key")
            .await
            .unwrap();
        manager
            .update_context_budget(&session.id, 80, 100)
            .await
            .unwrap();

        let updated = manager.get_session(&session.id).await.unwrap().unwrap();
        let snapshot = updated.memory.context_budget.expect("budget snapshot");
        assert_eq!(snapshot.utilization_percent, 80);
        assert_eq!(snapshot.status, ContextBudgetStatus::Warning);
    }
}
