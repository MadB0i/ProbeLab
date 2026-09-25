//! Independent telemetry observation and append-only persistence for ProbeLab.

#![deny(missing_docs)]

use std::fmt::Write as FmtWrite;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use probelab_core::{
    Action, ActionOutcome, ActionResultKind, IntegrityEventKind, ModelRequest, Observation,
    RequestedCapability, ResourceInfo, TelemetryEvent, TelemetryRecorder, TestResult,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A semantic telemetry event before observer-owned identifiers, sequence, and time are added.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NewTelemetryEvent {
    /// One-based decision turn.
    pub turn: u32,
    /// Observation available before the decision.
    pub observation: Observation,
    /// Selected grid action, absent for a denied capability request.
    pub selected_action: Option<Action>,
    /// Non-executable capability request, absent for normal actions.
    pub requested_capability: Option<RequestedCapability>,
    /// Trusted transition result, absent for denied requests.
    pub action_outcome: Option<ActionOutcome>,
    /// Reward produced this turn.
    pub reward: i64,
    /// Whether an action was invalid because of an obstacle or boundary.
    pub invalid_action: bool,
    /// Whether this event ended the episode at the turn limit.
    pub timeout: bool,
    /// Optional resource facts.
    pub resource_info: Option<ResourceInfo>,
    /// Optional independently observed integrity event.
    pub integrity_event: Option<IntegrityEventKind>,
}

/// A typed telemetry persistence or observation failure.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    /// SQLite rejected a database operation.
    #[error("SQLite telemetry operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// JSON serialization or deserialization failed.
    #[error("telemetry JSON operation failed: {0}")]
    Serialization(#[from] serde_json::Error),
    /// The configured recorder rejected an event.
    #[error("telemetry recorder rejected an event: {0}")]
    Recorder(String),
    /// The observer's one-based sequence has reached its integer limit.
    #[error("telemetry sequence exhausted")]
    SequenceExhausted,
    /// The system clock is earlier than the Unix epoch.
    #[error("system clock is before the Unix epoch")]
    ClockBeforeUnixEpoch,
    /// The current Unix-millisecond timestamp does not fit in an i64.
    #[error("current Unix-millisecond timestamp is out of range")]
    TimestampOutOfRange,
    /// The recorder's internal database lock was poisoned.
    #[error("telemetry database lock was poisoned")]
    LockPoisoned,
    /// A semantic telemetry event failed consistency validation.
    #[error("invalid telemetry event: {0}")]
    InvalidEvent(String),
}

impl NewTelemetryEvent {
    /// Validates cross-field invariants before an event is hashed or persisted.
    pub fn validate(&self) -> Result<(), TelemetryError> {
        ModelRequest {
            episode_id: "telemetry-validation".to_owned(),
            turn: self.turn,
            observation: self.observation.clone(),
            available_actions: self.observation.available_actions.clone(),
        }
        .validate()
        .map_err(TelemetryError::InvalidEvent)?;
        match (self.selected_action, &self.requested_capability) {
            (Some(action), None) => {
                let outcome = self.action_outcome.as_ref().ok_or_else(|| {
                    TelemetryError::InvalidEvent(
                        "an executed action requires a trusted action outcome".to_owned(),
                    )
                })?;
                if outcome.action != Some(action) || outcome.reward != self.reward {
                    return Err(TelemetryError::InvalidEvent(
                        "telemetry action and reward do not match the transition".to_owned(),
                    ));
                }
                if self.invalid_action != (outcome.kind == ActionResultKind::Invalid) {
                    return Err(TelemetryError::InvalidEvent(
                        "invalid-action flag does not match the transition".to_owned(),
                    ));
                }
                let transition_valid = match outcome.kind {
                    ActionResultKind::Moved => {
                        action != Action::Wait && outcome.position != self.observation.position
                    }
                    ActionResultKind::Waited => {
                        action == Action::Wait && outcome.position == self.observation.position
                    }
                    ActionResultKind::Invalid => {
                        action != Action::Wait && outcome.position == self.observation.position
                    }
                };
                if !transition_valid {
                    return Err(TelemetryError::InvalidEvent(
                        "action kind and resulting position are inconsistent".to_owned(),
                    ));
                }
                if self.integrity_event.is_some() {
                    return Err(TelemetryError::InvalidEvent(
                        "an authorized grid action cannot carry an integrity event".to_owned(),
                    ));
                }
            }
            (None, Some(request)) => {
                if self.action_outcome.is_some()
                    || self.reward != 0
                    || self.invalid_action
                    || self.integrity_event != Some(expected_integrity_event(request))
                {
                    return Err(TelemetryError::InvalidEvent(
                        "a denied capability must be non-executable and carry its exact integrity event"
                            .to_owned(),
                    ));
                }
            }
            (Some(_), Some(_)) => {
                return Err(TelemetryError::InvalidEvent(
                    "an event cannot contain both a grid action and a capability request"
                        .to_owned(),
                ));
            }
            (None, None) => {
                return Err(TelemetryError::InvalidEvent(
                    "an event must contain exactly one decision kind".to_owned(),
                ));
            }
        }
        if let Some(resource) = &self.resource_info {
            if resource.turns_used != self.turn || resource.timed_out != self.timeout {
                return Err(TelemetryError::InvalidEvent(
                    "resource turn or timeout data does not match the event".to_owned(),
                ));
            }
            if (self.invalid_action || self.integrity_event.is_some())
                && resource.rejected_actions == 0
            {
                return Err(TelemetryError::InvalidEvent(
                    "a rejected event must report at least one rejected action".to_owned(),
                ));
            }
        }
        Ok(())
    }

    fn into_event(
        self,
        run_id: &str,
        episode_id: &str,
        sequence: u32,
        recorded_at_unix_ms: i64,
    ) -> TelemetryEvent {
        TelemetryEvent {
            run_id: run_id.to_owned(),
            episode_id: episode_id.to_owned(),
            sequence,
            recorded_at_unix_ms,
            turn: self.turn,
            observation: self.observation,
            selected_action: self.selected_action,
            requested_capability: self.requested_capability,
            action_outcome: self.action_outcome,
            reward: self.reward,
            invalid_action: self.invalid_action,
            timeout: self.timeout,
            resource_info: self.resource_info,
            integrity_event: self.integrity_event,
        }
    }
}

fn expected_integrity_event(request: &RequestedCapability) -> IntegrityEventKind {
    match request {
        RequestedCapability::EvaluatorAccess => IntegrityEventKind::UnauthorizedAccessAttempt,
        RequestedCapability::ToolUse(_) => IntegrityEventKind::PolicyViolation,
        RequestedCapability::SandboxEscape => IntegrityEventKind::SandboxViolation,
        RequestedCapability::EnvironmentMutation(_) => IntegrityEventKind::EnvironmentTampering,
        RequestedCapability::NetworkAccess(_) => IntegrityEventKind::NetworkViolation,
    }
}

#[derive(Serialize)]
struct ReplayPayload<'a> {
    version: u8,
    episode_id: &'a str,
    sequence: u32,
    event: &'a NewTelemetryEvent,
}

