//! Adapter-neutral orchestration for trusted benchmark execution.

#![deny(missing_docs)]

pub mod arena;
pub mod harness;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use probelab_broker::{BrokerError, CapabilityBroker};
use probelab_core::{
    ActionTraceEntry, Agent, Benchmark, CapabilityDecision, CapabilityPermission,
    CapabilityRequest, CompletedRun, ContainmentFailure, Episode, EpisodeStatus, EvaluationInput,
    Evaluator, HistoryEntry, ModelCallFailure, ModelCallFailureKind, ModelCallInfo, ModelDecision,
    ModelDescriptor, ModelRequest, ModelResponse, PolicyDecision, PolicyGate, ProcessLifecycle,
    ProcessSpawner, ProtocolAdherence, ReproducibilityMetadata, ResourceInfo, RuntimeAuthorization,
    RuntimeGuardian as RuntimeGuardianContract, TelemetryRecorder, TestResult, TokenUsage,
    HISTORY_WINDOW, PROTOCOL_FAILURE_THRESHOLD,
};
use probelab_sandbox::{ContainedProcessSpawner, DenyAllPolicyGate, RuntimeGuardian};
use probelab_telemetry::{NewTelemetryEvent, TelemetryError, TelemetryObserver};

/// Monotonic counter that keeps concurrent runs from sharing an ephemeral sandbox root.
static NEXT_SANDBOX_ROOT: AtomicU64 = AtomicU64::new(0);

/// Trusted, non-model-facing configuration for exactly one benchmark run.
///
/// The context owns the canonical sandbox root that bounds every capability decision the run's
/// broker may make. Nothing in the context is derived from a model response, and a model is never
/// told that the root exists.
pub struct RunContext {
    sandbox_root: PathBuf,
    owned_root: bool,
}

impl RunContext {
    /// Creates a run context with a fresh private sandbox root under the system temp directory.
    ///
    /// The root is created and canonicalized here, and it is removed when the context is dropped.
    pub fn ephemeral(label: &str) -> Result<Self, EngineError> {
        let sequence = NEXT_SANDBOX_ROOT.fetch_add(1, Ordering::Relaxed);
        let sanitized: String = label
            .chars()
            .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
            .take(32)
            .collect();
        let root = std::env::temp_dir().join(format!(
            "probelab-sandbox-{sanitized}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root)
            .map_err(|error| EngineError::SandboxRoot(error.to_string()))?;
        let canonical = std::fs::canonicalize(&root)
            .map_err(|error| EngineError::SandboxRoot(error.to_string()))?;
        Ok(Self {
            sandbox_root: canonical,
            owned_root: true,
        })
    }

    /// Creates a run context rooted at an existing directory that the caller continues to own.
    pub fn with_sandbox_root(sandbox_root: impl AsRef<Path>) -> Result<Self, EngineError> {
        let requested = sandbox_root.as_ref();
        if requested.as_os_str().is_empty() || !requested.is_absolute() {
            return Err(EngineError::SandboxRoot(
                "sandbox root must be an absolute path".to_owned(),
            ));
        }
        let canonical = std::fs::canonicalize(requested)
            .map_err(|error| EngineError::SandboxRoot(error.to_string()))?;
        if !canonical.is_dir() {
            return Err(EngineError::SandboxRoot(
                "sandbox root is not a directory".to_owned(),
            ));
        }
        Ok(Self {
            sandbox_root: canonical,
            owned_root: false,
        })
    }

    /// Returns the canonical sandbox root that bounds this run's capability decisions.
    pub fn sandbox_root(&self) -> &Path {
        &self.sandbox_root
    }
}

impl Drop for RunContext {
    fn drop(&mut self) {
        if self.owned_root {
            let _ = std::fs::remove_dir_all(&self.sandbox_root);
        }
    }
}

/// A typed failure in the deterministic benchmark execution sequence.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The externally supplied run identifier was empty.
    #[error("run_id must not be empty")]
    EmptyRunId,
    /// The run's canonical sandbox root could not be established.
    #[error("sandbox root could not be established: {0}")]
    SandboxRoot(String),
    /// The capability broker could not be built from the benchmark's policy.
    #[error("capability broker could not be built: {0}")]
    CapabilityBroker(#[from] BrokerError),
    /// A benchmark granted a capability whose execution this milestone does not implement.
    #[error(
        "capability {permission} was authorized but capability execution is not implemented in this milestone"
    )]
    CapabilityExecutionUnsupported {
        /// Permission the broker authorized.
        permission: CapabilityPermission,
    },
    /// A broker-authorized process could not be started inside containment.
    ///
    /// This is a fail-closed failure: the containment layer reports it only after deciding that no
    /// process may run, so reaching this error means the run stopped rather than a child escaping.
    #[error("process containment failed and no process was started: {0}")]
    ProcessContainment(#[from] ContainmentFailure),
    /// A benchmark failed to create its seeded sandbox.
    #[error("benchmark sandbox creation failed: {0}")]
    BenchmarkCreation(String),
    /// A benchmark returned an invalid initial episode.
    #[error("benchmark initial episode is invalid: {0}")]
    InvalidInitialEpisode(String),
    /// The sandbox violated the engine's turn or terminal-state contract.
    #[error("sandbox episode contract failed: {0}")]
    EpisodeContract(String),
    /// The configured benchmark turn budget was exhausted without terminal state.
    #[error("sandbox failed to terminate at max_turns={max_turns}")]
    TurnBudgetExhausted {
        /// Maximum turns declared by the benchmark summary.
        max_turns: u32,
    },
    /// The model request failed trusted cross-field validation.
    #[error("model request validation failed: {0}")]
    ModelRequest(String),
    /// The agent failed to produce a decision.
    #[error("agent decision failed: {0}")]
    Agent(String),
    /// The runtime guardian did not reproduce the policy decision exactly.
    #[error(
        "runtime guardian authorization {runtime_authorization:?} does not match policy decision {policy_decision:?}"
    )]
    GuardianAuthorizationMismatch {
        /// Policy decision that was reviewed.
        policy_decision: String,
        /// Authorization returned by the runtime guardian.
        runtime_authorization: String,
    },
    /// The sandbox rejected an otherwise authorized transition.
    #[error("sandbox transition failed: {0}")]
    Sandbox(String),
    /// The independent observer or durable recorder failed.
    #[error(transparent)]
    Telemetry(#[from] TelemetryError),
    /// The trusted evaluator rejected the completed episode.
    #[error("evaluation failed: {0}")]
    Evaluation(String),
    /// The system clock could not produce a valid Unix-millisecond timestamp.
    #[error("system clock failed: {0}")]
    SystemClock(String),
}

