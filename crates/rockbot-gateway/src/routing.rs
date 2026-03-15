//! Routing system for RockBot
//!
//! This module implements the hierarchical binding-based routing system (SPEC Section 5)
//! that resolves incoming messages to agent instances. It supports multiple binding
//! types with priority-based resolution and configurable session scoping modes.

use crate::error::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// How a route was matched during resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchedByType {
    /// Thread/conversation-specific binding.
    BindingPeer,
    /// Inherited from parent thread.
    BindingPeerParent,
    /// Discord role-based routing.
    BindingGuildRoles,
    /// Discord guild-wide binding.
    BindingGuild,
    /// Microsoft Teams team binding.
    BindingTeam,
    /// Account-level default.
    BindingAccount,
    /// Channel-level default.
    BindingChannel,
    /// Global fallback agent.
    Default,
}

impl std::fmt::Display for MatchedByType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::BindingPeer => "binding.peer",
            Self::BindingPeerParent => "binding.peer.parent",
            Self::BindingGuildRoles => "binding.guild+roles",
            Self::BindingGuild => "binding.guild",
            Self::BindingTeam => "binding.team",
            Self::BindingAccount => "binding.account",
            Self::BindingChannel => "binding.channel",
            Self::Default => "default",
        };
        write!(f, "{label}")
    }
}

/// Policy that governs whether the resolved route uses the main session or a
/// scoped sub-session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum RoutePolicy {
    /// Use the main (shared) session.
    Main,
    /// Use a scoped per-context session.
    #[default]
    Session,
}


/// The fully-resolved route for an incoming message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedAgentRoute {
    /// ID of the agent that should handle this message.
    pub agent_id: String,
    /// Channel the message arrived on (e.g. "telegram", "discord").
    pub channel: String,
    /// Account ID within the channel.
    pub account_id: String,
    /// Scoped session key (depends on [`SessionScope`]).
    pub session_key: String,
    /// Main session key (always `main:{channel}:{account_id}`).
    pub main_session_key: String,
    /// Route policy that was active when the route was resolved.
    pub last_route_policy: RoutePolicy,
    /// How the route was matched.
    pub matched_by: MatchedByType,
}

/// Session scoping modes (SPEC Section 6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum SessionScope {
    /// Separate session per unique sender.
    PerSender,
    /// Shared session across all senders.
    Global,
    /// Session per conversation/thread.
    #[default]
    PerPeer,
    /// Session per channel + conversation.
    PerChannelPeer,
    /// Most specific: account + channel + conversation.
    PerAccountChannelPeer,
}


// ---------------------------------------------------------------------------
// Session key
// ---------------------------------------------------------------------------

/// A parsed session key of the form `{scope}:{channel}:{identifier}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionKey {
    /// The scope component (e.g. "main", "direct", "thread").
    pub scope: String,
    /// The channel component (e.g. "telegram", "discord", "slack").
    pub channel: String,
    /// The identifier component (e.g. user/thread ID).
    pub identifier: String,
}

impl SessionKey {
    /// Create a new session key.
    pub fn new(scope: impl Into<String>, channel: impl Into<String>, identifier: impl Into<String>) -> Self {
        Self {
            scope: scope.into(),
            channel: channel.into(),
            identifier: identifier.into(),
        }
    }

    /// Parse a session key string of the form `scope:channel:identifier`.
    ///
    /// The identifier component may itself contain colons (e.g.
    /// `thread:slack:C123456_ts1234567890`).
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.splitn(3, ':');
        let scope = parts.next()?.to_string();
        let channel = parts.next()?.to_string();
        let identifier = parts.next()?.to_string();

        if scope.is_empty() || channel.is_empty() || identifier.is_empty() {
            return None;
        }

        Some(Self {
            scope,
            channel,
            identifier,
        })
    }

    /// Build a main session key for the given channel and account.
    pub fn main_key(channel: &str, account_id: &str) -> Self {
        Self::new("main", channel, account_id)
    }
}

