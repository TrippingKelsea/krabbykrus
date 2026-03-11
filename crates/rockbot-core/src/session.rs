//! Session management for RockBot
//!
//! This module provides session tracking, message history management, and
//! persistent storage for agent conversations.

use crate::error::{Result, SessionError};
use crate::message::{Message, MessageRole};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info};
use uuid::Uuid;

/// Session identifier
pub type SessionId = String;

/// A conversation session with an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Unique session identifier
    pub id: SessionId,
    /// Agent ID this session belongs to
    pub agent_id: String,
    /// Session key for external reference (e.g., Discord channel ID)
    pub session_key: String,
    /// When the session was created
    pub created_at: DateTime<Utc>,
    /// When the session was last updated
    pub updated_at: DateTime<Utc>,
    /// Token usage statistics
    pub token_stats: TokenStats,
    /// Session metadata
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Current session state
    pub state: SessionState,
}

/// Token usage statistics for a session
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenStats {
    /// Total input tokens used
    pub input_tokens: u64,
    /// Total output tokens generated
    pub output_tokens: u64,
    /// Total tokens (input + output)
    pub total_tokens: u64,
}

/// Current state of a session
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    /// Session is active and ready for messages
    Active,
    /// Session is temporarily paused
    Paused,
    /// Session has been archived/closed
    Archived,
    /// Session encountered an error
    Error { message: String },
}

/// Session manager handles multiple concurrent sessions
pub struct SessionManager {
    /// Active sessions in memory
    sessions: Arc<RwLock<HashMap<SessionId, Session>>>,
    /// Database connection for persistence
    db: Arc<Mutex<Connection>>,
    /// Maximum number of concurrent sessions
    max_sessions: usize,
}

/// Session query parameters for searching/filtering
#[derive(Debug, Default)]
pub struct SessionQuery {
    pub agent_id: Option<String>,
    pub session_key: Option<String>,
    pub state: Option<SessionState>,
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
    /// Database row ID
    pub id: i64,
    /// Session this message belongs to
    pub session_id: SessionId,
    /// Message data
    pub message: Message,
}

impl Default for SessionState {
    fn default() -> Self {
        Self::Active
    }
}

impl Session {
    /// Create a new session
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
            metadata: HashMap::new(),
            state: SessionState::Active,
        }
    }
    
    /// Update token statistics
    pub fn add_tokens(&mut self, input: u64, output: u64) {
        self.token_stats.input_tokens += input;
        self.token_stats.output_tokens += output;
        self.token_stats.total_tokens += input + output;
        self.updated_at = Utc::now();
    }
    
    /// Set session state
    pub fn set_state(&mut self, state: SessionState) {
        self.state = state;
        self.updated_at = Utc::now();
    }
    
    /// Add metadata entry
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
    
    /// Check if session is active
    pub fn is_active(&self) -> bool {
        matches!(self.state, SessionState::Active)
    }
}

impl SessionManager {
    /// Create a new session manager
    pub async fn new<P: AsRef<Path>>(db_path: P, max_sessions: usize) -> Result<Self> {
        let path = db_path.as_ref();
        let db = Connection::open(path)?;
        
        // Initialize database schema
        db.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                session_key TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                total_tokens INTEGER DEFAULT 0,
                state TEXT DEFAULT 'active',
                metadata TEXT -- JSON
            );
            
