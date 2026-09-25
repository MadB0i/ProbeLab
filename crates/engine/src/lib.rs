//! Adapter-neutral orchestration for trusted benchmark execution.

#![deny(missing_docs)]

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use probelab_core::{
    ActionTraceEntry, Agent, Benchmark, CompletedRun, Episode, EpisodeStatus, EvaluationInput,
    Evaluator, ModelCallFailure, ModelCallFailureKind, ModelCallInfo, ModelDecision,
    ModelDescriptor, ModelRequest, ModelResponse, PolicyDecision, PolicyGate, ProtocolAdherence,
    ReproducibilityMetadata, ResourceInfo, RuntimeAuthorization,
    RuntimeGuardian as RuntimeGuardianContract, TelemetryRecorder, TestResult, TokenUsage,
    PROTOCOL_FAILURE_THRESHOLD,
};
use probelab_sandbox::{DenyAllPolicyGate, RuntimeGuardian};
use probelab_telemetry::{NewTelemetryEvent, TelemetryError, TelemetryObserver};

/// A typed failure in the deterministic benchmark execution sequence.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The externally supplied run identifier was empty.
    #[error("run_id must not be empty")]
    EmptyRunId,
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
        mut agent: Box<dyn Agent>,
        evaluator: &dyn Evaluator,
        benchmark_seed: u64,
        run_id: impl Into<String>,
        recorder: Arc<dyn TelemetryRecorder>,
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
        let mut consecutive_protocol_failures = 0_u32;

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
            let request = ModelRequest {
                episode_id: episode_id.clone(),
                turn,
                observation: observation.clone(),
                available_actions: available_actions.clone(),
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
                    let policy_decision = policy.review(decision);
                    let runtime_authorization = guardian.authorize(&policy_decision);
                    verify_authorization(&policy_decision, &runtime_authorization)?;

                    match policy_decision {
                        PolicyDecision::Allow(action) => {
                            let step = sandbox
                                .execute(runtime_authorization)
                                .map_err(EngineError::Sandbox)?;
                            let after = sandbox.episode();
                            validate_transition(&before, &after, step.status, episode_id.as_str())?;
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
                            validate_transition(&before, &after, step.status, episode_id.as_str())?;
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
                                },
                                after,
                                trace,
                            )
                        }
                    }
                }
            };

            let observed = observer.record(input)?;
            if let Some(integrity_event) = observed.integrity_event {
                integrity_events.push(integrity_event);
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
                capability_dimension: summary.capability_dimension.clone(),
                model,
                seed: benchmark_seed,
                episode: episode.clone(),
                integrity_events,
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
    ReproducibilityMetadata {
        provider: descriptor_string(model, "provider").unwrap_or_else(|| model.adapter.clone()),
        model: descriptor_string(model, "model").unwrap_or_else(|| model.strategy.clone()),
        prompt_template_version: descriptor_string(model, "prompt_template_version")
            .unwrap_or_default(),
        configuration_fingerprint: descriptor_string(model, "configuration_fingerprint")
            .unwrap_or_default(),
        environment_version: descriptor_string(model, "environment_version").unwrap_or_default(),
        token_usage: TokenUsage::default(),
        protocol_failure_threshold: PROTOCOL_FAILURE_THRESHOLD,
    }
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
    max_turns: u32,
) -> Result<(), EngineError> {
    if observation.turn != turn {
        return Err(EngineError::EpisodeContract(
            "observation turn does not equal the next consumed turn".to_owned(),
        ));
    }
    if observation.max_turns != max_turns {
        return Err(EngineError::EpisodeContract(
            "sandbox max_turns differs from benchmark summary".to_owned(),
        ));
    }
    Ok(())
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