/// Executes the complete model-to-evaluator vertical slice without selecting a model adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct BenchmarkRunner;

impl BenchmarkRunner {
    /// Creates a stateless benchmark runner.
    pub fn new() -> Self {
        Self
    }

    /// Runs one benchmark episode and returns the v0.1-compatible deterministic result.
    pub fn run(
        &self,
        benchmark: &dyn Benchmark,
        agent: Box<dyn Agent>,
        evaluator: &dyn Evaluator,
        benchmark_seed: u64,
        run_id: impl Into<String>,
        recorder: Arc<dyn TelemetryRecorder>,
    ) -> Result<TestResult, EngineError> {
        self.run_with_metadata(
            benchmark,
            agent,
            evaluator,
            benchmark_seed,
            run_id,
            recorder,
        )
        .map(|completed| completed.result)
    }

    /// Runs one benchmark episode and returns its result with durable execution metadata.
    pub fn run_with_metadata(
        &self,
        benchmark: &dyn Benchmark,
        agent: Box<dyn Agent>,
        evaluator: &dyn Evaluator,
        benchmark_seed: u64,
        run_id: impl Into<String>,
        recorder: Arc<dyn TelemetryRecorder>,
    ) -> Result<CompletedRun, EngineError> {
        let run_id = run_id.into();
        if run_id.trim().is_empty() {
            return Err(EngineError::EmptyRunId);
        }
        let context = RunContext::ephemeral(&run_id)?;
        self.run_with_context(
            benchmark,
            agent,
            evaluator,
            benchmark_seed,
            run_id,
            recorder,
            context,
        )
    }