impl std::fmt::Display for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.scope, self.channel, self.identifier)
    }
}

// ---------------------------------------------------------------------------
// Bindings
// ---------------------------------------------------------------------------

/// The kind of binding, determining what context it applies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingKind {
    /// Specific peer (thread/conversation).
    Peer {
        /// Channel name.
        channel: String,
        /// Peer identifier (thread ID, conversation ID, etc.).
        peer_id: String,
    },
    /// Guild-wide binding (Discord).
    Guild {
        /// Channel name (always "discord" for guild bindings).
        channel: String,
        /// Guild (server) ID.
        guild_id: String,
    },
    /// Guild + role binding (Discord).
    GuildRoles {
        /// Channel name.
        channel: String,
        /// Guild (server) ID.
        guild_id: String,
        /// Role IDs the sender must have (any match).
        role_ids: Vec<String>,
    },
    /// Team binding (Microsoft Teams).
    Team {
        /// Channel name (always "teams" for team bindings).
        channel: String,
        /// Team ID.
        team_id: String,
    },
    /// Account-level default.
    Account {
        /// Channel name.
        channel: String,
        /// Account ID.
        account_id: String,
    },
    /// Channel-level default (applies to all messages on that channel).
    Channel {
        /// Channel name.
        channel: String,
    },
}

/// A binding that maps a context to an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Binding {
    /// Unique binding identifier.
    pub id: String,
    /// The agent this binding routes to.
    pub agent_id: String,
    /// What kind of context this binding matches.
    pub kind: BindingKind,
    /// Optional route policy override.
    #[serde(default)]
    pub route_policy: RoutePolicy,
    /// Optional session scope override.
    pub session_scope: Option<SessionScope>,
    /// When the binding was created.
    pub created_at: DateTime<Utc>,
    /// When the binding was last updated.
    pub updated_at: DateTime<Utc>,
}

impl Binding {
    /// Create a new binding.
    pub fn new(agent_id: impl Into<String>, kind: BindingKind) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.into(),
            kind,
            route_policy: RoutePolicy::default(),
            session_scope: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Return the [`MatchedByType`] this binding corresponds to.
    pub fn matched_by_type(&self) -> MatchedByType {
        match &self.kind {
            BindingKind::Peer { .. } => MatchedByType::BindingPeer,
            BindingKind::Guild { .. } => MatchedByType::BindingGuild,
            BindingKind::GuildRoles { .. } => MatchedByType::BindingGuildRoles,
            BindingKind::Team { .. } => MatchedByType::BindingTeam,
            BindingKind::Account { .. } => MatchedByType::BindingAccount,
            BindingKind::Channel { .. } => MatchedByType::BindingChannel,
        }
    }
}

// ---------------------------------------------------------------------------
// Message context (input to route resolution)
// ---------------------------------------------------------------------------

/// Context about an incoming message used for route resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRoutingContext {
    /// Channel the message arrived on (e.g. "telegram", "discord").
    pub channel: String,
    /// Account ID within the channel.
    pub account_id: String,
    /// Sender identifier.
    pub sender_id: String,
    /// Peer (thread/conversation) identifier, if applicable.
    pub peer_id: Option<String>,
    /// Parent peer identifier (for sub-threads).
    pub parent_peer_id: Option<String>,
    /// Guild (server) ID, if applicable (Discord).
    pub guild_id: Option<String>,
    /// Role IDs the sender has, if applicable (Discord).
    #[serde(default)]
    pub role_ids: Vec<String>,
    /// Team ID, if applicable (Microsoft Teams).
    pub team_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Routing engine
// ---------------------------------------------------------------------------

/// The routing engine stores bindings and resolves incoming messages to agents.
pub struct RoutingEngine {
    /// In-memory binding cache keyed by binding ID.
    bindings: Arc<RwLock<HashMap<String, Binding>>>,
    /// Database connection for persistence.
    db: Arc<Mutex<Connection>>,
    /// Default agent ID used when no binding matches.
    default_agent_id: String,
    /// Default session scope used when a binding does not specify one.
    default_session_scope: SessionScope,
}

