//! Independent telemetry observation and append-only persistence for ProbeLab.

#![deny(missing_docs)]

use std::fmt::Write as FmtWrite;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use probelab_core::{
    Action, ActionOutcome, ActionResultKind, CapabilityDenial, CapabilityPermission, CompletedRun,
    ContainmentStatus, GameTelemetry, IntegrityEventKind, IntegrityLevel, ModelCallFailure,
    ModelCallFailureKind, ModelCallInfo, ModelRequest, Observation, ProcessAuthorization,
    ProcessExecution, ProcessLifecycle, RequestedCapability, ResourceInfo, RunMetadata,
    StreamCapture, TelemetryEvent, TelemetryRecorder, TerminationReason, TestResult,
    STREAM_PREVIEW_LIMIT,
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
    /// The game board and move played, absent for every grid benchmark.
    pub game: Option<GameTelemetry>,
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
    /// Safe metadata for a successful model call, when supplied.
    #[serde(default)]
    pub model_call: Option<ModelCallInfo>,
    /// Sanitized metadata for a failed model call, when supplied.
    #[serde(default)]
    pub model_failure: Option<ModelCallFailure>,
    /// Capability-broker refusal, when the trusted broker denied a named capability.
    ///
    /// A broker denial is non-executable: it has no selected action, no requested capability, no
    /// action outcome, no reward, and no invalid-action flag, and it never carries a model
    /// failure. Safe model-call metadata may still be attached.
    #[serde(default)]
    pub broker_denial: Option<CapabilityDenial>,
    /// The full lifecycle of a requested process spawn, when one was requested this turn.
    ///
    /// Like a broker denial this is non-executable: it carries no action, no outcome, no reward, and
    /// no model failure, because a child process is not a grid action. The difference is that an
    /// authorized spawn really did start a process, so the record carries the operating system
    /// outcome: whether a child was created, its process identifier, its exit code, the containment
    /// state that was in force, and why it stopped.
    #[serde(default)]
    pub process_lifecycle: Option<ProcessLifecycle>,
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
            // Not recorded per turn, so it is reconstructed as `false`. This rebuilt request exists
            // to re-check the structural invariants of one turn — turn numbering, grid bounds, and
            // the model/observation action-vocabulary match — none of which involve the spawn
            // advertisement. Whether the benchmark offered a process is a property of the
            // benchmark, not of a turn, and inventing a value here would put a number in the
            // validation path that no run actually observed.
            process_spawn_available: false,
            // Telemetry records one turn's observation, not a rolling window, so there is no history
            // to revalidate here. The bound is enforced where history is built, in the engine.
            history: Vec::new(),
            // A game turn is validated by its own request, which is not what this path re-checks, so
            // the reconstructed request is explicitly grid-shaped. `game: None` is what routes it to
            // the grid rules rather than the game rules.
            game: None,
        }
        .validate()
        .map_err(TelemetryError::InvalidEvent)?;
        if self.model_call.is_some() && self.model_failure.is_some() {
            return Err(TelemetryError::InvalidEvent(
                "a model call and model failure are mutually exclusive".to_owned(),
            ));
        }
        if let Some(lifecycle) = &self.process_lifecycle {
            validate_process_lifecycle(self, lifecycle)?;
        } else if let Some(denial) = &self.broker_denial {
            validate_broker_denial(self, denial)?;
        } else {
            self.validate_decision()?;
        }
        if let Some(resource) = &self.resource_info {
            if resource.turns_used != self.turn || resource.timed_out != self.timeout {
                return Err(TelemetryError::InvalidEvent(
                    "resource turn or timeout data does not match the event".to_owned(),
                ));
            }
            if (self.invalid_action
                || self.integrity_event.is_some()
                || self.model_failure.is_some())
                && resource.rejected_actions == 0
            {
                return Err(TelemetryError::InvalidEvent(
                    "a rejected event must report at least one rejected action".to_owned(),
                ));
            }
        }
        Ok(())
    }

    /// Validates the executable decision content of an event that carries no broker denial.
    fn validate_decision(&self) -> Result<(), TelemetryError> {
        match (self.selected_action, &self.requested_capability) {
            (Some(action), None) => {
                if self.model_failure.is_some() {
                    if self.action_outcome.is_some()
                        || self.reward != 0
                        || self.invalid_action
                        || self.integrity_event.is_some()
                    {
                        return Err(TelemetryError::InvalidEvent(
                            "a rejected action cannot contain an environment or integrity outcome"
                                .to_owned(),
                        ));
                    }
                } else {
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
            }
            (None, Some(request)) => {
                if self.model_failure.is_some() {
                    return Err(TelemetryError::InvalidEvent(
                        "a denied capability cannot carry a model failure".to_owned(),
                    ));
                }
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
                if self.model_failure.is_none() {
                    return Err(TelemetryError::InvalidEvent(
                        "an event must contain an executable decision or a model failure"
                            .to_owned(),
                    ));
                }
                if self.action_outcome.is_some()
                    || self.reward != 0
                    || self.invalid_action
                    || self.integrity_event.is_some()
                {
                    return Err(TelemetryError::InvalidEvent(
                        "a model-failure event cannot contain an action or integrity outcome"
                            .to_owned(),
                    ));
                }
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
            game: self.game,
            requested_capability: self.requested_capability,
            action_outcome: self.action_outcome,
            reward: self.reward,
            invalid_action: self.invalid_action,
            timeout: self.timeout,
            resource_info: self.resource_info,
            integrity_event: self.integrity_event,
            model_call: self.model_call,
            model_failure: self.model_failure,
            broker_denial: self.broker_denial,
            process_lifecycle: self.process_lifecycle,
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

/// Validates that a broker-denial event is non-executable and self-consistent.
///
/// The event must carry no decision, no environment outcome, and no model failure, and it must
/// repeat exactly the integrity category the broker assigned, so a published integrity count can
/// always be traced back to a broker decision rather than to an observer-side assumption.
/// Validates the lifecycle record of one requested process spawn.
///
/// The rules encode the whole ordering guarantee. A denied request must carry the broker's typed
/// denial, must record the matching integrity category, and must have no execution at all. An
/// authorized request must carry an execution, must record no integrity event, and must never be
/// described as denied. In both cases the turn stays non-executable: a child process is not a grid
/// action, so there is no selected action, no requested capability, no outcome, no reward, and no
/// model failure.
fn validate_process_lifecycle(
    event: &NewTelemetryEvent,
    lifecycle: &ProcessLifecycle,
) -> Result<(), TelemetryError> {
    if event.selected_action.is_some()
        || event.requested_capability.is_some()
        || event.action_outcome.is_some()
        || event.reward != 0
        || event.invalid_action
        || event.model_failure.is_some()
    {
        return Err(TelemetryError::InvalidEvent(
            "a process request must not contain a decision, an outcome, a reward, or a model failure"
                .to_owned(),
        ));
    }
    if !lifecycle.requested {
        return Err(TelemetryError::InvalidEvent(
            "a process lifecycle record must be marked as requested".to_owned(),
        ));
    }
    match (&lifecycle.decision, &lifecycle.execution) {
        (ProcessAuthorization::Denied(denial), None) => {
            if event.broker_denial.as_ref() != Some(denial) {
                return Err(TelemetryError::InvalidEvent(
                    "a denied process request must carry the broker's exact denial".to_owned(),
                ));
            }
            if event.integrity_event != Some(denial.integrity_event) {
                return Err(TelemetryError::InvalidEvent(
                    "a denied process request must record the denial's integrity category"
                        .to_owned(),
                ));
            }
        }
        (ProcessAuthorization::Authorized, Some(execution)) => {
            if event.broker_denial.is_some() || event.integrity_event.is_some() {
                return Err(TelemetryError::InvalidEvent(
                    "an authorized process request must not record a denial or an integrity event"
                        .to_owned(),
                ));
            }
            validate_process_execution(execution)?;
        }
        (ProcessAuthorization::Authorized, None) => {
            return Err(TelemetryError::InvalidEvent(
                "an authorized process request must record what the operating system did"
                    .to_owned(),
            ));
        }
        (ProcessAuthorization::Denied(_), Some(_)) => {
            return Err(TelemetryError::InvalidEvent(
                "a denied process request must not record an execution".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Checks that an execution record is internally consistent.
///
/// A record that says a child ran must carry a process identifier, enforced containment verified at
/// the low integrity level, and a reason that is not "never started". A record that says nothing ran
/// must carry no identifier and no exit code, and must name the containment failure that prevented it.
fn validate_process_execution(execution: &ProcessExecution) -> Result<(), TelemetryError> {
    if execution.spawned {
        if execution.pid.is_none() {
            return Err(TelemetryError::InvalidEvent(
                "a spawned process must report its process identifier".to_owned(),
            ));
        }
        if !execution.containment.is_enforced() {
            return Err(TelemetryError::InvalidEvent(
                "a spawned process must report enforced containment".to_owned(),
            ));
        }
        if execution.verified_integrity() != Some(IntegrityLevel::Low) {
            return Err(TelemetryError::InvalidEvent(
                "a spawned process must have been verified at the low integrity level".to_owned(),
            ));
        }
        if execution.termination.started_nothing() {
            return Err(TelemetryError::InvalidEvent(
                "a spawned process must not claim it never started".to_owned(),
            ));
        }
        if execution.termination == TerminationReason::Exited && execution.exit_code.is_none() {
            return Err(TelemetryError::InvalidEvent(
                "a process that exited must report its exit code".to_owned(),
            ));
        }
        if execution.termination == TerminationReason::Timeout && execution.exit_code.is_some() {
            return Err(TelemetryError::InvalidEvent(
                "a process stopped by timeout must not report an exit code it never chose"
                    .to_owned(),
            ));
        }
        validate_stream_capture(&execution.stdout)?;
        validate_stream_capture(&execution.stderr)?;
    } else {
        if execution.pid.is_some() || execution.exit_code.is_some() {
            return Err(TelemetryError::InvalidEvent(
                "a process that never started must not report an identity or an exit code"
                    .to_owned(),
            ));
        }
        if execution.containment.is_enforced() {
            return Err(TelemetryError::InvalidEvent(
                "a process that never started must not report enforced containment".to_owned(),
            ));
        }
        if !execution.termination.started_nothing() {
            return Err(TelemetryError::InvalidEvent(
                "a process that never started must say so".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Checks that one captured stream is internally consistent.
///
/// The point of the cap is that telemetry cannot be grown without bound by a child, so the recorded
/// preview must never be longer than the cap, and the `truncated` flag must agree with the byte count.
fn validate_stream_capture(capture: &StreamCapture) -> Result<(), TelemetryError> {
    if capture.preview.chars().count() > STREAM_PREVIEW_LIMIT {
        return Err(TelemetryError::InvalidEvent(
            "a captured stream preview exceeds the documented cap".to_owned(),
        ));
    }
    let expected_truncation = capture.bytes > STREAM_PREVIEW_LIMIT as u64;
    if capture.truncated != expected_truncation {
        return Err(TelemetryError::InvalidEvent(
            "a captured stream's truncated flag disagrees with its byte count".to_owned(),
        ));
    }
    if !expected_truncation && capture.bytes != capture.preview.chars().count() as u64 {
        return Err(TelemetryError::InvalidEvent(
            "an untruncated captured stream must carry its whole content".to_owned(),
        ));
    }
    Ok(())
}

fn validate_broker_denial(
    event: &NewTelemetryEvent,
    denial: &CapabilityDenial,
) -> Result<(), TelemetryError> {
    if event.selected_action.is_some()
        || event.requested_capability.is_some()
        || event.action_outcome.is_some()
        || event.reward != 0
        || event.invalid_action
        || event.model_failure.is_some()
    {
        return Err(TelemetryError::InvalidEvent(
            "a broker denial must not contain a decision, an outcome, a reward, or a model failure"
                .to_owned(),
        ));
    }
    if event.integrity_event != Some(denial.integrity_event) {
        return Err(TelemetryError::InvalidEvent(
            "a broker denial must carry the exact integrity event of the denial".to_owned(),
        ));
    }
    if !denial.integrity_event.is_broker_category() {
        return Err(TelemetryError::InvalidEvent(
            "a broker denial must use an independent broker integrity category".to_owned(),
        ));
    }
    Ok(())
}

/// The deterministic replay projection of one broker denial.
///
/// Only the permission and the independent integrity category are projected. The reason, the
/// requested path, program, endpoint, arguments, and environment variable are all excluded, so
/// the semantic replay digest can never carry a requested value.
#[derive(Serialize)]
struct ReplayBrokerDenial {
    permission: CapabilityPermission,
    integrity_event: IntegrityEventKind,
}

/// The deterministic replay projection of one process spawn lifecycle.
///
/// Only the shape of the lifecycle is projected: whether it was requested, whether the broker
/// authorized it, whether a child was created, the containment state, and why it stopped. The process
/// identifier, the exit code, the CPU time, and the program are all excluded, because a PID and an
/// exit code differ between two otherwise identical runs and would make a mock run's digest
/// unreproducible. A denial projects only its permission and category, exactly like a broker denial.
#[derive(Serialize)]
struct ReplayProcessLifecycle {
    requested: bool,
    authorized: bool,
    spawned: bool,
    containment: Option<ContainmentStatus>,
    termination: Option<TerminationReason>,
}

#[derive(Serialize)]
struct ReplayPayload<'a> {
    version: u8,
    episode_id: &'a str,
    sequence: u32,
    event: ReplayEvent<'a>,
}

#[derive(Serialize)]
struct ReplayEvent<'a> {
    turn: u32,
    observation: &'a Observation,
    selected_action: Option<Action>,
    requested_capability: Option<&'a RequestedCapability>,
    action_outcome: Option<&'a ActionOutcome>,
    reward: i64,
    invalid_action: bool,
    timeout: bool,
    resource_info: Option<&'a ResourceInfo>,
    integrity_event: Option<IntegrityEventKind>,
    model_failure_kind: Option<ModelCallFailureKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    broker_denial: Option<ReplayBrokerDenial>,
    #[serde(skip_serializing_if = "Option::is_none")]
    process_lifecycle: Option<ReplayProcessLifecycle>,
}

impl<'a> ReplayPayload<'a> {
    fn new(episode_id: &'a str, sequence: u32, input: &'a NewTelemetryEvent) -> Self {
        Self {
            version: 2,
            episode_id,
            sequence,
            event: ReplayEvent {
                turn: input.turn,
                observation: &input.observation,
                selected_action: input.selected_action,
                requested_capability: input.requested_capability.as_ref(),
                action_outcome: input.action_outcome.as_ref(),
                reward: input.reward,
                invalid_action: input.invalid_action,
                timeout: input.timeout,
                resource_info: input.resource_info.as_ref(),
                integrity_event: input.integrity_event,
                model_failure_kind: input.model_failure.as_ref().map(|failure| failure.kind),
                broker_denial: input
                    .broker_denial
                    .as_ref()
                    .map(|denial| ReplayBrokerDenial {
                        permission: denial.permission,
                        integrity_event: denial.integrity_event,
                    }),
                process_lifecycle: input.process_lifecycle.as_ref().map(|lifecycle| {
                    let execution = lifecycle.execution();
                    ReplayProcessLifecycle {
                        requested: lifecycle.requested,
                        authorized: !lifecycle.is_denied(),
                        spawned: execution.is_some_and(|record| record.spawned),
                        containment: execution.map(|record| record.containment),
                        termination: execution.map(|record| record.termination),
                    }
                }),
            },
        }
    }
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
        let replay_bytes =
            serde_json::to_vec(&ReplayPayload::new(&self.episode_id, sequence, &input))?;
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

    /// Stores one completed result and its durable execution metadata atomically.
    pub fn store_completed(
        &self,
        run_id: impl AsRef<str>,
        run: &CompletedRun,
    ) -> Result<(), ResultRepositoryError> {
        let run_id = run_id.as_ref();
        let result_json = serde_json::to_string(&run.result)?;
        let metadata_json = serde_json::to_string(&run.metadata)?;
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| ResultRepositoryError::LockPoisoned)?;
        let transaction = connection.transaction()?;
        let inserted = transaction
            .query_row(
                "INSERT INTO results (run_id, result_json, metadata_json) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(run_id) DO NOTHING RETURNING run_id",
                params![run_id, result_json, metadata_json],
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

    /// Every stored result, newest last, for the identifiers given.
    ///
    /// # Why this takes identifiers rather than returning everything
    ///
    /// A comparison is over a chosen set of runs, not over the whole history, so the caller decides
    /// the set. Returning everything would make an accidental "compare all" both possible and
    /// unbounded, and the number of stored results grows without limit.
    ///
    /// Results are returned in the order the identifiers were given, and an identifier with no stored
    /// result is simply absent rather than an error: a run can be listed before its result lands,
    /// and a comparison that refused to render because one run had not finished writing would be
    /// reporting a race as a failure.
    pub fn list(&self, run_ids: &[String]) -> Result<Vec<TestResult>, ResultRepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ResultRepositoryError::LockPoisoned)?;
        let mut found = Vec::with_capacity(run_ids.len());
        for run_id in run_ids {
            let result_json: Option<String> = connection
                .query_row(
                    "SELECT result_json FROM results WHERE run_id = ?1",
                    params![run_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(json) = result_json {
                found.push(serde_json::from_str(&json)?);
            }
        }
        Ok(found)
    }

    /// Records an experiment's lifecycle state.
    ///
    /// Separate from `put`, because a result is immutable evidence while this is a mutable status. A
    /// crashed run leaves its last persisted state behind, which is what
    /// [`Self::reconcile_interrupted`] later corrects.
    pub fn put_experiment(&self, record: &ExperimentRecord) -> Result<(), ResultRepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ResultRepositoryError::LockPoisoned)?;
        connection
            .execute(
                "INSERT INTO experiments (
                    experiment_id, benchmark_id, lifecycle, terminal_reason,
                    episodes_requested, episodes_completed, episodes_failed,
                    has_score, score, result_json, updated_unix_ms,
                    target_agent, root_seed
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                  ON CONFLICT(experiment_id) DO UPDATE SET
                     benchmark_id = excluded.benchmark_id,
                     lifecycle = excluded.lifecycle,
                     terminal_reason = excluded.terminal_reason,
                     episodes_requested = excluded.episodes_requested,
                     episodes_completed = excluded.episodes_completed,
                    episodes_failed = excluded.episodes_failed,
                    has_score = excluded.has_score,
                    score = excluded.score,
                    result_json = excluded.result_json,
                    updated_unix_ms = excluded.updated_unix_ms,
                    target_agent = excluded.target_agent,
                    root_seed = excluded.root_seed",
                rusqlite::params![
                    record.experiment_id,
                    record.benchmark_id,
                    record.lifecycle.as_str(),
                    record.terminal_reason,
                    i64::try_from(record.episodes_requested).unwrap_or(i64::MAX),
                    i64::try_from(record.episodes_completed).unwrap_or(i64::MAX),
                    i64::try_from(record.episodes_failed).unwrap_or(i64::MAX),
                    i64::from(record.has_score),
                    record.score,
                    record.result_json,
                    record.updated_unix_ms,
                    record.target_agent,
                    // SQLite has no unsigned integer, so a `u64` seed is stored as its bit pattern
                    // reinterpreted as `i64` and read back the same way. A plain `as i64` cast would
                    // wrap above `i64::MAX`, and a seed that read back as a different number is
                    // exactly the property a comparison relies on to decide what it may compare.
                    record.root_seed.map(|seed| seed as i64),
                ],
            )
            .map_err(ResultRepositoryError::Sqlite)?;
        Ok(())
    }

    /// Gets one experiment's lifecycle record.
    pub fn get_experiment(
        &self,
        experiment_id: impl AsRef<str>,
    ) -> Result<Option<ExperimentRecord>, ResultRepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ResultRepositoryError::LockPoisoned)?;
        let mut statement = connection
            .prepare(
                "SELECT experiment_id, benchmark_id, lifecycle, terminal_reason,
                        episodes_requested, episodes_completed, episodes_failed,
                        has_score, score, result_json, updated_unix_ms,
                        target_agent, root_seed
                 FROM experiments WHERE experiment_id = ?1",
            )
            .map_err(ResultRepositoryError::Sqlite)?;
        let mut rows = statement
            .query(rusqlite::params![experiment_id.as_ref().to_owned()])
            .map_err(ResultRepositoryError::Sqlite)?;
        let Some(row) = rows.next().map_err(ResultRepositoryError::Sqlite)? else {
            return Ok(None);
        };
        Ok(Some(read_experiment_row(row)?))
    }

    /// Every experiment record, newest first.
    pub fn list_experiments(&self) -> Result<Vec<ExperimentRecord>, ResultRepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ResultRepositoryError::LockPoisoned)?;
        let mut statement = connection
            .prepare(
                "SELECT experiment_id, benchmark_id, lifecycle, terminal_reason,
                        episodes_requested, episodes_completed, episodes_failed,
                        has_score, score, result_json, updated_unix_ms,
                        target_agent, root_seed
                 FROM experiments ORDER BY updated_unix_ms DESC, experiment_id DESC",
            )
            .map_err(ResultRepositoryError::Sqlite)?;
        let mut rows = statement.query([]).map_err(ResultRepositoryError::Sqlite)?;
        let mut records = Vec::new();
        while let Some(row) = rows.next().map_err(ResultRepositoryError::Sqlite)? {
            records.push(read_experiment_row(row)?);
        }
        Ok(records)
    }

    /// Marks every experiment still recorded as running as interrupted.
    ///
    /// Called once when the application starts. This is the transition that needs no cooperation from
    /// the process that died: if a row says `running` and we have just started up, nothing is running.
    ///
    /// It is also why the lifecycle has an `interrupted` state at all. Without it, a crashed run would
    /// be reported as completed — the single worst outcome available, because a partial measurement
    /// would be indistinguishable from a finished one.
    ///
    /// Returns the identifiers it changed, so a caller can report them rather than silently rewriting
    /// history. Only `running` is transitioned: a `planned` experiment that never started is left
    /// alone, because nothing is known about it.
    pub fn reconcile_interrupted(&self) -> Result<Vec<String>, ResultRepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ResultRepositoryError::LockPoisoned)?;
        let now = unix_milliseconds().unwrap_or(0);
        let mut statement = connection
            .prepare(
                "UPDATE experiments
                 SET lifecycle = 'interrupted', updated_unix_ms = ?1
                 WHERE lifecycle = 'running'
                 RETURNING experiment_id",
            )
            .map_err(ResultRepositoryError::Sqlite)?;
        let mut rows = statement
            .query(rusqlite::params![now])
            .map_err(ResultRepositoryError::Sqlite)?;
        let mut changed = Vec::new();
        while let Some(row) = rows.next().map_err(ResultRepositoryError::Sqlite)? {
            changed.push(row.get(0).map_err(ResultRepositoryError::Sqlite)?);
        }
        Ok(changed)
    }

    /// Gets immutable execution metadata stored for a run identifier, if present.
    pub fn get_metadata(
        &self,
        run_id: impl AsRef<str>,
    ) -> Result<Option<RunMetadata>, ResultRepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ResultRepositoryError::LockPoisoned)?;
        let metadata_json = connection
            .query_row(
                "SELECT metadata_json FROM results WHERE run_id = ?1",
                params![run_id.as_ref()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        metadata_json
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(Into::into)
    }

    /// Reads back one run's committed telemetry, in sequence order.
    ///
    /// # Why this exists
    ///
    /// A run's events were written append-only and then, until now, could only be had *while the
    /// process that recorded them was still alive* — the command-center's live buffer holds the last
    /// few hundred events of the current session. So a recording was only ever replayable inside the
    /// window in which it happened. A grid episode is exactly the kind of thing a person wants to watch
    /// a second time, and after a restart or a crash the events were still on disk and unreadable.
    ///
    /// Nothing is stored *for* this reader: the events were already recorded in full, so the read
    /// reconstructs a playback from data the run committed rather than adding a second, possibly
    /// divergent, trace beside it.
    ///
    /// # Why this is a read and not a re-run
    ///
    /// There is no model here, no provider, and no engine. Every field returned is a field the run
    /// committed, which is what makes a replayed episode the same episode rather than a second
    /// experiment wearing a familiar name — see `probelab_lab::replay` for the same argument at the
    /// arena's layer.
    ///
    /// # Ordering, and why it is ordered rather than assumed
    ///
    /// `ORDER BY sequence` is the whole contract. A viewer that reconstructed an episode in
    /// insertion order would depend on SQLite's physical layout, and `id` is an autoincrement column
    /// that only happens to agree with `sequence` today. The trigger on the table already refuses a
    /// gap, so the order read here is the order the run executed in.
    ///
    /// An unknown run identifier yields an empty vector rather than an error: "nothing was recorded
    /// under that name" is the truthful answer, and an error would force every caller to invent a
    /// distinction between a run that never happened and a run whose recording is missing.
    pub fn telemetry_events(
        &self,
        run_id: impl AsRef<str>,
    ) -> Result<Vec<TelemetryEvent>, ResultRepositoryError> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| ResultRepositoryError::LockPoisoned)?;
        let mut statement = connection
            .prepare(
                "SELECT event_json FROM telemetry_events \
                 WHERE run_id = ?1 ORDER BY sequence ASC",
            )
            .map_err(ResultRepositoryError::Sqlite)?;
        let mut rows = statement
            .query(rusqlite::params![run_id.as_ref()])
            .map_err(ResultRepositoryError::Sqlite)?;
        let mut events = Vec::new();
        while let Some(row) = rows.next().map_err(ResultRepositoryError::Sqlite)? {
            let event_json: String = row.get(0).map_err(ResultRepositoryError::Sqlite)?;
            events.push(serde_json::from_str(&event_json)?);
        }
        Ok(events)
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
    /// A stored lifecycle value was not one of the known states.
    ///
    /// Refused rather than defaulted. Defaulting would turn a corrupted row into a
    /// plausible-looking lifecycle, which is precisely the failure the lifecycle exists to prevent.
    #[error("stored experiment lifecycle {value:?} is not a known state")]
    UnknownLifecycle {
        /// The unrecognised value found in the database.
        value: String,
    },
}

/// The lifecycle of a persisted experiment.
///
/// Five states, and the fifth is the point. `planned`, `running`, `completed`, and `failed` are all
/// things a live process can assert. `interrupted` is the one a dead process cannot: it is written by
/// the *next* start, on the evidence that a row said `running` while nothing was running.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentLifecycle {
    /// Recorded but not started.
    Planned,
    /// Episodes are executing.
    Running,
    /// Every requested episode was attempted and the run reached a terminal state.
    Completed,
    /// The run reached a terminal state other than completion.
    Failed,
    /// The process died while the experiment was running.
    Interrupted,
}

impl ExperimentLifecycle {
    /// The stable stored value.
    pub fn as_str(self) -> &'static str {
        match self {
            ExperimentLifecycle::Planned => "planned",
            ExperimentLifecycle::Running => "running",
            ExperimentLifecycle::Completed => "completed",
            ExperimentLifecycle::Failed => "failed",
            ExperimentLifecycle::Interrupted => "interrupted",
        }
    }

    /// Parses a stored value, refusing anything unrecognised.
    ///
    /// An unknown value is an error rather than a default. Defaulting would silently turn a corrupted
    /// row into a plausible-looking lifecycle, which is the failure mode this whole mechanism exists
    /// to prevent.
    pub fn parse(value: &str) -> Result<Self, ResultRepositoryError> {
        match value {
            "planned" => Ok(ExperimentLifecycle::Planned),
            "running" => Ok(ExperimentLifecycle::Running),
            "completed" => Ok(ExperimentLifecycle::Completed),
            "failed" => Ok(ExperimentLifecycle::Failed),
            "interrupted" => Ok(ExperimentLifecycle::Interrupted),
            other => Err(ResultRepositoryError::UnknownLifecycle {
                value: other.to_owned(),
            }),
        }
    }

    /// Whether this state means the run is over.
    pub fn is_terminal(self) -> bool {
        !matches!(
            self,
            ExperimentLifecycle::Planned | ExperimentLifecycle::Running
        )
    }

    /// Whether the episodes this run recorded can be trusted as a complete measurement.
    ///
    /// False for anything that is not `completed`. A partial run's episodes are real evidence of what
    /// happened, and they are kept -- but they are not a measurement, and a caller must not present
    /// them as one.
    pub fn is_a_complete_measurement(self) -> bool {
        self == ExperimentLifecycle::Completed
    }
}

/// One persisted experiment's lifecycle state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentRecord {
    /// Stable identifier.
    pub experiment_id: String,
    /// The benchmark it ran.
    pub benchmark_id: String,
    /// Where it is in its lifecycle.
    pub lifecycle: ExperimentLifecycle,
    /// Why it ended, when it did.
    pub terminal_reason: Option<String>,
    /// Episodes the plan asked for.
    pub episodes_requested: usize,
    /// Episodes that produced a result.
    pub episodes_completed: usize,
    /// Episodes that failed.
    pub episodes_failed: usize,
    /// Whether a score exists.
    ///
    /// Stored beside the score rather than inferred from it, because a run with no completed episode
    /// has no score and `None` must not be read as zero. A boolean that means "the absence is
    /// deliberate" survives a schema migration better than a nullable float with an ambiguous default.
    pub has_score: bool,
    /// The score, when one exists.
    pub score: Option<f64>,
    /// The result bundle, when the run completed.
    pub result_json: Option<String>,
    /// When this row was last written, in Unix milliseconds.
    pub updated_unix_ms: i64,
    /// The agent the plan measured.
    ///
    /// Part of the experiment's *identity*, not its outcome: two runs of different agents are not
    /// measuring the same thing, and a comparison that could not see this would rank them as though
    /// it could. `None` for a row written before this was stored, which is a truthful answer rather
    /// than a missing one -- a comparison over such rows refuses instead of assuming.
    pub target_agent: Option<String>,
    /// The plan's root seed.
    ///
    /// `None` under the same condition and for the same reason as [`Self::target_agent`].
    pub root_seed: Option<u64>,
}

/// Reads one experiment row into a typed record.
fn read_experiment_row(row: &rusqlite::Row<'_>) -> Result<ExperimentRecord, ResultRepositoryError> {
    let lifecycle: String = row.get(2).map_err(ResultRepositoryError::Sqlite)?;
    Ok(ExperimentRecord {
        experiment_id: row.get(0).map_err(ResultRepositoryError::Sqlite)?,
        benchmark_id: row.get(1).map_err(ResultRepositoryError::Sqlite)?,
        lifecycle: ExperimentLifecycle::parse(&lifecycle)?,
        terminal_reason: row.get(3).map_err(ResultRepositoryError::Sqlite)?,
        episodes_requested: usize::try_from(
            row.get::<_, i64>(4)
                .map_err(ResultRepositoryError::Sqlite)?,
        )
        .unwrap_or(0),
        episodes_completed: usize::try_from(
            row.get::<_, i64>(5)
                .map_err(ResultRepositoryError::Sqlite)?,
        )
        .unwrap_or(0),
        episodes_failed: usize::try_from(
            row.get::<_, i64>(6)
                .map_err(ResultRepositoryError::Sqlite)?,
        )
        .unwrap_or(0),
        has_score: row
            .get::<_, i64>(7)
            .map_err(ResultRepositoryError::Sqlite)?
            != 0,
        score: row.get(8).map_err(ResultRepositoryError::Sqlite)?,
        result_json: row.get(9).map_err(ResultRepositoryError::Sqlite)?,
        updated_unix_ms: row.get(10).map_err(ResultRepositoryError::Sqlite)?,
        // Named by column rather than by position: the identity columns were added by a migration
        // and are the last two, but a select that lists them by name keeps working if another
        // column is ever added between them and the end.
        target_agent: column_by_name(row, "target_agent")?,
        // Reinterpreted rather than cast, undoing the `u64` to `i64` bit-pattern conversion on the
        // way in, so a seed above `i64::MAX` round-trips to the same number.
        root_seed: column_by_name::<i64>(row, "root_seed")?.map(|seed| seed as u64),
    })
}

/// Reads a column by name, for columns a migration may not have added to an older database.
///
/// `row.as_ref().column_index(name)` is what makes this work across schema versions: a database
/// written before the identity columns existed has no such column, and asking for a `NULL` from a
/// column that is not there would fail rather than report the absence.
fn column_by_name<T: rusqlite::types::FromSql>(
    row: &rusqlite::Row<'_>,
    name: &str,
) -> Result<Option<T>, ResultRepositoryError> {
    let index = match row.as_ref().column_index(name) {
        Ok(index) => index,
        Err(_) => return Ok(None),
    };
    // Written as an explicit match rather than `map_err` because the error conversion is what needs
    // the type annotation: `Sqlite` is a tuple variant holding a `rusqlite::Error`, and inferring it
    // through a closure leaves the compiler unable to name the parameter type.
    row.get::<_, Option<T>>(index)
        .map_err(ResultRepositoryError::from)
}

fn initialize_result_schema(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS results (
            run_id TEXT PRIMARY KEY,
            result_json TEXT NOT NULL,
            metadata_json TEXT,
            CHECK (length(run_id) > 0),
            CHECK (json_valid(result_json)),
            CHECK (metadata_json IS NULL OR json_valid(metadata_json))
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

        -- The experiment lifecycle, kept separate from results for one reason: a result is immutable
        -- evidence, while an experiment's state *changes* as it runs. Putting both in one table would
        -- force the immutability trigger to be relaxed, and with it the guarantee that a recorded
        -- result cannot be rewritten.
        --
        -- `interrupted` exists because a process that dies mid-run leaves a row saying `running`, and
        -- on the next start that row is provably wrong: nothing is running. Transitioning it to
        -- `interrupted` at startup is the one transition that requires no cooperation from the dead
        -- process, and it is what stops an abandoned run from being reported as completed.
        CREATE TABLE IF NOT EXISTS experiments (
            experiment_id TEXT PRIMARY KEY,
            benchmark_id TEXT NOT NULL,
            lifecycle TEXT NOT NULL,
            terminal_reason TEXT,
            episodes_requested INTEGER NOT NULL,
            episodes_completed INTEGER NOT NULL DEFAULT 0,
            episodes_failed INTEGER NOT NULL DEFAULT 0,
            has_score INTEGER NOT NULL DEFAULT 0,
            score REAL,
            result_json TEXT,
            updated_unix_ms INTEGER NOT NULL,
            CHECK (length(experiment_id) > 0),
            CHECK (lifecycle IN ('planned','running','completed','failed','interrupted')),
            CHECK (result_json IS NULL OR json_valid(result_json)),
            CHECK (episodes_completed >= 0 AND episodes_failed >= 0)
        );
        CREATE INDEX IF NOT EXISTS experiments_by_lifecycle
            ON experiments (lifecycle, updated_unix_ms);
        "#,
    )?;
    add_result_metadata_column(connection)?;
    add_experiment_identity_columns(connection)
}

fn add_result_metadata_column(connection: &Connection) -> Result<(), rusqlite::Error> {
    let mut statement = connection.prepare("PRAGMA table_info(results)")?;
    let has_metadata_column = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|column| column == "metadata_json");
    if !has_metadata_column {
        connection.execute_batch("ALTER TABLE results ADD COLUMN metadata_json TEXT")?;
    }
    Ok(())
}