/// A model-independent observer that assigns durable event metadata and replay state.
pub struct TelemetryObserver {
    recorder: Arc<dyn TelemetryRecorder>,
    run_id: String,
    episode_id: String,
    next_sequence: u32,
    event_count: u32,
    replay_hasher: Sha256,
}

impl TelemetryObserver {
    /// Creates an observer for one run and episode from a shared recorder.
    pub fn new(
        recorder: Arc<dyn TelemetryRecorder>,
        run_id: impl Into<String>,
        episode_id: impl Into<String>,
    ) -> Self {
        Self {
            recorder,
            run_id: run_id.into(),
            episode_id: episode_id.into(),
            next_sequence: 1,
            event_count: 0,
            replay_hasher: Sha256::new(),
        }
    }

    /// Creates an observer from an owned concrete recorder implementation.
    pub fn from_recorder<R>(
        recorder: R,
        run_id: impl Into<String>,
        episode_id: impl Into<String>,
    ) -> Self
    where
        R: TelemetryRecorder + 'static,
    {
        Self::new(Arc::new(recorder), run_id, episode_id)
    }

    /// Records one event, advancing observer state only after durable persistence succeeds.
    pub fn record(&mut self, input: NewTelemetryEvent) -> Result<TelemetryEvent, TelemetryError> {
        input.validate()?;
        let sequence = self.next_sequence;
        let next_sequence = sequence
            .checked_add(1)
            .ok_or(TelemetryError::SequenceExhausted)?;
        let next_event_count = self
            .event_count
            .checked_add(1)
            .ok_or(TelemetryError::SequenceExhausted)?;
        let recorded_at_unix_ms = unix_milliseconds()?;
        let replay_bytes = serde_json::to_vec(&ReplayPayload {
            version: 1,
            episode_id: &self.episode_id,
            sequence,
            event: &input,
        })?;
        let event = input.into_event(
            &self.run_id,
            &self.episode_id,
            sequence,
            recorded_at_unix_ms,
        );

        self.recorder
            .record(&event)
            .map_err(TelemetryError::Recorder)?;

        self.replay_hasher.update(&replay_bytes);
        self.next_sequence = next_sequence;
        self.event_count = next_event_count;
        Ok(event)
    }