impl RoutingEngine {
    /// Create a new routing engine backed by an SQLite database.
    pub async fn new<P: AsRef<Path>>(
        db_path: P,
        default_agent_id: impl Into<String>,
        default_session_scope: SessionScope,
    ) -> Result<Self> {
        let path = db_path.as_ref();
        let db = Connection::open(path)?;

        db.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS route_bindings (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                kind TEXT NOT NULL,       -- JSON-encoded BindingKind
                route_policy TEXT NOT NULL DEFAULT 'session',
                session_scope TEXT,       -- JSON-encoded SessionScope, nullable
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_bindings_agent_id ON route_bindings(agent_id);
            "#,
        )?;

        let engine = Self {
            bindings: Arc::new(RwLock::new(HashMap::new())),
            db: Arc::new(Mutex::new(db)),
            default_agent_id: default_agent_id.into(),
            default_session_scope,
        };

        // Load persisted bindings into memory.
        engine.load_bindings().await?;

        info!("Routing engine initialized with database at {:?}", path);
        Ok(engine)
    }

    // -----------------------------------------------------------------------
    // CRUD operations
    // -----------------------------------------------------------------------

    /// Add a new binding and persist it.
    pub async fn add_binding(&self, binding: Binding) -> Result<()> {
        // Persist.
        {
            let db = self.db.lock().await;
            let kind_json = serde_json::to_string(&binding.kind)?;
            let policy_json = serde_json::to_string(&binding.route_policy)?;
            let scope_json = binding
                .session_scope
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?;

            db.execute(
                "INSERT INTO route_bindings (id, agent_id, kind, route_policy, session_scope, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    &binding.id,
                    &binding.agent_id,
                    &kind_json,
                    &policy_json,
                    &scope_json,
                    binding.created_at.timestamp(),
                    binding.updated_at.timestamp(),
                ],
            )?;
        }

        // Update in-memory cache.
        {
            let mut bindings = self.bindings.write().await;
            bindings.insert(binding.id.clone(), binding);
        }