            CREATE TABLE IF NOT EXISTS session_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                metadata TEXT, -- JSON
                created_at INTEGER NOT NULL,
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );
            
            CREATE INDEX IF NOT EXISTS idx_sessions_agent_id ON sessions(agent_id);
            CREATE INDEX IF NOT EXISTS idx_sessions_session_key ON sessions(session_key);
            CREATE INDEX IF NOT EXISTS idx_sessions_updated_at ON sessions(updated_at);
            CREATE INDEX IF NOT EXISTS idx_messages_session_id ON session_messages(session_id);
            CREATE INDEX IF NOT EXISTS idx_messages_created_at ON session_messages(created_at);
            "#,
        )?;
        
        info!("Session manager initialized with database at {:?}", path);
        
        Ok(Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            db: Arc::new(Mutex::new(db)),
            max_sessions,
        })
    }
    
    /// Create a new session
    pub async fn create_session<S1, S2>(&self, agent_id: S1, session_key: S2) -> Result<Session>
    where
        S1: Into<String>,
        S2: Into<String>,
    {
        let session = Session::new(agent_id, session_key);
        
        // Check session limit
        {
            let sessions = self.sessions.read().await;
            if sessions.len() >= self.max_sessions {
                return Err(SessionError::LimitExceeded.into());
            }
        }
        
        // Store in database
        {
            let db = self.db.lock().await;
            db.execute(
                "INSERT INTO sessions (id, agent_id, session_key, created_at, updated_at, state, metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    &session.id,
                    &session.agent_id,
                    &session.session_key,
                    session.created_at.timestamp(),
                    session.updated_at.timestamp(),
                    serde_json::to_string(&session.state)?,
                    serde_json::to_string(&session.metadata)?,
                ],
            )?;
        }
        
        // Add to in-memory cache
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(session.id.clone(), session.clone());
        }
        
        info!("Created session {} for agent {}", session.id, session.agent_id);
        Ok(session)
    }
    
    /// Get a session by ID
    pub async fn get_session(&self, session_id: &str) -> Result<Option<Session>> {
        // Check in-memory cache first
        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(session_id) {
                return Ok(Some(session.clone()));
            }
        }
        
        // Load from database
        let db = self.db.lock().await;
        let session: Option<Session> = db
            .query_row(
                "SELECT id, agent_id, session_key, created_at, updated_at, 
                        input_tokens, output_tokens, total_tokens, state, metadata
                 FROM sessions WHERE id = ?1",
                params![session_id],
                |row| {
                    let created_at = DateTime::from_timestamp(row.get::<_, i64>(3)?, 0)
                        .unwrap_or_else(Utc::now);
                    let updated_at = DateTime::from_timestamp(row.get::<_, i64>(4)?, 0)
                        .unwrap_or_else(Utc::now);
                    let state: String = row.get(8)?;
                    let metadata_str: String = row.get(9)?;
                    
                    Ok(Session {
                        id: row.get(0)?,
                        agent_id: row.get(1)?,
                        session_key: row.get(2)?,
                        created_at,
                        updated_at,
                        token_stats: TokenStats {
                            input_tokens: row.get::<_, u64>(5).unwrap_or(0),
                            output_tokens: row.get::<_, u64>(6).unwrap_or(0),
                            total_tokens: row.get::<_, u64>(7).unwrap_or(0),
                        },
                        metadata: serde_json::from_str(&metadata_str).unwrap_or_default(),
                        state: serde_json::from_str(&state).unwrap_or(SessionState::Active),
                    })
                },
            )
            .optional()?;
        
        if let Some(ref session) = session {
            // Cache in memory
            let mut sessions = self.sessions.write().await;
            sessions.insert(session.id.clone(), session.clone());
        }
        
        Ok(session)
    }
    
    /// Find session by session key
    pub async fn find_by_session_key(&self, session_key: &str) -> Result<Option<Session>> {
        let db = self.db.lock().await;
        let session_id: Option<String> = db
            .query_row(
                "SELECT id FROM sessions WHERE session_key = ?1 ORDER BY updated_at DESC LIMIT 1",
                params![session_key],
                |row| row.get(0),
            )
            .optional()?;
        
        if let Some(session_id) = session_id {
            self.get_session(&session_id).await
        } else {
            Ok(None)
        }
    }
    
    /// Update session in database
    pub async fn update_session(&self, session: &Session) -> Result<()> {
        let db = self.db.lock().await;
        db.execute(
            "UPDATE sessions SET updated_at = ?1, input_tokens = ?2, output_tokens = ?3,
                    total_tokens = ?4, state = ?5, metadata = ?6 WHERE id = ?7",
            params![
                session.updated_at.timestamp(),
                session.token_stats.input_tokens,
                session.token_stats.output_tokens,
                session.token_stats.total_tokens,
                serde_json::to_string(&session.state)?,
                serde_json::to_string(&session.metadata)?,
                &session.id,
            ],
        )?;
        
        // Update in-memory cache
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.id.clone(), session.clone());
        
        Ok(())
    }
    
    /// Add a message to a session
    pub async fn add_message(&self, session_id: &str, message: Message) -> Result<()> {
        let db = self.db.lock().await;
        db.execute(
            "INSERT INTO session_messages (session_id, message_id, role, content, metadata, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                session_id,
                &message.id,
                serde_json::to_string(&message.metadata.role)?,
                serde_json::to_string(&message.content)?,
                serde_json::to_string(&message.metadata)?,
                message.created_at.timestamp(),
            ],
        )?;
        
        debug!("Added message {} to session {}", message.id, session_id);
        Ok(())
    }
    
    /// Get message history for a session
    pub async fn get_message_history(
        &self,
        session_id: &str,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<MessageHistory> {
        let limit = limit.unwrap_or(100);
        let offset = offset.unwrap_or(0);
        
        let db = self.db.lock().await;
        
        // Get total count
        let total_count: usize = db.query_row(
            "SELECT COUNT(*) FROM session_messages WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        
        // Get messages
        let mut stmt = db.prepare(
            "SELECT id, session_id, message_id, role, content, metadata, created_at
             FROM session_messages WHERE session_id = ?1
             ORDER BY created_at ASC LIMIT ?2 OFFSET ?3",
        )?;
        
        let message_iter = stmt.query_map(params![session_id, limit, offset], |row| {
            let created_at = DateTime::from_timestamp(row.get::<_, i64>(6)?, 0)
                .unwrap_or_else(Utc::now);
            let role_str: String = row.get(3)?;
            let content_str: String = row.get(4)?;
            let metadata_str: String = row.get(5)?;
            
            let role = serde_json::from_str(&role_str).unwrap_or(MessageRole::User);
            let content = serde_json::from_str(&content_str)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                    4, 
                    rusqlite::types::Type::Text,
                    Box::new(e)
                ))?;
            let metadata = serde_json::from_str(&metadata_str).unwrap_or_default();
            
            Ok(StoredMessage {
                id: row.get(0)?,
                session_id: row.get(1)?,
                message: Message {
                    id: row.get(2)?,
                    content,
                    metadata,
                    attachments: Vec::new(), // Attachments stored separately
                    created_at,
                },
            })
        })?;
        
        let mut messages = Vec::new();
        for message_result in message_iter {
            messages.push(message_result?);
        }
        
        Ok(MessageHistory {
            messages,
            total_count,
        })
    }
    
    /// Query sessions with filters
    pub async fn query_sessions(&self, query: SessionQuery) -> Result<Vec<Session>> {
        let db = self.db.lock().await;
        let mut sql = String::from(
            "SELECT id, agent_id, session_key, created_at, updated_at, 
                    input_tokens, output_tokens, total_tokens, state, metadata
             FROM sessions WHERE 1=1"
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        
        if let Some(ref agent_id) = query.agent_id {
            sql.push_str(" AND agent_id = ?");
            params.push(Box::new(agent_id.clone()));
        }
        
        if let Some(ref session_key) = query.session_key {
            sql.push_str(" AND session_key = ?");
            params.push(Box::new(session_key.clone()));
        }
        
        if let Some(ref created_after) = query.created_after {
            sql.push_str(" AND created_at >= ?");
            params.push(Box::new(created_after.timestamp()));
        }
        
        if let Some(ref created_before) = query.created_before {
            sql.push_str(" AND created_at <= ?");
            params.push(Box::new(created_before.timestamp()));
        }
        
        sql.push_str(" ORDER BY updated_at DESC");
        
        if let Some(limit) = query.limit {
            sql.push_str(&format!(" LIMIT {}", limit));
        }
        
        if let Some(offset) = query.offset {
            sql.push_str(&format!(" OFFSET {}", offset));
        }
        
        let mut stmt = db.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let session_iter = stmt.query_map(params_refs.as_slice(), |row| {
            let created_at = DateTime::from_timestamp(row.get::<_, i64>(3)?, 0)
                .unwrap_or_else(Utc::now);
            let updated_at = DateTime::from_timestamp(row.get::<_, i64>(4)?, 0)
                .unwrap_or_else(Utc::now);
            let state_str: String = row.get(8)?;
            let metadata_str: String = row.get(9)?;
            
            Ok(Session {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                session_key: row.get(2)?,
                created_at,
                updated_at,
                token_stats: TokenStats {
                    input_tokens: row.get::<_, u64>(5).unwrap_or(0),
                    output_tokens: row.get::<_, u64>(6).unwrap_or(0),
                    total_tokens: row.get::<_, u64>(7).unwrap_or(0),
                },
                metadata: serde_json::from_str(&metadata_str).unwrap_or_default(),
                state: serde_json::from_str(&state_str).unwrap_or(SessionState::Active),
            })
        })?;
        
        let mut sessions = Vec::new();
        for session_result in session_iter {
            sessions.push(session_result?);
        }
        
        Ok(sessions)
    }
    
    /// Archive a session (mark as archived)
    pub async fn archive_session(&self, session_id: &str) -> Result<()> {
        if let Some(mut session) = self.get_session(session_id).await? {
            session.set_state(SessionState::Archived);
            self.update_session(&session).await?;
            
            // Remove from in-memory cache
            let mut sessions = self.sessions.write().await;
            sessions.remove(session_id);
            
            info!("Archived session {}", session_id);
        } else {
            return Err(SessionError::NotFound {
                session_id: session_id.to_string(),
            }
            .into());
        }
        
        Ok(())
    }
    
    /// Delete a session and all its messages
    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        let db = self.db.lock().await;
        
        // Delete messages first (due to foreign key constraint)
        db.execute(
            "DELETE FROM session_messages WHERE session_id = ?1",
            params![session_id],
        )?;
        
        // Delete session
        let affected = db.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])?;
        
        if affected == 0 {
            return Err(SessionError::NotFound {
                session_id: session_id.to_string(),
            }
            .into());
        }
        
        // Remove from in-memory cache
        {
            let mut sessions = self.sessions.write().await;
            sessions.remove(session_id);
        }
        
        info!("Deleted session {}", session_id);
        Ok(())
    }
    
    /// Get statistics about sessions
    pub async fn get_statistics(&self) -> Result<SessionStatistics> {
        let db = self.db.lock().await;
        
        let total_sessions: i64 = db.query_row("SELECT COUNT(*) FROM sessions", [], |row| {
            row.get(0)
        })?;
        
        let active_sessions: i64 = db.query_row(
            "SELECT COUNT(*) FROM sessions WHERE state = 'active'",
            [],
            |row| row.get(0),
        )?;
        
        let total_messages: i64 =
            db.query_row("SELECT COUNT(*) FROM session_messages", [], |row| row.get(0))?;
        
        let total_tokens: Option<i64> = db.query_row(
            "SELECT SUM(total_tokens) FROM sessions",
            [],
            |row| row.get(0),
        )?;
        
        Ok(SessionStatistics {
            total_sessions: total_sessions as u64,
            active_sessions: active_sessions as u64,
            total_messages: total_messages as u64,
            total_tokens: total_tokens.unwrap_or(0) as u64,
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
    use super::*;
    use tempfile::NamedTempFile;
    
    #[tokio::test]
    async fn test_session_creation() {
        let temp_db = NamedTempFile::new().unwrap();
        let manager = SessionManager::new(temp_db.path(), 100).await.unwrap();
        
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
        let temp_db = NamedTempFile::new().unwrap();
        let manager = SessionManager::new(temp_db.path(), 100).await.unwrap();
        
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
        let temp_db = NamedTempFile::new().unwrap();
        let manager = SessionManager::new(temp_db.path(), 100).await.unwrap();
        
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
}