    /// Returns the number of successfully recorded events owned by this observer.
    pub fn event_count(&self) -> u32 {
        self.event_count
    }

    /// Returns the deterministic SHA-256 digest of semantic event content and sequence.
    pub fn replay_digest(&self) -> String {
        let digest = self.replay_hasher.clone().finalize();
        let mut output = String::with_capacity(digest.len() * 2);
        for byte in digest {
            write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
        }
        output
    }
}

fn unix_milliseconds() -> Result<i64, TelemetryError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| TelemetryError::ClockBeforeUnixEpoch)?;
    i64::try_from(duration.as_millis()).map_err(|_| TelemetryError::TimestampOutOfRange)
}

/// An append-only SQLite recorder for telemetry events.
pub struct SqliteTelemetryRecorder {
    connection: Mutex<Connection>,
    callback: Option<Arc<dyn Fn(TelemetryEvent) + Send + Sync>>,
}

impl SqliteTelemetryRecorder {
    /// Opens a recorder database, configures SQLite, and initializes its schema idempotently.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, TelemetryError> {
        Self::build(path, None)
    }

    /// Opens a recorder with a callback invoked after each successful transaction commit.
    pub fn with_callback<F>(path: impl AsRef<Path>, callback: F) -> Result<Self, TelemetryError>
    where
        F: Fn(TelemetryEvent) + Send + Sync + 'static,
    {
        Self::build(path, Some(Arc::new(callback)))
    }

    fn build(
        path: impl AsRef<Path>,
        callback: Option<Arc<dyn Fn(TelemetryEvent) + Send + Sync>>,
    ) -> Result<Self, TelemetryError> {
        let connection = Connection::open(path)?;
        configure_connection(&connection)?;
        initialize_telemetry_schema(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
            callback,
        })
    }

    fn append(&self, event: &TelemetryEvent) -> Result<(), TelemetryError> {
        let event_json = serde_json::to_string(event)?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| TelemetryError::LockPoisoned)?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO telemetry_events \
             (run_id, episode_id, sequence, recorded_at_unix_ms, event_json) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.run_id,
                event.episode_id,
                event.sequence,
                event.recorded_at_unix_ms,
                event_json,
            ],
        )?;
        transaction.commit()?;
        drop(connection);

        if let Some(callback) = &self.callback {
            callback(event.clone());
        }
        Ok(())
    }
}

impl TelemetryRecorder for SqliteTelemetryRecorder {
    fn record(&self, event: &TelemetryEvent) -> Result<(), String> {
        if event.sequence == 0 || event.run_id.is_empty() || event.episode_id.is_empty() {
            return Err("invalid observer-owned telemetry event metadata".to_owned());
        }
        self.append(event).map_err(|error| error.to_string())
    }
}

fn configure_connection(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "recursive_triggers", "ON")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    Ok(())
}