        Ok(())
    }

    /// Remove a binding by ID.
    pub async fn remove_binding(&self, binding_id: &str) -> Result<bool> {
        let affected = {
            let db = self.db.lock().await;
            db.execute(
                "DELETE FROM route_bindings WHERE id = ?1",
                params![binding_id],
            )?
        };

        let mut bindings = self.bindings.write().await;
        bindings.remove(binding_id);

        Ok(affected > 0)
    }

    /// Update an existing binding (full replacement).
    pub async fn update_binding(&self, mut binding: Binding) -> Result<bool> {
        binding.updated_at = Utc::now();

        let affected = {
            let db = self.db.lock().await;
            let kind_json = serde_json::to_string(&binding.kind)?;
            let policy_json = serde_json::to_string(&binding.route_policy)?;
            let scope_json = binding
                .session_scope
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?;

            db.execute(
                "UPDATE route_bindings SET agent_id = ?1, kind = ?2, route_policy = ?3,
                        session_scope = ?4, updated_at = ?5 WHERE id = ?6",
                params![
                    &binding.agent_id,
                    &kind_json,
                    &policy_json,
                    &scope_json,
                    binding.updated_at.timestamp(),
                    &binding.id,
                ],
            )?
        };

        if affected > 0 {
            let mut bindings = self.bindings.write().await;
            bindings.insert(binding.id.clone(), binding);
        }

        Ok(affected > 0)
    }

    /// Get a binding by ID.
    pub async fn get_binding(&self, binding_id: &str) -> Option<Binding> {
        let bindings = self.bindings.read().await;
        bindings.get(binding_id).cloned()
    }

    /// List all bindings, optionally filtered by agent ID.
    pub async fn list_bindings(&self, agent_id: Option<&str>) -> Vec<Binding> {
        let bindings = self.bindings.read().await;
        bindings
            .values()
            .filter(|b| agent_id.is_none_or(|id| b.agent_id == id))
            .cloned()
            .collect()
    }

    // -----------------------------------------------------------------------
    // Route resolution
    // -----------------------------------------------------------------------

    /// Resolve the route for an incoming message.
    ///
    /// Walks the binding hierarchy from most-specific to least-specific:
    ///
    /// 1. Peer binding (exact thread/conversation match)
    /// 2. Parent peer binding (inherited from parent thread)
    /// 3. Guild + roles (Discord role-based)
    /// 4. Guild (Discord guild-wide)
    /// 5. Team (Microsoft Teams)
    /// 6. Account (account-level default)
    /// 7. Channel (channel-level default)
    /// 8. Default (global fallback)
    pub async fn resolve(&self, ctx: &MessageRoutingContext) -> ResolvedAgentRoute {
        let bindings = self.bindings.read().await;
        let all_bindings: Vec<&Binding> = bindings.values().collect();

        // 1. Peer binding
        if let Some(peer_id) = &ctx.peer_id {
            if let Some(b) = find_peer_binding(&all_bindings, &ctx.channel, peer_id) {
                debug!("Route matched by peer binding for {}:{}", ctx.channel, peer_id);
                return self.build_route(ctx, b, MatchedByType::BindingPeer);
            }
        }

        // 2. Parent peer binding
        if let Some(parent_peer_id) = &ctx.parent_peer_id {
            if let Some(b) = find_peer_binding(&all_bindings, &ctx.channel, parent_peer_id) {
                debug!(
                    "Route matched by parent peer binding for {}:{}",
                    ctx.channel, parent_peer_id
                );
                return self.build_route(ctx, b, MatchedByType::BindingPeerParent);
            }
        }

        // 3. Guild + roles
        if let (Some(guild_id), roles) = (&ctx.guild_id, &ctx.role_ids) {
            if !roles.is_empty() {
                if let Some(b) =
                    find_guild_roles_binding(&all_bindings, &ctx.channel, guild_id, roles)
                {
                    debug!(
                        "Route matched by guild+roles binding for {}:{}",
                        ctx.channel, guild_id
                    );
                    return self.build_route(ctx, b, MatchedByType::BindingGuildRoles);
                }
            }
        }

        // 4. Guild
        if let Some(guild_id) = &ctx.guild_id {
            if let Some(b) = find_guild_binding(&all_bindings, &ctx.channel, guild_id) {
                debug!(
                    "Route matched by guild binding for {}:{}",
                    ctx.channel, guild_id
                );
                return self.build_route(ctx, b, MatchedByType::BindingGuild);
            }
        }

        // 5. Team
        if let Some(team_id) = &ctx.team_id {
            if let Some(b) = find_team_binding(&all_bindings, &ctx.channel, team_id) {
                debug!(
                    "Route matched by team binding for {}:{}",
                    ctx.channel, team_id
                );
                return self.build_route(ctx, b, MatchedByType::BindingTeam);
            }
        }

        // 6. Account
        if let Some(b) = find_account_binding(&all_bindings, &ctx.channel, &ctx.account_id) {
            debug!(
                "Route matched by account binding for {}:{}",
                ctx.channel, ctx.account_id
            );
            return self.build_route(ctx, b, MatchedByType::BindingAccount);
        }

        // 7. Channel
        if let Some(b) = find_channel_binding(&all_bindings, &ctx.channel) {
            debug!("Route matched by channel binding for {}", ctx.channel);
            return self.build_route(ctx, b, MatchedByType::BindingChannel);
        }

        // 8. Default
        debug!("Route fell through to default agent");
        self.build_default_route(ctx)
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Construct a [`ResolvedAgentRoute`] from a matched binding.
    fn build_route(
        &self,
        ctx: &MessageRoutingContext,
        binding: &Binding,
        matched_by: MatchedByType,
    ) -> ResolvedAgentRoute {
        let scope = binding
            .session_scope
            .as_ref()
            .unwrap_or(&self.default_session_scope);
        let session_key = self.compute_session_key(ctx, scope);
        let main_session_key = SessionKey::main_key(&ctx.channel, &ctx.account_id).to_string();

        ResolvedAgentRoute {
            agent_id: binding.agent_id.clone(),
            channel: ctx.channel.clone(),
            account_id: ctx.account_id.clone(),
            session_key,
            main_session_key,
            last_route_policy: binding.route_policy.clone(),
            matched_by,
        }
    }

    /// Construct the default (fallback) route.
    fn build_default_route(&self, ctx: &MessageRoutingContext) -> ResolvedAgentRoute {
        let session_key = self.compute_session_key(ctx, &self.default_session_scope);
        let main_session_key = SessionKey::main_key(&ctx.channel, &ctx.account_id).to_string();

        ResolvedAgentRoute {
            agent_id: self.default_agent_id.clone(),
            channel: ctx.channel.clone(),
            account_id: ctx.account_id.clone(),
            session_key,
            main_session_key,
            last_route_policy: RoutePolicy::default(),
            matched_by: MatchedByType::Default,
        }
    }

    /// Compute the session key for the given context and scope.
    #[allow(clippy::unused_self)]
    fn compute_session_key(&self, ctx: &MessageRoutingContext, scope: &SessionScope) -> String {
        match scope {
            SessionScope::Global => {
                SessionKey::new("global", &ctx.channel, "shared").to_string()
            }
            SessionScope::PerSender => {
                SessionKey::new("sender", &ctx.channel, &ctx.sender_id).to_string()
            }
            SessionScope::PerPeer => {
                let peer = ctx.peer_id.as_deref().unwrap_or(&ctx.sender_id);
                SessionKey::new("peer", &ctx.channel, peer).to_string()
            }
            SessionScope::PerChannelPeer => {
                let peer = ctx.peer_id.as_deref().unwrap_or(&ctx.sender_id);
                let identifier = format!("{}_{}", ctx.channel, peer);
                SessionKey::new("chpeer", &ctx.channel, &identifier).to_string()
            }
            SessionScope::PerAccountChannelPeer => {
                let peer = ctx.peer_id.as_deref().unwrap_or(&ctx.sender_id);
                let identifier = format!("{}_{}_{}", ctx.account_id, ctx.channel, peer);
                SessionKey::new("acctchpeer", &ctx.channel, &identifier).to_string()
            }
        }
    }

    /// Load all bindings from the database into the in-memory cache.
    async fn load_bindings(&self) -> Result<()> {
        let loaded: Vec<Binding> = {
            let db = self.db.lock().await;
            let mut stmt = db.prepare(
                "SELECT id, agent_id, kind, route_policy, session_scope, created_at, updated_at
                 FROM route_bindings",
            )?;

            let rows = stmt.query_map([], |row| {
                let id: String = row.get(0)?;
                let agent_id: String = row.get(1)?;
                let kind_json: String = row.get(2)?;
                let policy_json: String = row.get(3)?;
                let scope_json: Option<String> = row.get(4)?;
                let created_ts: i64 = row.get(5)?;
                let updated_ts: i64 = row.get(6)?;

                let kind: BindingKind = serde_json::from_str(&kind_json).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                let route_policy: RoutePolicy =
                    serde_json::from_str(&policy_json).unwrap_or_default();
                let session_scope: Option<SessionScope> = scope_json
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .unwrap_or(None);

                let created_at =
                    DateTime::from_timestamp(created_ts, 0).unwrap_or_else(Utc::now);
                let updated_at =
                    DateTime::from_timestamp(updated_ts, 0).unwrap_or_else(Utc::now);

                Ok(Binding {
                    id,
                    agent_id,
                    kind,
                    route_policy,
                    session_scope,
                    created_at,
                    updated_at,
                })
            })?;

            let mut result = Vec::new();
            for row in rows {
                match row {
                    Ok(binding) => result.push(binding),
                    Err(e) => {
                        warn!("Failed to load binding from database: {}", e);
                    }
                }
            }
            result
        };

        let mut bindings = self.bindings.write().await;
        for binding in loaded {
            bindings.insert(binding.id.clone(), binding);
        }

        info!("Loaded {} route bindings from database", bindings.len());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Binding lookup helpers
// ---------------------------------------------------------------------------

fn find_peer_binding<'a>(
    bindings: &[&'a Binding],
    channel: &str,
    peer_id: &str,
) -> Option<&'a Binding> {
    bindings.iter().find(|b| {
        matches!(
            &b.kind,
            BindingKind::Peer {
                channel: ch,
                peer_id: pid,
            } if ch == channel && pid == peer_id
        )
    }).copied()
}

fn find_guild_roles_binding<'a>(
    bindings: &[&'a Binding],
    channel: &str,
    guild_id: &str,
    sender_roles: &[String],
) -> Option<&'a Binding> {
    bindings.iter().find(|b| {
        matches!(
            &b.kind,
            BindingKind::GuildRoles {
                channel: ch,
                guild_id: gid,
                role_ids,
            } if ch == channel
                && gid == guild_id
                && role_ids.iter().any(|r| sender_roles.contains(r))
        )
    }).copied()
}