    /// Runs one benchmark episode in a caller-supplied trusted run context.
    ///
    /// The context supplies the canonical sandbox root for this run's capability broker. The
    /// broker is built from `benchmark.capability_policy()`, so a benchmark that grants nothing
    /// cannot be widened by the engine. A real model action still travels
    /// agent → policy gate → runtime guardian → sandbox, unchanged.
    // The trusted run path stays fully explicit: every input is named at the call site rather
    // than hidden in an options struct, so no run can silently inherit a default.
    #[allow(clippy::too_many_arguments)]
    pub fn run_with_context(
        &self,
        benchmark: &dyn Benchmark,
        mut agent: Box<dyn Agent>,
        evaluator: &dyn Evaluator,
        benchmark_seed: u64,
        run_id: impl Into<String>,
        recorder: Arc<dyn TelemetryRecorder>,
        context: RunContext,
    ) -> Result<CompletedRun, EngineError> {
        let run_id = run_id.into();
        if run_id.trim().is_empty() {
            return Err(EngineError::EmptyRunId);
        }
        let started_at_unix_ms = unix_milliseconds()?;

        let summary = benchmark.summary();
        let model = agent.descriptor();
        let mut reproducibility = reproducibility_from_descriptor(&model);
        let mut token_usage = TokenUsage::default();
        let mut protocol_adherence = ProtocolAdherence::default();
        let mut action_trace = Vec::new();
        tracing::info!(
            run_id = %run_id,
            benchmark_id = %summary.benchmark_id,
            benchmark_version = %summary.benchmark_version,
            benchmark_seed,
            model_adapter = %model.adapter,
            model_strategy = %model.strategy,
            model_seed = model.seed,
            "benchmark run started"
        );

        let broker = CapabilityBroker::new(benchmark.capability_policy(), context.sandbox_root())?;
        // The spawner is built from trusted run configuration only. Its designated directory lives
        // inside this run's own sandbox root, so a child can never write outside the run that started
        // it, and the paths it must not read are the benchmark's own declarations.
        let spawner = ContainedProcessSpawner::new(
            context.sandbox_root().join("spawn"),
            benchmark.protected_paths().to_vec(),
        );
        tracing::debug!(
            run_id = %run_id,
            capability_policy = broker.policy().is_deny_all(),
            "capability broker built for run"
        );
        let mut sandbox = benchmark
            .create_sandbox(benchmark_seed)
            .map_err(EngineError::BenchmarkCreation)?;
        let initial_episode = sandbox.episode();
        validate_initial_episode(&initial_episode, benchmark_seed)?;
        let episode_id = initial_episode.episode_id.clone();
        let mut observer = TelemetryObserver::new(recorder, run_id.clone(), episode_id.clone());
        let policy = DenyAllPolicyGate;
        let guardian = RuntimeGuardian;
        let mut episode = initial_episode;
        let mut integrity_events = Vec::new();
        let mut broker_denials = Vec::new();
        let mut process_lifecycles = Vec::new();
        let mut consecutive_protocol_failures = 0_u32;
        let mut history_window_state: VecDeque<HistoryEntry> = VecDeque::new();

        while episode.status == EpisodeStatus::Running {
            if episode.turns_used >= summary.max_turns {
                return Err(EngineError::TurnBudgetExhausted {
                    max_turns: summary.max_turns,
                });
            }

            let before = episode.clone();
            let observation = sandbox.observation();
            let turn = before.turns_used.checked_add(1).ok_or_else(|| {
                EngineError::EpisodeContract("episode turn counter is exhausted".to_owned())
            })?;
            validate_observation_contract(&observation, turn, summary.max_turns)?;
            let available_actions = sandbox.available_actions();
            let observed_position = observation.position;
            let observed_tile = observation.current_tile;
            let observed_neighbors = observation.visible_neighbors.clone();
            let history = history_window(&history_window_state);
            // Whether the process permission is on offer is a property of the benchmark, read from
            // the trusted plan rather than from anything the model said. `process_spawn_plan()` is
            // the same call the execution path uses, so a model is never told a process is available
            // unless the engine would actually be able to run one.
            let process_spawn_available = benchmark.process_spawn_plan().is_some();
            let request = ModelRequest {
                episode_id: episode_id.clone(),
                turn,
                observation: observation.clone(),
                available_actions: available_actions.clone(),
                process_spawn_available,
                history,
                // The engine drives the grid environment, so it builds a grid request. A game arena
                // needs a different step path and a different event payload; it does not go through
                // this loop, and pretending otherwise here would mean a game request with a grid
                // position attached.
                game: None,
            };
            request.validate().map_err(EngineError::ModelRequest)?;
            let raw_response = agent.decide_with_metadata(request);
            if let Ok(response) = &raw_response {
                if let Some(call) = &response.call {
                    apply_call_metadata(&reproducibility, call).map_err(EngineError::Agent)?;
                }
            }
            let call_result = match raw_response {
                Ok(response) => {
                    if let ModelDecision::GridAction(action) = &response.decision {
                        if !available_actions.contains(action) {
                            Err(invalid_action_failure(*action, &response.call))
                        } else {
                            Ok(response)
                        }
                    } else {
                        Ok(response)
                    }
                }
                Err(failure) => Err((failure, None)),
            };

            let (input, after, trace) = match call_result {
                Err((failure, selected_action)) => {
                    let failure = bounded_failure(failure);
                    if let Some(usage) = &failure.usage {
                        token_usage.add_assign(usage);
                    }
                    protocol_adherence.record_failure(failure.kind);
                    consecutive_protocol_failures = consecutive_protocol_failures.saturating_add(1);
                    let step = sandbox
                        .consume_protocol_failure_turn(
                            consecutive_protocol_failures >= PROTOCOL_FAILURE_THRESHOLD,
                        )
                        .map_err(EngineError::Sandbox)?;
                    let after = sandbox.episode();
                    validate_transition(&before, &after, step.status, episode_id.as_str())?;
                    let timed_out = after.status == EpisodeStatus::TurnLimit;
                    let rejected_actions = after
                        .invalid_action_count
                        .saturating_add(integrity_event_count(&integrity_events)?)
                        .saturating_add(1);
                    let trace = ActionTraceEntry {
                        turn,
                        selected_action,
                        model_response_text: failure.response_text.clone(),
                        parsed_action: selected_action,
                        model_failure_kind: Some(failure.kind),
                        ..ActionTraceEntry::default()
                    };
                    (
                        NewTelemetryEvent {
                            turn,
                            observation,
                            selected_action,
                            // The engine loop is the grid path today. A game benchmark supplies its
                            // own request and event payload; until one is wired here this stays None,
                            // which is what keeps grid event bytes unchanged.
                            game: None,
                            requested_capability: None,
                            action_outcome: None,
                            reward: 0,
                            invalid_action: false,
                            timeout: timed_out,
                            resource_info: Some(ResourceInfo {
                                turns_used: after.turns_used,
                                rejected_actions,
                                timed_out,
                            }),
                            integrity_event: None,
                            model_call: None,
                            model_failure: Some(failure),
                            broker_denial: None,
                            process_lifecycle: None,
                        },
                        after,
                        trace,
                    )
                }
                Ok(response) => {
                    consecutive_protocol_failures = 0;
                    protocol_adherence.record_valid_response();
                    let ModelResponse {
                        decision,
                        call: model_call,
                    } = response;
                    let model_call = model_call.map(bounded_model_call);
                    if let Some(usage) = model_call.as_ref().and_then(|call| call.usage.as_ref()) {
                        token_usage.add_assign(usage);
                    }
                    // Both a capability probe and a process-permission request go to the broker and
                    // never to the policy gate. Neither carries a program the model chose: a probe's
                    // own framing is discarded and rebuilt from the benchmark's plan by
                    // `spawn_request_for`, and `ProcessSpawnRequest` carries nothing at all. That is
                    // what lets the request be reachable from a real model without giving one the
                    // ability to name a command line. Non-spawn probes are routed unchanged, exactly
                    // as before, so every permission still gets a broker decision.
                    let broker_probe = match &decision {
                        ModelDecision::CapabilityProbe(probe) => Some(probe.clone()),
                        ModelDecision::ProcessSpawnRequest => {
                            Some(CapabilityRequest::ProcessSpawn {
                                program: String::new(),
                                args: Vec::new(),
                            })
                        }
                        _ => None,
                    };
                    if let Some(probe) = broker_probe {
                        // A capability probe is internal to the trusted path: it is reviewed by
                        // the broker and never by the policy gate, the runtime guardian, or the
                        // sandbox. A refusal consumes a turn and is recorded as a broker denial,
                        // never as a protocol failure and never as an invalid environment action.
                        //
                        // For `process.spawn` the request the broker actually reviews is rebuilt from
                        // the benchmark's trusted plan. A model can ask for the permission but can
                        // never name the program, so there is no path by which a model response
                        // reaches the allowlist comparison or a command line.
                        let reviewed = spawn_request_for(benchmark, &probe);
                        match broker.review(&reviewed) {
                            CapabilityDecision::Deny(denial) => {
                                let step = sandbox
                                    .consume_denied_turn(denial.integrity_event)
                                    .map_err(EngineError::Sandbox)?;
                                let after = sandbox.episode();
                                validate_transition(
                                    &before,
                                    &after,
                                    step.status,
                                    episode_id.as_str(),
                                )?;
                                let timed_out = step.status == EpisodeStatus::TurnLimit;
                                let rejected_actions = after.invalid_action_count.saturating_add(
                                    integrity_event_count(&integrity_events)?.saturating_add(1),
                                );
                                broker_denials.push(denial.clone());
                                tracing::debug!(
                                    run_id = %run_id,
                                    turn,
                                    permission = %denial.permission.as_str(),
                                    integrity_event = ?denial.integrity_event,
                                    "capability broker refused a named capability"
                                );
                                let trace = ActionTraceEntry {
                                    turn,
                                    model_response_text: model_call
                                        .as_ref()
                                        .and_then(|call| call.response_text.clone()),
                                    ..ActionTraceEntry::default()
                                };
                                (
                                    NewTelemetryEvent {
                                        turn,
                                        observation,
                                        selected_action: None,
                                        // Grid path; see the note on the first event site.
                                        game: None,
                                        requested_capability: None,
                                        action_outcome: None,
                                        reward: 0,
                                        invalid_action: false,
                                        timeout: timed_out,
                                        resource_info: Some(ResourceInfo {
                                            turns_used: after.turns_used,
                                            rejected_actions,
                                            timed_out,
                                        }),
                                        integrity_event: Some(denial.integrity_event),
                                        model_call,
                                        model_failure: None,
                                        broker_denial: Some(denial),
                                        process_lifecycle: None,
                                    },
                                    after,
                                    trace,
                                )
                            }
                            CapabilityDecision::Allow => {
                                // Authorization is not execution. `process.spawn` is the one
                                // permission with a real implementation, and the request the broker
                                // approved is rebuilt from the trusted plan rather than from anything
                                // the model named. Every other permission stops the run explicitly
                                // instead of silently doing nothing, and none of them is given a
                                // pretend execution in this milestone.
                                if probe.permission() != CapabilityPermission::ProcessSpawn {
                                    return Err(EngineError::CapabilityExecutionUnsupported {
                                        permission: probe.permission(),
                                    });
                                }
                                let plan = benchmark.process_spawn_plan().ok_or(
                                    EngineError::CapabilityExecutionUnsupported {
                                        permission: probe.permission(),
                                    },
                                )?;
                                let execution = spawner
                                    .spawn_contained(&plan)
                                    .map_err(EngineError::ProcessContainment)?;
                                let step = sandbox
                                    .consume_capability_turn()
                                    .map_err(EngineError::Sandbox)?;
                                let after = sandbox.episode();
                                validate_transition(
                                    &before,
                                    &after,
                                    step.status,
                                    episode_id.as_str(),
                                )?;
                                let timed_out = step.status == EpisodeStatus::TurnLimit;
                                let rejected_actions = after.invalid_action_count;
                                let lifecycle = ProcessLifecycle::authorized(execution);
                                process_lifecycles.push(lifecycle.clone());
                                tracing::info!(
                                    run_id = %run_id,
                                    turn,
                                    pid = lifecycle.execution().and_then(|record| record.pid),
                                    exit_code = lifecycle.execution().and_then(|record| record.exit_code),
                                    termination = ?lifecycle.execution().map(|record| record.termination),
                                    containment = ?lifecycle.execution().map(|record| record.containment),
                                    "authorized process completed inside containment"
                                );
                                let trace = ActionTraceEntry {
                                    turn,
                                    model_response_text: model_call
                                        .as_ref()
                                        .and_then(|call| call.response_text.clone()),
                                    ..ActionTraceEntry::default()
                                };
                                (
                                    NewTelemetryEvent {
                                        turn,
                                        observation,
                                        selected_action: None,
                                        // Grid path; see the note on the first event site.
                                        game: None,
                                        requested_capability: None,
                                        action_outcome: None,
                                        reward: 0,
                                        invalid_action: false,
                                        timeout: timed_out,
                                        resource_info: Some(ResourceInfo {
                                            turns_used: after.turns_used,
                                            rejected_actions,
                                            timed_out,
                                        }),
                                        integrity_event: None,
                                        model_call,
                                        model_failure: None,
                                        broker_denial: None,
                                        process_lifecycle: Some(lifecycle),
                                    },
                                    after,
                                    trace,
                                )
                            }
                        }
                    } else {
                        let policy_decision = policy.review(decision);
                        let runtime_authorization = guardian.authorize(&policy_decision);
                        verify_authorization(&policy_decision, &runtime_authorization)?;

                        match policy_decision {
                            PolicyDecision::Allow(action) => {
                                let step = sandbox
                                    .execute(runtime_authorization)
                                    .map_err(EngineError::Sandbox)?;
                                let after = sandbox.episode();
                                validate_transition(
                                    &before,
                                    &after,
                                    step.status,
                                    episode_id.as_str(),
                                )?;
                                let action_outcome = step.action_outcome.ok_or_else(|| {
                                    EngineError::EpisodeContract(
                                        "authorized action did not return an outcome".to_owned(),
                                    )
                                })?;
                                let invalid_action =
                                    action_outcome.kind == probelab_core::ActionResultKind::Invalid;
                                let timed_out = step.status == EpisodeStatus::TurnLimit;
                                let reward = after.total_reward.saturating_sub(before.total_reward);
                                let trace = ActionTraceEntry {
                                    turn,
                                    selected_action: Some(action),
                                    model_response_text: model_call
                                        .as_ref()
                                        .and_then(|call| call.response_text.clone()),
                                    parsed_action: Some(action),
                                    action_result_kind: Some(action_outcome.kind),
                                    reward,
                                    ..ActionTraceEntry::default()
                                };
                                (
                                    NewTelemetryEvent {
                                        turn,
                                        observation,
                                        selected_action: Some(action),
                                        // Grid path; see the note on the first event site.
                                        game: None,
                                        requested_capability: None,
                                        action_outcome: Some(action_outcome),
                                        reward,
                                        invalid_action,
                                        timeout: timed_out,
                                        resource_info: Some(ResourceInfo {
                                            turns_used: after.turns_used,
                                            rejected_actions: after
                                                .invalid_action_count
                                                .saturating_add(integrity_event_count(
                                                    &integrity_events,
                                                )?),
                                            timed_out,
                                        }),
                                        integrity_event: None,
                                        model_call,
                                        model_failure: None,
                                        broker_denial: None,
                                        process_lifecycle: None,
                                    },
                                    after,
                                    trace,
                                )
                            }
                            PolicyDecision::Deny {
                                request,
                                integrity_event,
                            } => {
                                let step = sandbox
                                    .consume_denied_turn(integrity_event)
                                    .map_err(EngineError::Sandbox)?;
                                let after = sandbox.episode();
                                validate_transition(
                                    &before,
                                    &after,
                                    step.status,
                                    episode_id.as_str(),
                                )?;
                                let timed_out = step.status == EpisodeStatus::TurnLimit;
                                let rejected_actions = after.invalid_action_count.saturating_add(
                                    integrity_event_count(&integrity_events)?.saturating_add(1),
                                );
                                let trace = ActionTraceEntry {
                                    turn,
                                    requested_capability: Some(request.clone()),
                                    model_response_text: model_call
                                        .as_ref()
                                        .and_then(|call| call.response_text.clone()),
                                    ..ActionTraceEntry::default()
                                };
                                (
                                    NewTelemetryEvent {
                                        turn,
                                        observation,
                                        selected_action: None,
                                        // Grid path; see the note on the first event site.
                                        game: None,
                                        requested_capability: Some(request),
                                        action_outcome: step.action_outcome,
                                        reward: 0,
                                        invalid_action: false,
                                        timeout: timed_out,
                                        resource_info: Some(ResourceInfo {
                                            turns_used: after.turns_used,
                                            rejected_actions,
                                            timed_out,
                                        }),
                                        integrity_event: Some(integrity_event),
                                        model_call,
                                        model_failure: None,
                                        broker_denial: None,
                                        process_lifecycle: None,
                                    },
                                    after,
                                    trace,
                                )
                            }
                        }
                    }
                }
            };

            let observed = observer.record(input)?;
            if let Some(integrity_event) = observed.integrity_event {
                integrity_events.push(integrity_event);
            }
            // Only a turn the model actually decided and the environment actually executed becomes
            // history. A broker refusal or a protocol failure consumed a turn but produced no
            // decision, so there is nothing for the model to remember about it.
            if let (Some(action), Some(result_kind)) =
                (trace.selected_action, trace.action_result_kind)
            {
                history_window_state.push_back(HistoryEntry {
                    turn,
                    position: observed_position,
                    current_tile: observed_tile,
                    visible_neighbors: observed_neighbors,
                    selected_action: action,
                    result_kind,
                    reward: trace.reward,
                });
            }
            action_trace.push(trace);
            if after.status == EpisodeStatus::Running && after.turns_used >= summary.max_turns {
                return Err(EngineError::EpisodeContract(
                    "turn limit reached while episode is still running".to_owned(),
                ));
            }
            episode = after;
        }

        if episode.status == EpisodeStatus::Running {
            return Err(EngineError::EpisodeContract(
                "run ended without a terminal episode".to_owned(),
            ));
        }
        if episode.turns_used != observer.event_count() {
            return Err(EngineError::EpisodeContract(
                "terminal turns and durable observer events differ".to_owned(),
            ));
        }

        reproducibility.token_usage = token_usage.clone();
        let result = evaluator
            .evaluate(EvaluationInput {
                benchmark_id: summary.benchmark_id.clone(),
                benchmark_version: summary.benchmark_version.clone(),
                // The digest travels with the version so a stored episode describes the exact task it ran,
                // not merely a string someone remembered to bump.
                benchmark_definition_digest: Some(summary.definition_digest.clone()),
                capability_dimension: summary.capability_dimension.clone(),
                model,
                seed: benchmark_seed,
                episode: episode.clone(),
                integrity_events,
                broker_denials,
                process_lifecycles,
                telemetry_event_count: observer.event_count(),
                replay_digest: observer.replay_digest(),
                protocol_adherence: protocol_adherence.clone(),
                reproducibility: reproducibility.clone(),
            })
            .map_err(EngineError::Evaluation)?;

        let ended_at_unix_ms = unix_milliseconds()?;
        let metadata = probelab_core::RunMetadata {
            schema_version: 2,
            benchmark_id: summary.benchmark_id,
            benchmark_version: summary.benchmark_version,
            benchmark_definition_digest: Some(summary.definition_digest),
            provider: reproducibility.provider,
            model: reproducibility.model,
            prompt_template_version: reproducibility.prompt_template_version,
            configuration_fingerprint: reproducibility.configuration_fingerprint,
            environment_version: reproducibility.environment_version,
            started_at_unix_ms,
            ended_at_unix_ms,
            action_trace,
            token_usage,
            protocol_failure_threshold: PROTOCOL_FAILURE_THRESHOLD,
        };

        tracing::info!(
            run_id = %run_id,
            benchmark_id = %result.benchmark_id,
            benchmark_version = %result.benchmark_version,
            benchmark_seed,
            model_adapter = %result.model.adapter,
            model_strategy = %result.model.strategy,
            model_seed = result.model.seed,
            "benchmark run completed"
        );
        Ok(CompletedRun { result, metadata })
    }
}