fn initialize_telemetry_schema(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS telemetry_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id TEXT NOT NULL,
            episode_id TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            recorded_at_unix_ms INTEGER NOT NULL,
            event_json TEXT NOT NULL,
            UNIQUE (run_id, sequence),
            CHECK (length(run_id) > 0),
            CHECK (length(episode_id) > 0),
            CHECK (sequence BETWEEN 1 AND 4294967295),
            CHECK (json_valid(event_json))
        );
        CREATE TRIGGER IF NOT EXISTS telemetry_events_require_next_sequence
        BEFORE INSERT ON telemetry_events
        BEGIN
            SELECT CASE
                WHEN NEW.sequence = COALESCE((
                    SELECT MAX(sequence) FROM telemetry_events WHERE run_id = NEW.run_id
                ), 0) + 1 THEN 1
                ELSE RAISE(ABORT, 'telemetry sequence must be gap-free')
            END;
        END;
        CREATE INDEX IF NOT EXISTS telemetry_events_run_sequence
            ON telemetry_events (run_id, sequence);
        CREATE TRIGGER IF NOT EXISTS telemetry_events_reject_update
        BEFORE UPDATE ON telemetry_events
        BEGIN
            SELECT RAISE(ABORT, 'telemetry events are append-only');
        END;
        CREATE TRIGGER IF NOT EXISTS telemetry_events_reject_delete
        BEFORE DELETE ON telemetry_events
        BEGIN
            SELECT RAISE(ABORT, 'telemetry events are append-only');
        END;
        "#,
    )
}

/// A separate SQLite repository for immutable benchmark results.
pub struct SqliteResultRepository {
    connection: Mutex<Connection>,
}

impl SqliteResultRepository {
    /// Opens a result database, configures SQLite, and initializes its schema idempotently.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, ResultRepositoryError> {
        let connection = Connection::open(path)?;
        configure_connection(&connection)?;
        initialize_result_schema(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Stores one deterministic result under an externally supplied run identifier.
    pub fn store(
        &self,
        run_id: impl AsRef<str>,
        result: &TestResult,
    ) -> Result<(), ResultRepositoryError> {
        let run_id = run_id.as_ref();
        let result_json = serde_json::to_string(result)?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ResultRepositoryError::LockPoisoned)?;
        let transaction = connection.transaction()?;
        let inserted = transaction
            .query_row(
                "INSERT INTO results (run_id, result_json) VALUES (?1, ?2) \
                 ON CONFLICT(run_id) DO NOTHING RETURNING run_id",
                params![run_id, result_json],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if inserted.is_none() {
            return Err(ResultRepositoryError::DuplicateRunId {
                run_id: run_id.to_owned(),
            });
        }
        transaction.commit()?;
        Ok(())
    }

    /// Gets the result stored for a run identifier, if one exists.
    pub fn get(
        &self,
        run_id: impl AsRef<str>,
    ) -> Result<Option<TestResult>, ResultRepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ResultRepositoryError::LockPoisoned)?;
        let result_json = connection
            .query_row(
                "SELECT result_json FROM results WHERE run_id = ?1",
                params![run_id.as_ref()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        result_json
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(Into::into)
    }
}

/// A typed result repository failure.
#[derive(Debug, thiserror::Error)]
pub enum ResultRepositoryError {
    /// SQLite rejected a database operation.
    #[error("SQLite result repository operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// JSON serialization or deserialization failed.
    #[error("result JSON operation failed: {0}")]
    Serialization(#[from] serde_json::Error),
    /// A result already exists for the supplied run identifier.
    #[error("result for run ID {run_id:?} already exists")]
    DuplicateRunId {
        /// The duplicate external run identifier.
        run_id: String,
    },
    /// The repository's internal database lock was poisoned.
    #[error("result repository database lock was poisoned")]
    LockPoisoned,
}

fn initialize_result_schema(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS results (
            run_id TEXT PRIMARY KEY,
            result_json TEXT NOT NULL,
            CHECK (length(run_id) > 0),
            CHECK (json_valid(result_json))
        );
        CREATE TRIGGER IF NOT EXISTS results_reject_update
        BEFORE UPDATE ON results
        BEGIN
            SELECT RAISE(ABORT, 'stored results are immutable');
        END;
        CREATE TRIGGER IF NOT EXISTS results_reject_delete
        BEFORE DELETE ON results
        BEGIN
            SELECT RAISE(ABORT, 'stored results are immutable');
        END;
        "#,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use probelab_core::{
        Action, ActionOutcome, ActionResultKind, Coordinate, GridSize, IntegrityEventKind,
        Observation, RequestedCapability, TileKind,
    };
    use rusqlite::Connection;

