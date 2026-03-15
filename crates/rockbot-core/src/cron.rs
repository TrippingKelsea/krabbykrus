//! Cron system for RockBot (SPEC Section 13)
//!
//! This module provides scheduled job execution with support for one-time,
//! interval-based, and cron-expression schedules. Jobs are persisted to SQLite
//! and executed via a background tokio task.

use crate::error::{RockBotError, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cron::Schedule as CronExpression;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Cron-specific errors
#[derive(Debug, Error)]
pub enum CronError {
    #[error("Cron job not found: {job_id}")]
    NotFound { job_id: String },

    #[error("Invalid cron expression: {expression}")]
    InvalidExpression { expression: String },

    #[error("Job already exists: {job_id}")]
    AlreadyExists { job_id: String },

    #[error("Scheduler not running")]
    NotRunning,

    #[error("Cron error: {message}")]
    Other { message: String },
}

impl From<CronError> for RockBotError {
    fn from(e: CronError) -> Self {
        RockBotError::Config(crate::error::ConfigError::Invalid {
            message: e.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// The status of a job's last run
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Success,
    Failed,
    Skipped,
    Running,
}

/// How the scheduler should target a session when executing a job
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SessionTarget {
    /// Use an existing session identified by session_key
    #[default]
    Existing,
    /// Create a new ephemeral session each run
    Ephemeral,
    /// Create a new session only on first run, reuse thereafter
    Persistent,
}


/// How to wake/invoke the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum WakeMode {
    /// Inject the payload as a new user turn
    #[default]
    Turn,
    /// Fire a system event that the agent can observe
    Event,
}


/// Where to deliver results after job execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronDelivery {
    /// Channel type to deliver to (e.g., "discord", "telegram", "webhook")
    pub channel: String,
    /// Channel-specific target (e.g., channel ID, webhook URL)
    pub target: String,
    /// Extra delivery metadata
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Schedule specification for a cron job
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CronSchedule {
    /// One-time execution at a specific timestamp (milliseconds since epoch)
    At { at_ms: u64 },
    /// Repeating at a fixed interval (milliseconds)
    Every { interval_ms: u64 },
    /// Cron expression (standard 7-field cron format)
    Cron { expression: String },
}

/// The payload to deliver when a job fires
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CronPayload {
    /// Fire a system event
    SystemEvent {
        event: String,
        data: Option<serde_json::Value>,
    },
    /// Inject an agent turn
    AgentTurn {
        message: String,
        extra_system_prompt: Option<String>,
    },
}

/// Runtime state tracked per job
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct CronJobState {
    pub next_run_at_ms: Option<u64>,
    pub last_run_at_ms: Option<u64>,
    pub last_run_status: Option<RunStatus>,
    pub last_error: Option<String>,
    pub consecutive_errors: u32,
}


/// A scheduled cron job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    /// Unique job identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Optional description
    pub description: Option<String>,
    /// Whether the job is enabled
    pub enabled: bool,
    /// Agent to target (if applicable)
    pub agent_id: Option<String>,
    /// Session key for session lookup
    pub session_key: Option<String>,
    /// Schedule specification
    pub schedule: CronSchedule,
    /// Payload to deliver on trigger
    pub payload: CronPayload,
    /// How to target a session
    pub session_target: SessionTarget,
    /// How to wake the agent
    pub wake_mode: WakeMode,
    /// Optional delivery channel for results
    pub delivery: Option<CronDelivery>,
    /// Target client for remote execution. When set, the job is dispatched to
    /// a specific connected client rather than executed locally on the gateway.
    /// The value is matched against clients by UUID first (exact), then label,
    /// then hostname — using the UUID is strongly recommended for deterministic
    /// dispatch. If the target client is not connected when the job fires, the
    /// execution is skipped and an error is recorded.
    #[serde(default)]
    pub target_client: Option<String>,
    /// Runtime state
    pub state: CronJobState,
    /// When the job was created
    pub created_at: DateTime<Utc>,
    /// When the job was last modified
    pub updated_at: DateTime<Utc>,
}

impl CronJob {
    /// Create a new cron job with the given parameters
    pub fn new(
        name: impl Into<String>,
        schedule: CronSchedule,
        payload: CronPayload,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            description: None,
            enabled: true,
            agent_id: None,
            session_key: None,
            schedule,
            payload,
            session_target: SessionTarget::default(),
            wake_mode: WakeMode::default(),
            delivery: None,
            target_client: None,
            state: CronJobState::default(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Compute the next run time from `now_ms` based on the schedule.
    /// Returns `None` if the job should not run again (e.g., one-shot already fired).
    pub fn compute_next_run(&self, now_ms: u64) -> Option<u64> {
        match &self.schedule {
            CronSchedule::At { at_ms } => {
                if self.state.last_run_at_ms.is_some() {
                    // One-shot already fired
                    None
                } else if *at_ms > now_ms {
                    Some(*at_ms)
                } else {
                    // Overdue — run immediately
                    Some(now_ms)
                }
            }
            CronSchedule::Every { interval_ms } => {
                let base = self
                    .state
                    .last_run_at_ms
                    .unwrap_or(now_ms);
                let next = base + interval_ms;
                if next <= now_ms {
                    Some(now_ms)
                } else {
                    Some(next)
                }
            }
            CronSchedule::Cron { expression } => {
                let schedule = CronExpression::from_str(expression).ok()?;
                let now_dt = DateTime::from_timestamp_millis(now_ms as i64)?;
                schedule
                    .after(&now_dt)
                    .next()
                    .map(|dt| dt.timestamp_millis() as u64)
            }
        }
    }

    /// Check whether this job is due to run at `now_ms`.
    pub fn is_due(&self, now_ms: u64) -> bool {
        if !self.enabled {
            return false;
        }
        match self.state.next_run_at_ms {
            Some(next) => now_ms >= next,
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Executor trait — implemented by the gateway / runtime
// ---------------------------------------------------------------------------

/// Trait that the host application implements to actually execute cron payloads.
#[async_trait]
pub trait CronExecutor: Send + Sync + 'static {
    /// Execute a cron job. Returns `Ok(())` on success.
    async fn execute(&self, job: &CronJob) -> std::result::Result<(), String>;
}

// ---------------------------------------------------------------------------
// CronScheduler
// ---------------------------------------------------------------------------

/// Commands sent to the scheduler background task
enum SchedulerCommand {
    /// Trigger a specific job immediately
    TriggerNow { job_id: String },
    /// Shutdown the scheduler
    Shutdown,
}

/// The main cron scheduler that manages jobs, persists them, and runs a
/// background tick loop.
pub struct CronScheduler {
    /// In-memory job store
    jobs: Arc<RwLock<HashMap<String, CronJob>>>,
    /// SQLite persistence
    db: Arc<Mutex<Connection>>,
    /// Channel to send commands to the background task (interior mutability for Arc compatibility)
    cmd_tx: Arc<tokio::sync::Mutex<Option<mpsc::Sender<SchedulerCommand>>>>,
    /// Tick interval for checking due jobs
    tick_interval: Duration,
}

impl CronScheduler {
    /// Create a new scheduler with SQLite persistence at `db_path`.
    pub async fn new<P: AsRef<Path>>(db_path: P) -> Result<Self> {
        let path = db_path.as_ref();
        let db = Connection::open(path)?;

        db.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS cron_jobs (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                agent_id TEXT,
                session_key TEXT,
                schedule TEXT NOT NULL,       -- JSON
                payload TEXT NOT NULL,        -- JSON
                session_target TEXT NOT NULL,  -- JSON
                wake_mode TEXT NOT NULL,       -- JSON
                delivery TEXT,                -- JSON
                target_client TEXT,
                next_run_at_ms INTEGER,
                last_run_at_ms INTEGER,
                last_run_status TEXT,
                last_error TEXT,
                consecutive_errors INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_cron_jobs_enabled ON cron_jobs(enabled);
            CREATE INDEX IF NOT EXISTS idx_cron_jobs_next_run ON cron_jobs(next_run_at_ms);

            -- Migration: add target_client column if missing (for existing DBs)
            -- SQLite silently ignores duplicate ADD COLUMN, so this is safe.
            "#,
        )?;

        // Best-effort migration for existing databases (ignore errors if column exists)
        let _ = db.execute_batch(
            "ALTER TABLE cron_jobs ADD COLUMN target_client TEXT;",
        );

        info!("Cron scheduler initialized with database at {:?}", path);

        // Load existing jobs from DB
        let jobs = Self::load_jobs_from_db(&db)?;

        Ok(Self {
            jobs: Arc::new(RwLock::new(jobs)),
            db: Arc::new(Mutex::new(db)),
            cmd_tx: Arc::new(tokio::sync::Mutex::new(None)),
            tick_interval: Duration::from_secs(1),
        })
    }

    /// Set the tick interval (how often the scheduler checks for due jobs).
    pub fn with_tick_interval(mut self, interval: Duration) -> Self {
        self.tick_interval = interval;
        self
    }

    /// Start the background scheduler loop. Must provide an executor.
    /// Can be called on `&self` (or `Arc<Self>`) — uses interior mutability.
    pub async fn start(&self, executor: Arc<dyn CronExecutor>) {
        let (cmd_tx, cmd_rx) = mpsc::channel::<SchedulerCommand>(64);
        {
            let mut tx_guard = self.cmd_tx.lock().await;
            *tx_guard = Some(cmd_tx);
        }

        let jobs = Arc::clone(&self.jobs);
        let db = Arc::clone(&self.db);
        let tick_interval = self.tick_interval;

        tokio::spawn(async move {
            Self::run_loop(jobs, db, executor, cmd_rx, tick_interval).await;
        });

        info!("Cron scheduler background task started");
    }

    /// The main scheduler loop
    async fn run_loop(
        jobs: Arc<RwLock<HashMap<String, CronJob>>>,
        db: Arc<Mutex<Connection>>,
        executor: Arc<dyn CronExecutor>,
        mut cmd_rx: mpsc::Receiver<SchedulerCommand>,
        tick_interval: Duration,
    ) {
        let mut interval = tokio::time::interval(tick_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    Self::tick(&jobs, &db, &executor).await;
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(SchedulerCommand::TriggerNow { job_id }) => {
                            Self::trigger_job(&jobs, &db, &executor, &job_id).await;
                        }
                        Some(SchedulerCommand::Shutdown) | None => {
                            info!("Cron scheduler shutting down");
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Single tick: find and execute all due jobs
    async fn tick(
        jobs: &Arc<RwLock<HashMap<String, CronJob>>>,
        db: &Arc<Mutex<Connection>>,
        executor: &Arc<dyn CronExecutor>,
    ) {
        let now_ms = Utc::now().timestamp_millis() as u64;

        // Collect IDs of due jobs (avoid holding lock during execution)
        let due_ids: Vec<String> = {
            let jobs_read = jobs.read().await;
            jobs_read
                .values()
                .filter(|j| j.is_due(now_ms))
                .map(|j| j.id.clone())
                .collect()
        };

        for job_id in due_ids {
            Self::trigger_job(jobs, db, executor, &job_id).await;
        }
    }

    /// Execute a single job by ID
    async fn trigger_job(
        jobs: &Arc<RwLock<HashMap<String, CronJob>>>,
        db: &Arc<Mutex<Connection>>,
        executor: &Arc<dyn CronExecutor>,
        job_id: &str,
    ) {
        let job = {
            let jobs_read = jobs.read().await;
            if let Some(j) = jobs_read.get(job_id) { j.clone() } else {
                warn!("Cron job {} not found for execution", job_id);
                return;
            }
        };

        debug!("Executing cron job '{}' ({})", job.name, job.id);

        let now_ms = Utc::now().timestamp_millis() as u64;
        let result = executor.execute(&job).await;

        // Update state
        {
            let mut jobs_write = jobs.write().await;
            if let Some(j) = jobs_write.get_mut(job_id) {
                j.state.last_run_at_ms = Some(now_ms);
                j.updated_at = Utc::now();

                match result {
                    Ok(()) => {
                        j.state.last_run_status = Some(RunStatus::Success);
                        j.state.last_error = None;
                        j.state.consecutive_errors = 0;
                        info!("Cron job '{}' completed successfully", j.name);
                    }
                    Err(e) => {
                        j.state.last_run_status = Some(RunStatus::Failed);
                        j.state.last_error = Some(e.clone());
                        j.state.consecutive_errors += 1;
                        error!(
                            "Cron job '{}' failed (consecutive: {}): {}",
                            j.name, j.state.consecutive_errors, e
                        );
                    }
                }

                // Compute next run
                j.state.next_run_at_ms = j.compute_next_run(now_ms);

                // Disable one-shot jobs that have completed
                if matches!(j.schedule, CronSchedule::At { .. }) && j.state.next_run_at_ms.is_none()
                {
                    j.enabled = false;
                }

                // Persist state update
                let job_clone = j.clone();
                drop(jobs_write);
                if let Err(e) = Self::persist_job_state(db, &job_clone).await {
                    error!("Failed to persist cron job state: {}", e);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // CRUD operations
    // -----------------------------------------------------------------------

    /// Add a new cron job
    pub async fn add_job(&self, mut job: CronJob) -> Result<CronJob> {
        // Validate cron expression if applicable
        if let CronSchedule::Cron { ref expression } = job.schedule {
            CronExpression::from_str(expression).map_err(|_| CronError::InvalidExpression {
                expression: expression.clone(),
            })?;
        }

        // Compute initial next_run
        let now_ms = Utc::now().timestamp_millis() as u64;
        job.state.next_run_at_ms = job.compute_next_run(now_ms);

        // Persist to DB
        {
            let db = self.db.lock().await;
            db.execute(
                "INSERT INTO cron_jobs (
                    id, name, description, enabled, agent_id, session_key,
                    schedule, payload, session_target, wake_mode, delivery,
                    target_client, next_run_at_ms, last_run_at_ms, last_run_status,
                    last_error, consecutive_errors, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                          ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                params![
                    &job.id,
                    &job.name,
                    &job.description,
                    job.enabled as i32,
                    &job.agent_id,
                    &job.session_key,
                    serde_json::to_string(&job.schedule)?,
                    serde_json::to_string(&job.payload)?,
                    serde_json::to_string(&job.session_target)?,
                    serde_json::to_string(&job.wake_mode)?,
                    job.delivery
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?,
                    &job.target_client,
                    job.state.next_run_at_ms.map(|v| v as i64),
                    job.state.last_run_at_ms.map(|v| v as i64),
                    job.state
                        .last_run_status
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?,
                    &job.state.last_error,
                    job.state.consecutive_errors,
                    job.created_at.timestamp(),
                    job.updated_at.timestamp(),
                ],
            )?;
        }

        // Add to in-memory store
        {
            let mut jobs = self.jobs.write().await;
            jobs.insert(job.id.clone(), job.clone());
        }

        info!("Added cron job '{}' ({})", job.name, job.id);
        Ok(job)
    }

    /// Update an existing cron job (full replacement of mutable fields)
    pub async fn update_job(&self, job: CronJob) -> Result<CronJob> {
        // Validate cron expression if applicable
        if let CronSchedule::Cron { ref expression } = job.schedule {
            CronExpression::from_str(expression).map_err(|_| CronError::InvalidExpression {
                expression: expression.clone(),
            })?;
        }

        let mut job = job;
        job.updated_at = Utc::now();

        // Recompute next_run
        let now_ms = Utc::now().timestamp_millis() as u64;
        job.state.next_run_at_ms = job.compute_next_run(now_ms);

        // Persist
        {
            let db = self.db.lock().await;
            let affected = db.execute(
                "UPDATE cron_jobs SET
                    name = ?1, description = ?2, enabled = ?3, agent_id = ?4,
                    session_key = ?5, schedule = ?6, payload = ?7,
                    session_target = ?8, wake_mode = ?9, delivery = ?10,
                    target_client = ?11, next_run_at_ms = ?12, last_run_at_ms = ?13,
                    last_run_status = ?14, last_error = ?15,
                    consecutive_errors = ?16, updated_at = ?17
                 WHERE id = ?18",
                params![
                    &job.name,
                    &job.description,
                    job.enabled as i32,
                    &job.agent_id,
                    &job.session_key,
                    serde_json::to_string(&job.schedule)?,
                    serde_json::to_string(&job.payload)?,
                    serde_json::to_string(&job.session_target)?,
                    serde_json::to_string(&job.wake_mode)?,
                    job.delivery
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?,
                    &job.target_client,
                    job.state.next_run_at_ms.map(|v| v as i64),
                    job.state.last_run_at_ms.map(|v| v as i64),
                    job.state
                        .last_run_status
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?,
                    &job.state.last_error,
                    job.state.consecutive_errors,
                    job.updated_at.timestamp(),
                    &job.id,
                ],
            )?;

            if affected == 0 {
                return Err(CronError::NotFound {
                    job_id: job.id.clone(),
                }
                .into());
            }
        }

        // Update in-memory
        {
            let mut jobs = self.jobs.write().await;
            jobs.insert(job.id.clone(), job.clone());
        }

        info!("Updated cron job '{}' ({})", job.name, job.id);
        Ok(job)
    }

    /// Remove a cron job by ID
    pub async fn remove_job(&self, job_id: &str) -> Result<()> {
        {
            let db = self.db.lock().await;
            let affected = db.execute("DELETE FROM cron_jobs WHERE id = ?1", params![job_id])?;
            if affected == 0 {
                return Err(CronError::NotFound {
                    job_id: job_id.to_string(),
                }
                .into());
            }
        }

        {
            let mut jobs = self.jobs.write().await;
            jobs.remove(job_id);
        }

        info!("Removed cron job {}", job_id);
        Ok(())
    }

    /// Get a single job by ID
    pub async fn get_job(&self, job_id: &str) -> Result<Option<CronJob>> {
        let jobs = self.jobs.read().await;
        Ok(jobs.get(job_id).cloned())
    }

    /// List all jobs, optionally filtering by enabled status
    pub async fn list_jobs(&self, enabled_only: bool) -> Vec<CronJob> {
        let jobs = self.jobs.read().await;
        jobs.values()
            .filter(|j| !enabled_only || j.enabled)
            .cloned()
            .collect()
    }

    /// Trigger a job to run immediately (outside its schedule)
    pub async fn trigger_now(&self, job_id: &str) -> Result<()> {
        // Verify job exists
        {
            let jobs = self.jobs.read().await;
            if !jobs.contains_key(job_id) {
                return Err(CronError::NotFound {
                    job_id: job_id.to_string(),
                }
                .into());
            }
        }

        let tx_guard = self.cmd_tx.lock().await;
        if let Some(ref tx) = *tx_guard {
            tx.send(SchedulerCommand::TriggerNow {
                job_id: job_id.to_string(),
            })
            .await
            .map_err(|_| CronError::NotRunning)?;
        } else {
            return Err(CronError::NotRunning.into());
        }

        Ok(())
    }

    /// Gracefully shut down the scheduler
    pub async fn shutdown(&self) {
        let tx_guard = self.cmd_tx.lock().await;
        if let Some(ref tx) = *tx_guard {
            let _ = tx.send(SchedulerCommand::Shutdown).await;
        }
    }

    // -----------------------------------------------------------------------
    // Persistence helpers
    // -----------------------------------------------------------------------

    /// Persist only the runtime state fields of a job
    async fn persist_job_state(db: &Arc<Mutex<Connection>>, job: &CronJob) -> Result<()> {
        let db = db.lock().await;
        db.execute(
            "UPDATE cron_jobs SET
                enabled = ?1, next_run_at_ms = ?2, last_run_at_ms = ?3,
                last_run_status = ?4, last_error = ?5, consecutive_errors = ?6,
                updated_at = ?7
             WHERE id = ?8",
            params![
                job.enabled as i32,
                job.state.next_run_at_ms.map(|v| v as i64),
                job.state.last_run_at_ms.map(|v| v as i64),
                job.state
                    .last_run_status
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                &job.state.last_error,
                job.state.consecutive_errors,
                job.updated_at.timestamp(),
                &job.id,
            ],
        )?;
        Ok(())
    }

    /// Load all jobs from the database into memory
    fn load_jobs_from_db(db: &Connection) -> Result<HashMap<String, CronJob>> {
        let mut stmt = db.prepare(
            "SELECT id, name, description, enabled, agent_id, session_key,
                    schedule, payload, session_target, wake_mode, delivery,
                    target_client, next_run_at_ms, last_run_at_ms, last_run_status,
                    last_error, consecutive_errors, created_at, updated_at
             FROM cron_jobs",
        )?;

        let rows = stmt.query_map([], |row| {
            let schedule_str: String = row.get(6)?;
            let payload_str: String = row.get(7)?;
            let session_target_str: String = row.get(8)?;
            let wake_mode_str: String = row.get(9)?;
            let delivery_str: Option<String> = row.get(10)?;
            let target_client: Option<String> = row.get(11)?;
            let last_run_status_str: Option<String> = row.get(14)?;

            let created_at = DateTime::from_timestamp(row.get::<_, i64>(17)?, 0)
                .unwrap_or_else(Utc::now);
            let updated_at = DateTime::from_timestamp(row.get::<_, i64>(18)?, 0)
                .unwrap_or_else(Utc::now);

            Ok(CronJob {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                enabled: row.get::<_, i32>(3)? != 0,
                agent_id: row.get(4)?,
                session_key: row.get(5)?,
                schedule: serde_json::from_str(&schedule_str).unwrap_or(CronSchedule::Every {
                    interval_ms: 60_000,
                }),
                payload: serde_json::from_str(&payload_str).unwrap_or(
                    CronPayload::SystemEvent {
                        event: "unknown".into(),
                        data: None,
                    },
                ),
                session_target: serde_json::from_str(&session_target_str)
                    .unwrap_or_default(),
                wake_mode: serde_json::from_str(&wake_mode_str).unwrap_or_default(),
                delivery: delivery_str
                    .and_then(|s| serde_json::from_str(&s).ok()),
                target_client,
                state: CronJobState {
                    next_run_at_ms: row
                        .get::<_, Option<i64>>(12)?
                        .map(|v| v as u64),
                    last_run_at_ms: row
                        .get::<_, Option<i64>>(13)?
                        .map(|v| v as u64),
                    last_run_status: last_run_status_str
                        .and_then(|s| serde_json::from_str(&s).ok()),
                    last_error: row.get(15)?,
                    consecutive_errors: row.get::<_, u32>(16).unwrap_or(0),
                },
                created_at,
                updated_at,
            })
        })?;

        let mut jobs = HashMap::new();
        for row_result in rows {
            match row_result {
                Ok(job) => {
                    jobs.insert(job.id.clone(), job);
                }
                Err(e) => {
                    warn!("Failed to load cron job from DB: {}", e);
                }
            }
        }

        info!("Loaded {} cron jobs from database", jobs.len());
        Ok(jobs)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_compute_next_run_at() {
        let job = CronJob::new(
            "test",
            CronSchedule::At { at_ms: 5000 },
            CronPayload::SystemEvent {
                event: "test".into(),
                data: None,
            },
        );

        // Before the target time
        assert_eq!(job.compute_next_run(1000), Some(5000));

        // After the target time — run immediately
        assert_eq!(job.compute_next_run(6000), Some(6000));

        // Already ran — no more runs
        let mut fired = job.clone();
        fired.state.last_run_at_ms = Some(5000);
        assert_eq!(fired.compute_next_run(6000), None);
    }

    #[test]
    fn test_compute_next_run_every() {
        let mut job = CronJob::new(
            "test",
            CronSchedule::Every { interval_ms: 1000 },
            CronPayload::SystemEvent {
                event: "tick".into(),
                data: None,
            },
        );

        // First run — base is now
        let next = job.compute_next_run(10_000);
        assert_eq!(next, Some(11_000));

        // After a run
        job.state.last_run_at_ms = Some(10_000);
        assert_eq!(job.compute_next_run(10_500), Some(11_000));

        // Overdue
        assert_eq!(job.compute_next_run(12_000), Some(12_000));
    }

    #[test]
    fn test_is_due() {
        let mut job = CronJob::new(
            "test",
            CronSchedule::Every { interval_ms: 1000 },
            CronPayload::SystemEvent {
                event: "tick".into(),
                data: None,
            },
        );
        job.state.next_run_at_ms = Some(5000);

        assert!(!job.is_due(4999));
        assert!(job.is_due(5000));
        assert!(job.is_due(5001));

        // Disabled
        job.enabled = false;
        assert!(!job.is_due(5001));
    }

    #[test]
    fn test_cron_expression_validation() {
        // Valid 7-field cron
        let result = CronExpression::from_str("0 0 * * * * *");
        assert!(result.is_ok());

        // Invalid
        let result = CronExpression::from_str("not a cron");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_scheduler_crud() {
        let temp_db = NamedTempFile::new().unwrap();
        let scheduler = CronScheduler::new(temp_db.path()).await.unwrap();

        // Add
        let job = CronJob::new(
            "test-job",
            CronSchedule::Every { interval_ms: 60_000 },
            CronPayload::AgentTurn {
                message: "Hello".into(),
                extra_system_prompt: None,
            },
        );
        let job_id = job.id.clone();
        let added = scheduler.add_job(job).await.unwrap();
        assert_eq!(added.name, "test-job");

        // Get
        let fetched = scheduler.get_job(&job_id).await.unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().name, "test-job");

        // List
        let all = scheduler.list_jobs(false).await;
        assert_eq!(all.len(), 1);

        // Update
        let mut updated = added.clone();
        updated.name = "updated-job".into();
        let result = scheduler.update_job(updated).await.unwrap();
        assert_eq!(result.name, "updated-job");

        // Remove
        scheduler.remove_job(&job_id).await.unwrap();
        let gone = scheduler.get_job(&job_id).await.unwrap();
        assert!(gone.is_none());
    }

    #[tokio::test]
    async fn test_scheduler_persistence() {
        let temp_db = NamedTempFile::new().unwrap();
        let path = temp_db.path().to_path_buf();

        // Create and add a job
        {
            let scheduler = CronScheduler::new(&path).await.unwrap();
            let job = CronJob::new(
                "persist-test",
                CronSchedule::Every { interval_ms: 30_000 },
                CronPayload::SystemEvent {
                    event: "heartbeat".into(),
                    data: None,
                },
            );
            scheduler.add_job(job).await.unwrap();
        }

        // Re-open and verify it was loaded
        {
            let scheduler = CronScheduler::new(&path).await.unwrap();
            let jobs = scheduler.list_jobs(false).await;
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].name, "persist-test");
        }
    }
}