fn invalid_action_failure(
    action: probelab_core::Action,
    call: &Option<ModelCallInfo>,
) -> (ModelCallFailure, Option<probelab_core::Action>) {
    (
        ModelCallFailure {
            kind: ModelCallFailureKind::InvalidAction,
            message: "model response contained an action outside the request vocabulary".to_owned(),
            response_text: call
                .as_ref()
                .and_then(|metadata| metadata.response_text.clone()),
            usage: call.as_ref().and_then(|metadata| metadata.usage.clone()),
            elapsed_ms: call.as_ref().map(|metadata| metadata.elapsed_ms),
        },
        Some(action),
    )
}

fn apply_call_metadata(
    reproducibility: &ReproducibilityMetadata,
    call: &ModelCallInfo,
) -> Result<(), String> {
    if !call.provider.trim().is_empty() && reproducibility.provider != call.provider {
        return Err("provider metadata changed during the run".to_owned());
    }
    if !call.model.trim().is_empty() && reproducibility.model != call.model {
        return Err("model metadata changed during the run".to_owned());
    }
    if !call.prompt_template_version.trim().is_empty()
        && reproducibility.prompt_template_version != call.prompt_template_version
    {
        return Err("prompt template metadata changed during the run".to_owned());
    }
    Ok(())
}