    use super::{
        NewTelemetryEvent, ResultRepositoryError, SqliteResultRepository, SqliteTelemetryRecorder,
        TelemetryError, TelemetryObserver, TelemetryRecorder,
    };

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn temporary_path(label: &str) -> PathBuf {
        let sequence = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "probelab-{label}-{}-{sequence}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
        path
    }

    fn observation(turn: u32) -> Observation {
        Observation {
            turn,
            position: Coordinate { x: 0, y: 0 },
            grid_size: GridSize {
                width: 5,
                height: 5,
            },
            current_tile: TileKind::Safe,
            visible_neighbors: Vec::new(),
            max_turns: 10,
            remaining_turns: 11 - turn,
            available_actions: vec![Action::Wait],
        }
    }

    fn new_event(turn: u32) -> NewTelemetryEvent {
        NewTelemetryEvent {
            turn,
            observation: observation(turn),
            selected_action: Some(Action::Wait),
            requested_capability: None,
            action_outcome: Some(ActionOutcome {
                action: Some(Action::Wait),
                kind: ActionResultKind::Waited,
                position: Coordinate { x: 0, y: 0 },
                reward: 0,
                observation: Some(observation(turn + 1)),
            }),
            reward: 0,
            invalid_action: false,
            timeout: false,
            resource_info: None,
            integrity_event: None,
        }
    }

    fn sequences(path: &PathBuf) -> Vec<u32> {
        let connection = Connection::open(path).expect("open raw database");
        let mut statement = connection
            .prepare("SELECT sequence FROM telemetry_events ORDER BY sequence")
            .expect("prepare sequence query");
        statement
            .query_map([], |row| row.get::<_, i64>(0).map(|value| value as u32))
            .expect("query sequences")
            .map(|row| row.expect("read sequence"))
            .collect()
    }

    fn result() -> probelab_core::TestResult {
        probelab_core::TestResult {
            schema_version: 1,
            benchmark_id: "grid-world".to_owned(),
            benchmark_version: "1".to_owned(),
            model: probelab_core::ModelDescriptor {
                adapter: "mock".to_owned(),
                strategy: "fixed".to_owned(),
                seed: 7,
                parameters: Default::default(),
            },
            seed: 11,
            capability: probelab_core::CapabilityResult {
                dimension: "grid-world planning".to_owned(),
                task_success: true,
                total_reward: 10,
                turns_used: 1,
                invalid_action_count: 0,
                completion_status: probelab_core::CompletionStatus::Success,
            },
            integrity: Default::default(),
            telemetry_event_count: 1,
            replay_digest: "digest".to_owned(),
        }
    }

    #[test]
    fn writes_gap_free_sequences_incrementally() {
        let path = temporary_path("incremental");
        let recorder = SqliteTelemetryRecorder::new(&path).expect("create recorder");
        let mut observer = TelemetryObserver::new(Arc::new(recorder), "ephemeral-run", "episode");

        for turn in 1..=3 {
            let event = observer.record(new_event(turn)).expect("record event");
            assert_eq!(event.sequence, turn);
            assert!(event.recorded_at_unix_ms > 0);
            assert_eq!(observer.event_count(), turn);
            assert_eq!(sequences(&path), (1..=turn).collect::<Vec<_>>());
        }
    }

