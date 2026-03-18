//! Cron system for RockBot (SPEC Section 13)
//!
//! This module provides scheduled job execution with support for one-time,
//! interval-based, and cron-expression schedules. Jobs are persisted to redb
//! and executed via a background tokio task.

use crate::error::{Result, RockBotError};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cron::Schedule as CronExpression;
use rockbot_storage::{tables, Store};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, RwLock};
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    pub fn new(name: impl Into<String>, schedule: CronSchedule, payload: CronPayload) -> Self {
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
                let base = self.state.last_run_at_ms.unwrap_or(now_ms);
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
    /// redb persistence
    store: Arc<Store>,
    /// Channel to send commands to the background task (interior mutability for Arc compatibility)
    cmd_tx: Arc<tokio::sync::Mutex<Option<mpsc::Sender<SchedulerCommand>>>>,
    /// Tick interval for checking due jobs
    tick_interval: Duration,
}

impl CronScheduler {
    /// Create a new scheduler with redb persistence at `db_path`.
    pub async fn new<P: AsRef<Path>>(db_path: P) -> Result<Self> {
        Self::new_with_key(db_path, None).await
    }

    /// Create a new scheduler with an optional node-local storage key.
    pub async fn new_with_key<P: AsRef<Path>>(db_path: P, key: Option<[u8; 32]>) -> Result<Self> {
        let path = if db_path.as_ref() == Path::new(":memory:") {
            std::env::temp_dir().join(format!("rockbot-cron-{}.redb", Uuid::new_v4()))
        } else {
            db_path.as_ref().to_path_buf()
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let encrypted = key.is_some();
        let store = Arc::new(
            Store::open_with_optional_key(&path, key).map_err(crate::error::RockBotError::from)?,
        );

        info!(
            "Cron scheduler initialized with {} database at {:?}",
            if encrypted { "encrypted" } else { "plaintext" },
            path
        );

        let jobs = Self::load_jobs_from_store(&store)?;

        Ok(Self {
            jobs: Arc::new(RwLock::new(jobs)),
            store,
            cmd_tx: Arc::new(tokio::sync::Mutex::new(None)),
            tick_interval: Duration::from_secs(1),
        })
    }

    pub async fn new_with_store(store: Arc<Store>, descriptor: &str) -> Result<Self> {
        info!("Cron scheduler initialized with {descriptor}");
        let jobs = Self::load_jobs_from_store(&store)?;
        Ok(Self {
            jobs: Arc::new(RwLock::new(jobs)),
            store,
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
        let store = Arc::clone(&self.store);
        let tick_interval = self.tick_interval;

        tokio::spawn(async move {
            Self::run_loop(jobs, store, executor, cmd_rx, tick_interval).await;
        });

        info!("Cron scheduler background task started");
    }

    /// The main scheduler loop
    async fn run_loop(
        jobs: Arc<RwLock<HashMap<String, CronJob>>>,
        store: Arc<Store>,
        executor: Arc<dyn CronExecutor>,
        mut cmd_rx: mpsc::Receiver<SchedulerCommand>,
        tick_interval: Duration,
    ) {
        let mut interval = tokio::time::interval(tick_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    Self::tick(&jobs, &store, &executor).await;
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(SchedulerCommand::TriggerNow { job_id }) => {
                            Self::trigger_job(&jobs, &store, &executor, &job_id).await;
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
        store: &Arc<Store>,
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
            Self::trigger_job(jobs, store, executor, &job_id).await;
        }
    }

    /// Execute a single job by ID
    async fn trigger_job(
        jobs: &Arc<RwLock<HashMap<String, CronJob>>>,
        store: &Arc<Store>,
        executor: &Arc<dyn CronExecutor>,
        job_id: &str,
    ) {
        let job = {
            let jobs_read = jobs.read().await;
            if let Some(j) = jobs_read.get(job_id) {
                j.clone()
            } else {
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
                if let Err(e) = Self::persist_job_state(store, &job_clone).await {
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

        self.persist_job(&job)?;

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

        if self.store.get(tables::CRON_JOBS, &job.id)?.is_none() {
            return Err(CronError::NotFound {
                job_id: job.id.clone(),
            }
            .into());
        }
        self.persist_job(&job)?;

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
        if !self.store.delete(tables::CRON_JOBS, job_id)? {
            return Err(CronError::NotFound {
                job_id: job_id.to_string(),
            }
            .into());
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
    fn persist_job(&self, job: &CronJob) -> Result<()> {
        let bytes = serde_json::to_vec(job)?;
        self.store.put(tables::CRON_JOBS, &job.id, &bytes)?;
        Ok(())
    }

    async fn persist_job_state(store: &Arc<Store>, job: &CronJob) -> Result<()> {
        let bytes = serde_json::to_vec(job)?;
        store.put(tables::CRON_JOBS, &job.id, &bytes)?;
        Ok(())
    }

    /// Load all jobs from the store into memory
    fn load_jobs_from_store(store: &Store) -> Result<HashMap<String, CronJob>> {
        let mut jobs = HashMap::new();
        for (_, bytes) in store.list(tables::CRON_JOBS)? {
            match serde_json::from_slice::<CronJob>(&bytes) {
                Ok(job) => {
                    jobs.insert(job.id.clone(), job);
                }
                Err(e) => {
                    warn!("Failed to load cron job from store: {}", e);
                }
            }
        }

        info!("Loaded {} cron jobs from store", jobs.len());
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
            CronSchedule::Every {
                interval_ms: 60_000,
            },
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
                CronSchedule::Every {
                    interval_ms: 30_000,
                },
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