fn find_guild_binding<'a>(
    bindings: &[&'a Binding],
    channel: &str,
    guild_id: &str,
) -> Option<&'a Binding> {
    bindings.iter().find(|b| {
        matches!(
            &b.kind,
            BindingKind::Guild {
                channel: ch,
                guild_id: gid,
            } if ch == channel && gid == guild_id
        )
    }).copied()
}

fn find_team_binding<'a>(
    bindings: &[&'a Binding],
    channel: &str,
    team_id: &str,
) -> Option<&'a Binding> {
    bindings.iter().find(|b| {
        matches!(
            &b.kind,
            BindingKind::Team {
                channel: ch,
                team_id: tid,
            } if ch == channel && tid == team_id
        )
    }).copied()
}

fn find_account_binding<'a>(
    bindings: &[&'a Binding],
    channel: &str,
    account_id: &str,
) -> Option<&'a Binding> {
    bindings.iter().find(|b| {
        matches!(
            &b.kind,
            BindingKind::Account {
                channel: ch,
                account_id: aid,
            } if ch == channel && aid == account_id
        )
    }).copied()
}

fn find_channel_binding<'a>(
    bindings: &[&'a Binding],
    channel: &str,
) -> Option<&'a Binding> {
    bindings.iter().find(|b| {
        matches!(
            &b.kind,
            BindingKind::Channel { channel: ch } if ch == channel
        )
    }).copied()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use tempfile::NamedTempFile;

    fn telegram_ctx() -> MessageRoutingContext {
        MessageRoutingContext {
            channel: "telegram".to_string(),
            account_id: "bot_123".to_string(),
            sender_id: "user_456".to_string(),
            peer_id: Some("chat_789".to_string()),
            parent_peer_id: None,
            guild_id: None,
            role_ids: vec![],
            team_id: None,
        }
    }

    #[test]
    fn test_session_key_parse() {
        let key = SessionKey::parse("main:telegram:123456789").unwrap();
        assert_eq!(key.scope, "main");
        assert_eq!(key.channel, "telegram");
        assert_eq!(key.identifier, "123456789");
        assert_eq!(key.to_string(), "main:telegram:123456789");
    }

    #[test]
    fn test_session_key_parse_with_colons_in_identifier() {
        let key = SessionKey::parse("thread:slack:C123456_ts1234567890").unwrap();
        assert_eq!(key.scope, "thread");
        assert_eq!(key.channel, "slack");
        assert_eq!(key.identifier, "C123456_ts1234567890");
    }

    #[test]
    fn test_session_key_parse_invalid() {
        assert!(SessionKey::parse("only_one_part").is_none());
        assert!(SessionKey::parse("two:parts").is_none());
        assert!(SessionKey::parse("::empty").is_none());
        assert!(SessionKey::parse(":channel:id").is_none());
    }

    #[test]
    fn test_session_key_main() {
        let key = SessionKey::main_key("telegram", "bot_123");
        assert_eq!(key.to_string(), "main:telegram:bot_123");
    }

    #[test]
    fn test_matched_by_display() {
        assert_eq!(MatchedByType::BindingPeer.to_string(), "binding.peer");
        assert_eq!(MatchedByType::Default.to_string(), "default");
        assert_eq!(
            MatchedByType::BindingGuildRoles.to_string(),
            "binding.guild+roles"
        );
    }

    #[tokio::test]
    async fn test_default_route() {
        let temp_db = NamedTempFile::new().unwrap();
        let engine = RoutingEngine::new(temp_db.path(), "default-agent", SessionScope::PerPeer)
            .await
            .unwrap();

        let ctx = telegram_ctx();
        let route = engine.resolve(&ctx).await;

        assert_eq!(route.agent_id, "default-agent");
        assert_eq!(route.matched_by, MatchedByType::Default);
        assert_eq!(route.channel, "telegram");
        assert_eq!(route.main_session_key, "main:telegram:bot_123");
    }

    #[tokio::test]
    async fn test_peer_binding_resolution() {
        let temp_db = NamedTempFile::new().unwrap();
        let engine = RoutingEngine::new(temp_db.path(), "default-agent", SessionScope::PerPeer)
            .await
            .unwrap();

        let binding = Binding::new(
            "special-agent",
            BindingKind::Peer {
                channel: "telegram".to_string(),
                peer_id: "chat_789".to_string(),
            },
        );
        engine.add_binding(binding).await.unwrap();

        let ctx = telegram_ctx();
        let route = engine.resolve(&ctx).await;

        assert_eq!(route.agent_id, "special-agent");
        assert_eq!(route.matched_by, MatchedByType::BindingPeer);
    }

    #[tokio::test]
    async fn test_channel_binding_fallback() {
        let temp_db = NamedTempFile::new().unwrap();
        let engine = RoutingEngine::new(temp_db.path(), "default-agent", SessionScope::PerPeer)
            .await
            .unwrap();

        let binding = Binding::new(
            "telegram-agent",
            BindingKind::Channel {
                channel: "telegram".to_string(),
            },
        );
        engine.add_binding(binding).await.unwrap();

        let ctx = telegram_ctx();
        let route = engine.resolve(&ctx).await;

        assert_eq!(route.agent_id, "telegram-agent");
        assert_eq!(route.matched_by, MatchedByType::BindingChannel);
    }

    #[tokio::test]
    async fn test_peer_overrides_channel() {
        let temp_db = NamedTempFile::new().unwrap();
        let engine = RoutingEngine::new(temp_db.path(), "default-agent", SessionScope::PerPeer)
            .await
            .unwrap();

        let channel_binding = Binding::new(
            "channel-agent",
            BindingKind::Channel {
                channel: "telegram".to_string(),
            },
        );
        let peer_binding = Binding::new(
            "peer-agent",
            BindingKind::Peer {
                channel: "telegram".to_string(),
                peer_id: "chat_789".to_string(),
            },
        );
        engine.add_binding(channel_binding).await.unwrap();
        engine.add_binding(peer_binding).await.unwrap();

        let ctx = telegram_ctx();
        let route = engine.resolve(&ctx).await;

        assert_eq!(route.agent_id, "peer-agent");
        assert_eq!(route.matched_by, MatchedByType::BindingPeer);
    }

    #[tokio::test]
    async fn test_guild_roles_binding() {
        let temp_db = NamedTempFile::new().unwrap();
        let engine = RoutingEngine::new(temp_db.path(), "default-agent", SessionScope::PerPeer)
            .await
            .unwrap();

        let binding = Binding::new(
            "admin-agent",
            BindingKind::GuildRoles {
                channel: "discord".to_string(),
                guild_id: "guild_001".to_string(),
                role_ids: vec!["admin".to_string(), "moderator".to_string()],
            },
        );
        engine.add_binding(binding).await.unwrap();

        let ctx = MessageRoutingContext {
            channel: "discord".to_string(),
            account_id: "bot_discord".to_string(),
            sender_id: "user_disc".to_string(),
            peer_id: None,
            parent_peer_id: None,
            guild_id: Some("guild_001".to_string()),
            role_ids: vec!["moderator".to_string()],
            team_id: None,
        };

        let route = engine.resolve(&ctx).await;
        assert_eq!(route.agent_id, "admin-agent");
        assert_eq!(route.matched_by, MatchedByType::BindingGuildRoles);
    }

    #[tokio::test]
    async fn test_binding_crud() {
        let temp_db = NamedTempFile::new().unwrap();
        let engine = RoutingEngine::new(temp_db.path(), "default-agent", SessionScope::PerPeer)
            .await
            .unwrap();

        // Add
        let binding = Binding::new(
            "test-agent",
            BindingKind::Channel {
                channel: "slack".to_string(),
            },
        );
        let binding_id = binding.id.clone();
        engine.add_binding(binding).await.unwrap();

        // Get
        let fetched = engine.get_binding(&binding_id).await;
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().agent_id, "test-agent");

        // List
        let all = engine.list_bindings(None).await;
        assert_eq!(all.len(), 1);

        let filtered = engine.list_bindings(Some("nonexistent")).await;
        assert!(filtered.is_empty());

        // Update
        let mut updated = engine.get_binding(&binding_id).await.unwrap();
        updated.agent_id = "updated-agent".to_string();
        assert!(engine.update_binding(updated).await.unwrap());

        let refetched = engine.get_binding(&binding_id).await.unwrap();
        assert_eq!(refetched.agent_id, "updated-agent");

        // Remove
        assert!(engine.remove_binding(&binding_id).await.unwrap());
        assert!(engine.get_binding(&binding_id).await.is_none());
    }

    #[tokio::test]
    async fn test_session_scope_keys() {
        let temp_db = NamedTempFile::new().unwrap();
        let engine = RoutingEngine::new(temp_db.path(), "default-agent", SessionScope::Global)
            .await
            .unwrap();

        let ctx = telegram_ctx();

        // Global scope produces a shared key.
        let route = engine.resolve(&ctx).await;
        assert_eq!(route.session_key, "global:telegram:shared");

        // Per-sender scope via a binding override.
        let temp_db2 = NamedTempFile::new().unwrap();
        let engine2 =
            RoutingEngine::new(temp_db2.path(), "default-agent", SessionScope::PerSender)
                .await
                .unwrap();

        let route2 = engine2.resolve(&ctx).await;
        assert_eq!(route2.session_key, "sender:telegram:user_456");
    }

    #[tokio::test]
    async fn test_persistence_across_reload() {
        let temp_db = NamedTempFile::new().unwrap();
        let path = temp_db.path().to_path_buf();

        // Create engine and add a binding.
        {
            let engine =
                RoutingEngine::new(&path, "default-agent", SessionScope::PerPeer)
                    .await
                    .unwrap();

            let binding = Binding::new(
                "persisted-agent",
                BindingKind::Channel {
                    channel: "telegram".to_string(),
                },
            );
            engine.add_binding(binding).await.unwrap();
        }

        // Create a fresh engine from the same database.
        let engine2 =
            RoutingEngine::new(&path, "default-agent", SessionScope::PerPeer)
                .await
                .unwrap();

        let ctx = telegram_ctx();
        let route = engine2.resolve(&ctx).await;

        assert_eq!(route.agent_id, "persisted-agent");
        assert_eq!(route.matched_by, MatchedByType::BindingChannel);
    }
}