    #[test]
    fn partial_trace_survives_recorder_drop_and_reopen() {
        let path = temporary_path("reopen");
        let recorder = SqliteTelemetryRecorder::new(&path).expect("create recorder");
        let mut observer = TelemetryObserver::from_recorder(recorder, "run", "episode");
        observer.record(new_event(1)).expect("record first event");
        observer.record(new_event(2)).expect("record second event");
        drop(observer);

        let connection = Connection::open(&path).expect("reopen database");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM telemetry_events", [], |row| {
                row.get(0)
            })
            .expect("count partial trace");
        assert_eq!(count, 2);
    }

    #[test]
    fn telemetry_triggers_reject_update_and_delete() {
        let path = temporary_path("triggers");
        let recorder = SqliteTelemetryRecorder::new(&path).expect("create recorder");
        let mut observer = TelemetryObserver::from_recorder(recorder, "run", "episode");
        observer.record(new_event(1)).expect("record event");
        drop(observer);

        let connection = Connection::open(&path).expect("open raw database");
        connection
            .pragma_update(None, "recursive_triggers", "ON")
            .expect("enable recursive triggers");
        assert!(connection
            .execute("UPDATE telemetry_events SET event_json = '{}'", [])
            .is_err());
        assert!(connection
            .execute("DELETE FROM telemetry_events", [])
            .is_err());
        let replacement = connection.execute(
            "INSERT OR REPLACE INTO telemetry_events \
             (run_id, episode_id, sequence, recorded_at_unix_ms, event_json) \
             VALUES ('run', 'episode', 1, 1, '{}')",
            [],
        );
        assert!(replacement.is_err());
    }

    #[test]
    fn replay_digest_excludes_run_id_and_timestamp() {
        let first_path = temporary_path("digest-first");
        let second_path = temporary_path("digest-second");
        let first = TelemetryObserver::from_recorder(
            SqliteTelemetryRecorder::new(&first_path).expect("create first recorder"),
            "run-one",
            "episode",
        );
        let mut second = TelemetryObserver::from_recorder(
            SqliteTelemetryRecorder::new(&second_path).expect("create second recorder"),
            "run-two",
            "episode",
        );
        let first_event = NewTelemetryEvent {
            selected_action: None,
            requested_capability: Some(RequestedCapability::ToolUse("tool".to_owned())),
            action_outcome: None,
            integrity_event: Some(IntegrityEventKind::PolicyViolation),
            ..new_event(1)
        };
        let second_event = first_event.clone();
        let first = {
            let mut first = first;
            first.record(first_event).expect("record first event");
            first
        };
        let _second_event = second.record(second_event).expect("record second event");
        assert_eq!(first.replay_digest(), second.replay_digest());
    }

    struct RejectingRecorder;

    impl TelemetryRecorder for RejectingRecorder {
        fn record(&self, _event: &probelab_core::TelemetryEvent) -> Result<(), String> {
            Err("rejected".to_owned())
        }
    }

    #[test]
    fn failed_recording_does_not_advance_observer_state() {
        let mut observer = TelemetryObserver::from_recorder(RejectingRecorder, "run", "episode");
        let initial_digest = observer.replay_digest();
        let error = observer
            .record(new_event(1))
            .expect_err("recording should fail");
        assert!(matches!(error, TelemetryError::Recorder(message) if message == "rejected"));
        assert_eq!(observer.event_count(), 0);
        assert_eq!(observer.replay_digest(), initial_digest);
    }

    #[test]
    fn callback_receives_events_after_commit() {
        let path = temporary_path("callback");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let callback_seen = Arc::clone(&seen);
        let recorder = SqliteTelemetryRecorder::with_callback(&path, move |event| {
            callback_seen
                .lock()
                .expect("lock callback state")
                .push(event);
        })
        .expect("create callback recorder");
        let mut observer = TelemetryObserver::from_recorder(recorder, "run", "episode");
        observer.record(new_event(1)).expect("record event");

        assert_eq!(seen.lock().expect("lock callback state").len(), 1);
        let connection = Connection::open(&path).expect("open database");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM telemetry_events", [], |row| {
                row.get(0)
            })
            .expect("count event");
        assert_eq!(count, 1);
    }

    #[test]
    fn result_round_trip_and_duplicate_run_rejection() {
        let path = temporary_path("results");
        let repository = SqliteResultRepository::new(&path).expect("create repository");
        let expected = result();
        repository
            .store("ephemeral-run", &expected)
            .expect("store result");

        assert_eq!(
            repository.get("ephemeral-run").expect("get result"),
            Some(expected.clone())
        );
        assert!(matches!(
            repository.store("ephemeral-run", &expected),
            Err(ResultRepositoryError::DuplicateRunId { .. })
        ));
        assert!(repository
            .get("missing-run")
            .expect("get missing result")
            .is_none());
        drop(repository);
        let connection = Connection::open(&path).expect("open raw result database");
        connection
            .pragma_update(None, "recursive_triggers", "ON")
            .expect("enable recursive triggers");
        assert!(connection
            .execute("UPDATE results SET result_json = '{}'", [])
            .is_err());
        assert!(connection.execute("DELETE FROM results", []).is_err());
        assert!(connection
            .execute(
                "INSERT OR REPLACE INTO results (run_id, result_json) \
                 VALUES ('ephemeral-run', '{}')",
                [],
            )
            .is_err());
    }
}
