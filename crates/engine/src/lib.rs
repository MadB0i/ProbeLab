//! Adapter-neutral orchestration for trusted benchmark execution.

#![deny(missing_docs)]

use std::sync::Arc;

use probelab_core::{
    Agent, Benchmark, Episode, EpisodeStatus, EvaluationInput, Evaluator, ModelRequest,
    PolicyDecision, PolicyGate, ResourceInfo, RuntimeAuthorization,
    RuntimeGuardian as RuntimeGuardianContract, TelemetryRecorder, TestResult,
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
}

/// Executes the complete model-to-evaluator vertical slice without selecting a model adapter.
#[derive(Clone, Copy, Debug, Default)]
pub struct BenchmarkRunner;

impl BenchmarkRunner {
    /// Creates a stateless benchmark runner.
    pub fn new() -> Self {
        Self
    }

    /// Runs one deterministic benchmark episode with independently observed telemetry.
    pub fn run(
        &self,
        benchmark: &dyn Benchmark,
        mut agent: Box<dyn Agent>,
        evaluator: &dyn Evaluator,
        benchmark_seed: u64,
        run_id: impl Into<String>,
        recorder: Arc<dyn TelemetryRecorder>,
    ) -> Result<TestResult, EngineError> {
        let run_id = run_id.into();
        if run_id.trim().is_empty() {
            return Err(EngineError::EmptyRunId);
        }

        let summary = benchmark.summary();
        let model = agent.descriptor();
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
            let request = ModelRequest {
                episode_id: episode_id.clone(),
                turn,
                observation: observation.clone(),
                available_actions: sandbox.available_actions(),
            };
            request.validate().map_err(EngineError::ModelRequest)?;
            let decision = agent.decide(request).map_err(EngineError::Agent)?;
            let policy_decision = policy.review(decision);
            let runtime_authorization = guardian.authorize(&policy_decision);
            verify_authorization(&policy_decision, &runtime_authorization)?;

            let (input, after) = match policy_decision {
                PolicyDecision::Allow(action) => {
                    let step = sandbox
                        .execute(runtime_authorization)
                        .map_err(EngineError::Sandbox)?;
                    let after = sandbox.episode();
                    validate_transition(&before, &after, step.status, episode_id.as_str())?;
                    let invalid_action = step.action_outcome.as_ref().is_some_and(|outcome| {
                        outcome.kind == probelab_core::ActionResultKind::Invalid
                    });
                    let timed_out = step.status == EpisodeStatus::TurnLimit;
                    (
                        NewTelemetryEvent {
                            turn,
                            observation,
                            selected_action: Some(action),
                            requested_capability: None,
                            action_outcome: step.action_outcome,
                            reward: after.total_reward.saturating_sub(before.total_reward),
                            invalid_action,
                            timeout: timed_out,
                            resource_info: Some(ResourceInfo {
                                turns_used: after.turns_used,
                                rejected_actions: after.invalid_action_count.saturating_add(
                                    u32::try_from(integrity_events.len()).map_err(|_| {
                                        EngineError::EpisodeContract(
                                            "integrity event count exceeds u32".to_owned(),
                                        )
                                    })?,
                                ),
                                timed_out,
                            }),
                            integrity_event: None,
                        },
                        after,
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
                        u32::try_from(integrity_events.len())
                            .map_err(|_| {
                                EngineError::EpisodeContract(
                                    "integrity event count exceeds u32".to_owned(),
                                )
                            })?
                            .saturating_add(1),
                    );
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
                        },
                        after,
                    )
                }
            };

            let observed = observer.record(input)?;
            if let Some(integrity_event) = observed.integrity_event {
                integrity_events.push(integrity_event);
            }
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

        let result = evaluator
            .evaluate(EvaluationInput {
                benchmark_id: summary.benchmark_id,
                benchmark_version: summary.benchmark_version,
                capability_dimension: summary.capability_dimension,
                model,
                seed: benchmark_seed,
                episode: episode.clone(),
                integrity_events,
                telemetry_event_count: observer.event_count(),
                replay_digest: observer.replay_digest(),
            })
            .map_err(EngineError::Evaluation)?;

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
        Ok(result)
    }
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