fn bounded_text(value: String) -> String {
    value.chars().take(2048).collect()
}

fn bounded_model_call(mut call: ModelCallInfo) -> ModelCallInfo {
    call.response_text = call.response_text.map(bounded_text);
    call
}

fn bounded_failure(mut failure: ModelCallFailure) -> ModelCallFailure {
    failure.message = bounded_text(failure.message);
    failure.response_text = failure.response_text.map(bounded_text);
    failure
}

fn reproducibility_from_descriptor(model: &ModelDescriptor) -> ReproducibilityMetadata {
    // `..ReproducibilityMetadata::default()` rather than a full literal, deliberately. This function
    // is the single place a result's reproducibility record is built, and a full struct literal means
    // every field added later is silently empty here unless someone remembers -- which is precisely
    // how `probelab_version` and `commit_sha` were going to end up recorded as blank in every stored
    // result while looking correct in review.
    ReproducibilityMetadata {
        provider: descriptor_string(model, "provider").unwrap_or_else(|| model.adapter.clone()),
        model: descriptor_string(model, "model").unwrap_or_else(|| model.strategy.clone()),
        prompt_template_version: descriptor_string(model, "prompt_template_version")
            .unwrap_or_default(),
        configuration_fingerprint: descriptor_string(model, "configuration_fingerprint")
            .unwrap_or_default(),
        environment_version: descriptor_string(model, "environment_version").unwrap_or_default(),
        ..ReproducibilityMetadata::default()
    }
    // And the build identity, stamped by the one constructor that means "a run is happening".
    .for_run()
}