/// Adds the columns that make an experiment's identity queryable.
///
/// # Why these are nullable
///
/// A comparison can only be made between runs that measured the same thing, and that requires the
/// agent and the seed — the plan, not the result. Adding them as nullable columns with no default
/// means an existing database keeps its rows and reports the truth it has: these runs predate
/// identity, so a comparison over them will refuse rather than assume. Backfilling a seed would
/// fabricate the exact property the comparison exists to check.
fn add_experiment_identity_columns(connection: &Connection) -> Result<(), rusqlite::Error> {
    let mut statement = connection.prepare("PRAGMA table_info(experiments)")?;
    let columns: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;

    for (column, definition) in [("target_agent", "TEXT"), ("root_seed", "INTEGER")] {
        if !columns.iter().any(|existing| existing == column) {
            connection.execute_batch(&format!(
                "ALTER TABLE experiments ADD COLUMN {column} {definition}"
            ))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use probelab_core::{
        Action, ActionOutcome, ActionResultKind, CapabilityDenial, CapabilityPermission,
        ContainmentStatus, Coordinate, GridSize, IntegrityEventKind, IntegrityLevel,
        ModelCallFailure, ModelCallFailureKind, ModelCallInfo, Observation, ProcessExecution,
        ProcessLifecycle, RequestedCapability, StreamCapture, TerminationReason, TileKind,
        STREAM_PREVIEW_LIMIT,
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
            distance_to_goal: 1,
            max_turns: 10,
            remaining_turns: 11 - turn,
            available_actions: vec![Action::Wait],
        }
    }

    /// Builds an authorized-spawn event whose execution can be varied by the caller.
    fn authorized_spawn_event(stdout: StreamCapture) -> NewTelemetryEvent {
        let execution = ProcessExecution {
            spawned: true,
            pid: Some(4242),
            exit_code: Some(0),
            containment: ContainmentStatus::Enforced {
                integrity: IntegrityLevel::Low,
                limits_applied: 7,
            },
            termination: TerminationReason::Exited,
            user_cpu_millis: 3,
            kernel_cpu_millis: 1,
            stdout,
            stderr: StreamCapture::empty(),
        };
        NewTelemetryEvent {
            turn: 1,
            observation: observation(1),
            selected_action: None,
            game: None,
            requested_capability: None,
            action_outcome: None,
            reward: 0,
            invalid_action: false,
            timeout: false,
            resource_info: None,
            integrity_event: None,
            model_call: None,
            model_failure: None,
            broker_denial: None,
            process_lifecycle: Some(ProcessLifecycle::authorized(execution)),
        }
    }

    fn new_event(turn: u32) -> NewTelemetryEvent {
        NewTelemetryEvent {
            turn,
            observation: observation(turn),
            selected_action: Some(Action::Wait),
            game: None,
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
            model_call: None,
            model_failure: None,
            broker_denial: None,
            process_lifecycle: None,
        }
    }

    fn denial(
        permission: CapabilityPermission,
        integrity_event: IntegrityEventKind,
    ) -> CapabilityDenial {
        CapabilityDenial::new(permission, integrity_event, "unit-test broker refusal")
    }

    fn broker_denial_event(
        turn: u32,
        permission: CapabilityPermission,
        integrity_event: IntegrityEventKind,
    ) -> NewTelemetryEvent {
        NewTelemetryEvent {
            turn,
            observation: observation(turn),
            selected_action: None,
            game: None,
            requested_capability: None,
            action_outcome: None,
            reward: 0,
            invalid_action: false,
            timeout: false,
            resource_info: Some(probelab_core::ResourceInfo {
                turns_used: turn,
                rejected_actions: 1,
                timed_out: false,
            }),
            integrity_event: Some(integrity_event),
            model_call: None,
            model_failure: None,
            broker_denial: Some(denial(permission, integrity_event)),
            process_lifecycle: None,
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
            // Absent, not fabricated: a storage test has no embedded definition behind it, and a
            // digest invented here would be a value a reader could mistake for a real one.
            benchmark_definition_digest: None,
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
                protocol_adherence: Default::default(),
            },
            integrity: Default::default(),
            telemetry_event_count: 1,
            replay_digest: "digest".to_owned(),
            reproducibility: Default::default(),
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

    /// A grid event's serialized bytes must not mention the game channel at all.
    ///
    /// This is the guard that lets the arena be added without invalidating published results. The
    /// `game` field is `Option`, and an `Option` that always serialised would write `null` into every
    /// grid event — changing the bytes that feed the replay digest and therefore every recorded run,
    /// including the grid-world numbers this project has already published. `skip_serializing_if`
    /// omits the key entirely, so grid events are byte-identical to before games existed.
    ///
    /// Asserting on the absence of the key is what makes that guarantee checkable. Relying on a test
    /// suite staying green is not enough here, because a digest change would be *silent*: every test
    /// would still pass, and only a number already in a document would have moved.
    #[test]
    fn a_grid_event_serializes_without_the_game_channel() {
        let event = new_event(1).clone().into_event("run", "episode", 1, 0);
        let json = serde_json::to_string(&event).expect("serialize a grid event");
        assert!(
            !json.contains("\"game\""),
            "a grid event leaked the game channel into its bytes: {json}"
        );
        // And the field round-trips as absent rather than as a null the digest could pick up.
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("reparse");
        assert!(
            parsed.get("game").is_none(),
            "the game key should be absent, not null: {json}"
        );
    }

    /// A game event does carry the channel, and it changes the digest.
    ///
    /// The other half of the guard. If a game payload were dropped rather than serialised, the
    /// absence check above would still pass and a match would replay to a different board.
    #[test]
    fn a_game_event_serializes_the_board_and_changes_the_digest() {
        use probelab_core::{GameBoard, GameKind, GameMove, GameSide, GameTelemetry, Position};

        let grid = new_event(1).clone().into_event("run", "episode", 1, 0);
        let mut game = new_event(1).clone();
        game.game = Some(GameTelemetry {
            kind: GameKind::Battleship,
            side: GameSide::First,
            board: GameBoard::TicTacToe {
                cells: vec![probelab_core::TicTacToeCell::First],
            },
            played: Some(GameMove::Battleship(Position::new(1, 2))),
            ply: 1,
        });
        let game = game.into_event("run", "episode", 1, 0);

        let grid_json = serde_json::to_string(&grid).expect("serialize grid");
        let game_json = serde_json::to_string(&game).expect("serialize game");
        assert!(
            game_json.contains("\"game\""),
            "a game event must carry its board, or a live match view has nothing to draw"
        );
        assert_ne!(
            grid_json, game_json,
            "a game payload that does not change the bytes would be dropped by the digest"
        );
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

    #[test]
    fn replay_digest_excludes_volatile_model_call_metadata() {
        let first_path = temporary_path("model-digest-first");
        let second_path = temporary_path("model-digest-second");
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
            model_call: Some(ModelCallInfo {
                provider: "provider".to_owned(),
                model: "model".to_owned(),
                prompt_template_version: "prompt-v1".to_owned(),
                request_id: Some("request-one".to_owned()),
                finish_reason: Some("stop".to_owned()),
                usage: None,
                timeout_seconds: Some(1),
                elapsed_ms: 1,
                response_text: Some("first response".to_owned()),
            }),
            ..new_event(1)
        };
        let second_event = NewTelemetryEvent {
            model_call: Some(ModelCallInfo {
                provider: "provider".to_owned(),
                model: "model".to_owned(),
                prompt_template_version: "prompt-v1".to_owned(),
                request_id: Some("request-two".to_owned()),
                finish_reason: Some("length".to_owned()),
                usage: None,
                timeout_seconds: Some(9),
                elapsed_ms: 99,
                response_text: Some("second response".to_owned()),
            }),
            ..new_event(1)
        };
        let first = {
            let mut first = first;
            first.record(first_event).expect("record first event");
            first
        };
        second.record(second_event).expect("record second event");
        assert_eq!(first.replay_digest(), second.replay_digest());
    }

    #[test]
    fn model_failure_event_requires_no_decision_content() {
        let valid = NewTelemetryEvent {
            selected_action: None,
            requested_capability: None,
            action_outcome: None,
            reward: 0,
            invalid_action: false,
            timeout: false,
            resource_info: None,
            integrity_event: None,
            model_call: None,
            model_failure: Some(ModelCallFailure::new(
                ModelCallFailureKind::MalformedResponse,
                "malformed response",
            )),
            ..new_event(1)
        };
        valid.validate().expect("valid model failure event");
        let rejected_action = NewTelemetryEvent {
            selected_action: Some(Action::Wait),
            ..valid.clone()
        };
        rejected_action
            .validate()
            .expect("rejected action may be recorded without an outcome");
        let invalid = NewTelemetryEvent {
            selected_action: Some(Action::Wait),
            action_outcome: new_event(1).action_outcome,
            ..valid.clone()
        };
        assert!(matches!(
            invalid.validate(),
            Err(TelemetryError::InvalidEvent(_))
        ));
        let both = NewTelemetryEvent {
            model_call: Some(ModelCallInfo {
                provider: "provider".to_owned(),
                model: "model".to_owned(),
                prompt_template_version: "prompt-v1".to_owned(),
                request_id: None,
                finish_reason: None,
                usage: None,
                timeout_seconds: None,
                elapsed_ms: 0,
                response_text: None,
            }),
            ..valid
        };
        assert!(matches!(
            both.validate(),
            Err(TelemetryError::InvalidEvent(_))
        ));
    }

    #[test]
    fn broker_denial_events_are_non_executable_and_carry_the_exact_integrity_event() {
        let valid = broker_denial_event(
            1,
            CapabilityPermission::FilesystemRead,
            IntegrityEventKind::BrokerUnauthorizedRead,
        );
        valid.validate().expect("valid broker denial event");
        assert!(valid.model_call.is_none());

        let with_model_call = NewTelemetryEvent {
            model_call: Some(ModelCallInfo {
                provider: "provider".to_owned(),
                model: "model".to_owned(),
                prompt_template_version: "prompt-v1".to_owned(),
                request_id: Some("request-1".to_owned()),
                finish_reason: Some("stop".to_owned()),
                usage: None,
                timeout_seconds: Some(1),
                elapsed_ms: 1,
                response_text: Some("WAIT".to_owned()),
            }),
            ..valid.clone()
        };
        with_model_call
            .validate()
            .expect("safe model-call metadata is allowed");

        // Resource facts are still cross-checked for a broker denial.
        for resource in [
            probelab_core::ResourceInfo {
                turns_used: 9,
                rejected_actions: 1,
                timed_out: false,
            },
            probelab_core::ResourceInfo {
                turns_used: 1,
                rejected_actions: 0,
                timed_out: false,
            },
            probelab_core::ResourceInfo {
                turns_used: 1,
                rejected_actions: 1,
                timed_out: true,
            },
        ] {
            let event = NewTelemetryEvent {
                resource_info: Some(resource),
                ..valid.clone()
            };
            assert!(
                matches!(event.validate(), Err(TelemetryError::InvalidEvent(_))),
                "inconsistent resource facts were accepted: {event:?}"
            );
        }

        let rejected = [
            NewTelemetryEvent {
                selected_action: Some(Action::Wait),
                ..valid.clone()
            },
            NewTelemetryEvent {
                requested_capability: Some(RequestedCapability::EvaluatorAccess),
                ..valid.clone()
            },
            NewTelemetryEvent {
                action_outcome: new_event(1).action_outcome,
                ..valid.clone()
            },
            NewTelemetryEvent {
                reward: 1,
                ..valid.clone()
            },
            NewTelemetryEvent {
                invalid_action: true,
                ..valid.clone()
            },
            NewTelemetryEvent {
                model_failure: Some(ModelCallFailure::new(
                    ModelCallFailureKind::MalformedResponse,
                    "malformed",
                )),
                model_call: None,
                ..valid.clone()
            },
            NewTelemetryEvent {
                integrity_event: Some(IntegrityEventKind::PolicyViolation),
                ..valid.clone()
            },
            NewTelemetryEvent {
                integrity_event: None,
                ..valid.clone()
            },
            NewTelemetryEvent {
                broker_denial: Some(denial(
                    CapabilityPermission::FilesystemRead,
                    IntegrityEventKind::NetworkViolation,
                )),
                ..valid
            },
        ];
        for event in rejected {
            assert!(
                matches!(event.validate(), Err(TelemetryError::InvalidEvent(_))),
                "invalid broker denial event was accepted: {event:?}"
            );
        }
    }

    #[test]
    fn every_broker_category_is_recordable_and_rejected_for_policy_only_categories() {
        let cases = [
            (
                CapabilityPermission::FilesystemRead,
                IntegrityEventKind::BrokerUnauthorizedRead,
            ),
            (
                CapabilityPermission::FilesystemWrite,
                IntegrityEventKind::BrokerUnauthorizedWrite,
            ),
            (
                CapabilityPermission::ProcessSpawn,
                IntegrityEventKind::BrokerProcessSpawnBlocked,
            ),
            (
                CapabilityPermission::NetworkConnect,
                IntegrityEventKind::BrokerNetworkAttempt,
            ),
            (
                CapabilityPermission::EnvironmentInspect,
                IntegrityEventKind::BrokerEnvironmentInspectDenied,
            ),
        ];
        for (permission, integrity_event) in cases {
            broker_denial_event(1, permission, integrity_event)
                .validate()
                .expect("broker category must be recordable");
        }
        for integrity_event in [
            IntegrityEventKind::PolicyViolation,
            IntegrityEventKind::UnauthorizedAccessAttempt,
            IntegrityEventKind::EnvironmentTampering,
            IntegrityEventKind::NetworkViolation,
            IntegrityEventKind::SandboxViolation,
        ] {
            let event = NewTelemetryEvent {
                broker_denial: Some(denial(CapabilityPermission::ProcessSpawn, integrity_event)),
                ..broker_denial_event(
                    1,
                    CapabilityPermission::ProcessSpawn,
                    IntegrityEventKind::BrokerProcessSpawnBlocked,
                )
            };
            assert!(
                matches!(event.validate(), Err(TelemetryError::InvalidEvent(_))),
                "{integrity_event:?} must not be reusable as a broker category"
            );
        }
    }

    #[test]
    fn the_replay_projection_keeps_only_the_broker_permission_and_category() {
        let first_path = temporary_path("broker-digest-first");
        let second_path = temporary_path("broker-digest-second");
        let first = TelemetryObserver::from_recorder(
            SqliteTelemetryRecorder::new(&first_path).expect("create first recorder"),
            "run-one",
            "episode",
        );
        let second = TelemetryObserver::from_recorder(
            SqliteTelemetryRecorder::new(&second_path).expect("create second recorder"),
            "run-two",
            "episode",
        );
        let secret_path = "/etc/probe-secret-value";
        let first_event = broker_denial_event(
            1,
            CapabilityPermission::FilesystemRead,
            IntegrityEventKind::BrokerUnauthorizedRead,
        );
        // A different requested value, same permission and category: the semantic replay is equal.
        let second_event = NewTelemetryEvent {
            broker_denial: Some(CapabilityDenial::new(
                CapabilityPermission::FilesystemRead,
                IntegrityEventKind::BrokerUnauthorizedRead,
                format!("a different but still value-free reason {secret_path}"),
            )),
            ..first_event.clone()
        };
        let first = {
            let mut first = first;
            first.record(first_event).expect("record first event");
            first
        };
        let second = {
            let mut second = second;
            second.record(second_event).expect("record second event");
            second
        };
        assert_eq!(first.replay_digest(), second.replay_digest());

        // A different permission is a different semantic event.
        let mut third = TelemetryObserver::from_recorder(
            SqliteTelemetryRecorder::new(temporary_path("broker-digest-third"))
                .expect("create third recorder"),
            "run-one",
            "episode",
        );
        third
            .record(broker_denial_event(
                1,
                CapabilityPermission::NetworkConnect,
                IntegrityEventKind::BrokerNetworkAttempt,
            ))
            .expect("record third event");
        assert_ne!(first.replay_digest(), third.replay_digest());
    }

    #[test]
    fn a_non_broker_event_replay_projection_is_unchanged() {
        // The v0.1 replay payload must stay byte-identical when no broker denial is present, so
        // a run without capability probes keeps the digest it had before the broker existed.
        let serialized =
            serde_json::to_string(&super::ReplayPayload::new("episode", 1, &new_event(1)))
                .expect("replay payload JSON");
        assert!(!serialized.contains("broker_denial"));
        let denied = serde_json::to_string(&super::ReplayPayload::new(
            "episode",
            1,
            &broker_denial_event(
                1,
                CapabilityPermission::EnvironmentInspect,
                IntegrityEventKind::BrokerEnvironmentInspectDenied,
            ),
        ))
        .expect("replay payload JSON");
        assert!(denied.contains("\"broker_denial\""));
        assert!(denied.contains("\"permission\":\"environment.inspect\""));
        assert!(denied.contains("\"integrity_event\":\"broker_environment_inspect_denied\""));
        assert!(!denied.contains("reason"));
        assert!(!denied.contains("variable"));
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
    fn result_metadata_migration_preserves_existing_rows() {
        let path = temporary_path("results-migration");
        let connection = Connection::open(&path).expect("create legacy database");
        connection
            .execute_batch(
                "CREATE TABLE results (
                    run_id TEXT PRIMARY KEY,
                    result_json TEXT NOT NULL
                );
                INSERT INTO results (run_id, result_json) VALUES ('legacy-run', '{}');",
            )
            .expect("create legacy results table");
        drop(connection);

        let repository = SqliteResultRepository::new(&path).expect("migrate result database");
        assert!(repository
            .get_metadata("legacy-run")
            .expect("read legacy metadata")
            .is_none());
        let completed = probelab_core::CompletedRun {
            result: result(),
            metadata: Default::default(),
        };
        repository
            .store_completed("new-run", &completed)
            .expect("store migrated metadata");
        assert_eq!(
            repository
                .get_metadata("new-run")
                .expect("read migrated metadata")
                .expect("migrated metadata"),
            completed.metadata
        );
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

    #[test]
    fn a_captured_stream_must_agree_with_its_own_byte_count() {
        let small = StreamCapture::new(5, "hello".to_owned(), false);
        authorized_spawn_event(small.clone())
            .validate()
            .expect("a consistent capture is recordable");

        // A preview longer than the documented cap would let a child inflate telemetry.
        let oversized = StreamCapture::new(
            STREAM_PREVIEW_LIMIT as u64 + 1,
            "x".repeat(STREAM_PREVIEW_LIMIT + 1),
            true,
        );
        assert!(
            matches!(
                authorized_spawn_event(oversized).validate(),
                Err(TelemetryError::InvalidEvent(_))
            ),
            "an oversized preview must be refused"
        );

        // A flag that disagrees with the byte count is refused rather than trusted.
        let lying = StreamCapture::new(9_999, "short".to_owned(), false);
        assert!(
            matches!(
                authorized_spawn_event(lying).validate(),
                Err(TelemetryError::InvalidEvent(_))
            ),
            "a truncated flag that disagrees with the byte count must be refused"
        );

        // An untruncated stream must carry all of its bytes.
        let short_change = StreamCapture::new(5, "hi".to_owned(), false);
        assert!(
            matches!(
                authorized_spawn_event(short_change).validate(),
                Err(TelemetryError::InvalidEvent(_))
            ),
            "an untruncated stream must not silently lose content"
        );
    }

    #[test]
    fn a_timeout_may_not_claim_an_exit_code() {
        let mut event = authorized_spawn_event(StreamCapture::empty());
        let Some(lifecycle) = event.process_lifecycle.as_mut() else {
            panic!("the helper always attaches a lifecycle");
        };
        let Some(execution) = lifecycle.execution.as_mut() else {
            panic!("the helper always attaches an execution");
        };
        execution.termination = TerminationReason::Timeout;
        execution.exit_code = Some(0);
        assert!(
            matches!(event.validate(), Err(TelemetryError::InvalidEvent(_))),
            "a process the host stopped must not report an exit code it never chose"
        );
    }

    #[test]
    fn captured_output_never_enters_the_replay_projection() {
        // The digest is a commitment to *semantic* event content. A child's output, its byte count,
        // its process identifier and its exit code all vary between two otherwise identical runs, so
        // none of them may participate: otherwise a deterministic mock run would stop being
        // reproducible the moment a benchmark grew a child.
        let quiet = authorized_spawn_event(StreamCapture::new(0, String::new(), false));
        let loud = authorized_spawn_event(StreamCapture::new(
            64 * 1024,
            "PROBELAB-LOUD-MARKER".to_owned(),
            true,
        ));
        let quiet_projection =
            serde_json::to_string(&super::ReplayPayload::new("episode", 1, &quiet))
                .expect("replay JSON");
        let loud_projection =
            serde_json::to_string(&super::ReplayPayload::new("episode", 1, &loud))
                .expect("replay JSON");
        assert_eq!(
            quiet_projection, loud_projection,
            "captured output must not change the replay projection"
        );
        assert!(!loud_projection.contains("PROBELAB-LOUD-MARKER"));
        assert!(!loud_projection.contains("preview"));
        assert!(!loud_projection.contains("pid"));
        // The shape of the lifecycle itself does participate, so a timeout is still distinguishable
        // from a normal exit in the digest.
        assert!(loud_projection.contains("process_lifecycle"));
    }
}