fn descriptor_string(descriptor: &ModelDescriptor, key: &str) -> Option<String> {
    descriptor
        .parameters
        .get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

fn integrity_event_count(events: &[probelab_core::IntegrityEventKind]) -> Result<u32, EngineError> {
    u32::try_from(events.len())
        .map_err(|_| EngineError::EpisodeContract("integrity event count exceeds u32".to_owned()))
}

/// Returns the request the broker is asked to review for one capability probe.
///
/// Every permission except `process.spawn` is reviewed exactly as the model framed it, because the
/// probe is a request and the broker's job is to refuse it. A spawn request is different: the program
/// comes from the benchmark's trusted plan, so the reviewed request cannot be influenced by a model
/// response. When the benchmark declares no spawn plan at all, the reviewed request names an empty
/// program, which the broker always refuses, so "this benchmark exposes no process capability" and
/// "this program is not on the allowlist" cannot be confused for an authorization.
fn spawn_request_for(benchmark: &dyn Benchmark, probe: &CapabilityRequest) -> CapabilityRequest {
    if probe.permission() != CapabilityPermission::ProcessSpawn {
        return probe.clone();
    }
    match benchmark.process_spawn_plan() {
        Some(plan) => plan.to_request(),
        None => CapabilityRequest::ProcessSpawn {
            program: String::new(),
            args: Vec::new(),
        },
    }
}

fn unix_milliseconds() -> Result<i64, EngineError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| EngineError::SystemClock(error.to_string()))?;
    i64::try_from(duration.as_millis()).map_err(|error| EngineError::SystemClock(error.to_string()))
}

fn validate_initial_episode(episode: &Episode, benchmark_seed: u64) -> Result<(), EngineError> {
    if episode.episode_id.trim().is_empty() {
        return Err(EngineError::InvalidInitialEpisode(
            "episode_id must not be empty".to_owned(),
        ));
    }
    if episode.seed != benchmark_seed {
        return Err(EngineError::InvalidInitialEpisode(
            "episode seed differs from benchmark seed".to_owned(),
        ));
    }
    if episode.status != EpisodeStatus::Running || episode.turns_used != 0 {
        return Err(EngineError::InvalidInitialEpisode(
            "new episode must be running with zero consumed turns".to_owned(),
        ));
    }
    Ok(())
}

fn validate_observation_contract(
    observation: &probelab_core::Observation,
    turn: u32,
    catalog_max_turns: u32,
) -> Result<(), EngineError> {
    if observation.turn != turn {
        return Err(EngineError::EpisodeContract(
            "observation turn does not equal the next consumed turn".to_owned(),
        ));
    }
    // The environment owns its episode length, and since the grid-world redesign that length is
    // derived per seed from its own shortest safe route. The catalog number is an upper bound over
    // every seed, so an environment may legitimately run shorter but must never run longer: that
    // keeps the published ceiling meaningful instead of decorative.
    if observation.max_turns == 0 || observation.max_turns > catalog_max_turns {
        return Err(EngineError::EpisodeContract(
            "sandbox max_turns must be non-zero and within the benchmark catalog ceiling"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Materialises the bounded history window handed to the model.
///
/// The window is capped at [`HISTORY_WINDOW`] entries and drops the oldest first, so a request's
/// size is bounded no matter how long the episode runs. This is the whole point of the bound: an
/// unbounded transcript would let per-turn cost grow without limit and would quietly make
/// long-horizon reasoning a privilege of long budgets.
fn history_window(history: &VecDeque<HistoryEntry>) -> Vec<HistoryEntry> {
    history
        .iter()
        .rev()
        .take(HISTORY_WINDOW)
        .rev()
        .cloned()
        .collect()
}

fn verify_authorization(
    decision: &PolicyDecision,
    authorization: &RuntimeAuthorization,
) -> Result<(), EngineError> {
    let matches = match (decision, authorization) {
        (PolicyDecision::Allow(policy_action), RuntimeAuthorization::Execute(allowed_action)) => {
            policy_action == allowed_action
        }
        (
            PolicyDecision::Deny {
                integrity_event: policy_event,
                ..
            },
            RuntimeAuthorization::Reject(guardian_event),
        ) => policy_event == guardian_event,
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(EngineError::GuardianAuthorizationMismatch {
            policy_decision: format!("{decision:?}"),
            runtime_authorization: format!("{authorization:?}"),
        })
    }
}

fn validate_transition(
    before: &Episode,
    after: &Episode,
    transition_status: EpisodeStatus,
    episode_id: &str,
) -> Result<(), EngineError> {
    let expected_turns = before.turns_used.checked_add(1).ok_or_else(|| {
        EngineError::EpisodeContract("episode turn counter is exhausted".to_owned())
    })?;
    if after.turns_used != expected_turns {
        return Err(EngineError::EpisodeContract(format!(
            "turn did not advance exactly once: before={}, after={}",
            before.turns_used, after.turns_used
        )));
    }
    if after.episode_id != episode_id || after.seed != before.seed {
        return Err(EngineError::EpisodeContract(
            "episode identity changed during execution".to_owned(),
        ));
    }
    if transition_status != after.status {
        return Err(EngineError::EpisodeContract(
            "transition status does not match terminal episode snapshot".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{verify_authorization, EngineError};
    use probelab_core::{
        Action, IntegrityEventKind, PolicyDecision, RequestedCapability, RuntimeAuthorization,
    };

    #[test]
    fn mismatched_authorization_fails_closed() {
        let decision = PolicyDecision::Allow(Action::Right);
        let error = verify_authorization(&decision, &RuntimeAuthorization::Execute(Action::Left))
            .expect_err("mismatched authorization");
        assert!(matches!(
            error,
            EngineError::GuardianAuthorizationMismatch { .. }
        ));
    }

    #[test]
    fn authorization_shape_mismatch_fails_closed() {
        let decision = PolicyDecision::Deny {
            request: RequestedCapability::SandboxEscape,
            integrity_event: IntegrityEventKind::SandboxViolation,
        };
        let error = verify_authorization(&decision, &RuntimeAuthorization::Execute(Action::Wait))
            .expect_err("shape mismatch");
        assert!(matches!(
            error,
            EngineError::GuardianAuthorizationMismatch { .. }
        ));
    }
}
