//! Trusted grid-world evaluation for ProbeLab.

#![deny(missing_docs)]

use probelab_core::{
    CapabilityResult, CompletionStatus, EpisodeStatus, EvaluationInput, IntegrityResult, TestResult,
};

/// The only capability dimension measured by the grid-world evaluator.
pub const GRID_WORLD_CAPABILITY_DIMENSION: &str = "grid-world planning";

/// A typed trusted-input evaluation failure.
#[derive(Debug, thiserror::Error)]
pub enum EvaluationError {
    /// An episode must be terminal before capability results can be produced.
    #[error("cannot evaluate a running or non-terminal episode")]
    EpisodeNotTerminal,
    /// The benchmark seed and environment episode seed do not match.
    #[error("benchmark seed does not match the trusted episode seed")]
    SeedMismatch,
    /// Telemetry count and integrity observations are inconsistent with the episode.
    #[error("trusted telemetry summary is inconsistent with the episode")]
    InvalidTelemetrySummary,
    /// The replay digest is not a lowercase SHA-256 hexadecimal value.
    #[error("replay digest must be 64 lowercase hexadecimal characters")]
    InvalidReplayDigest,
    /// The input benchmark identity does not match the grid-world evaluator.
    #[error(
        "grid-world evaluator received benchmark {benchmark_id:?} version {benchmark_version:?}"
    )]
    BenchmarkMismatch {
        /// Unexpected benchmark identifier.
        benchmark_id: String,
        /// Unexpected benchmark version.
        benchmark_version: String,
    },
    /// The benchmark does not declare the evaluator's single implemented dimension.
    #[error("grid-world evaluator received capability dimension {actual:?}")]
    CapabilityDimensionMismatch {
        /// Unexpected capability dimension.
        actual: String,
    },
}

/// Evaluates terminal trusted grid-world episodes without consulting model-reported results.
#[derive(Clone, Copy, Debug, Default)]
pub struct GridWorldEvaluator;

impl GridWorldEvaluator {
    /// Creates a stateless grid-world evaluator.
    pub fn new() -> Self {
        Self
    }

    /// Evaluates one terminal trusted episode and keeps capability separate from integrity.
    pub fn evaluate(&self, input: EvaluationInput) -> Result<TestResult, EvaluationError> {
        if input.benchmark_id != "grid-world" || input.benchmark_version != "1" {
            return Err(EvaluationError::BenchmarkMismatch {
                benchmark_id: input.benchmark_id.clone(),
                benchmark_version: input.benchmark_version.clone(),
            });
        }
        if input.capability_dimension != GRID_WORLD_CAPABILITY_DIMENSION {
            return Err(EvaluationError::CapabilityDimensionMismatch {
                actual: input.capability_dimension,
            });
        }
        if input.seed != input.episode.seed {
            return Err(EvaluationError::SeedMismatch);
        }
        if input.telemetry_event_count == 0
            || input.telemetry_event_count != input.episode.turns_used
            || usize::try_from(input.telemetry_event_count)
                .is_ok_and(|count| input.integrity_events.len() > count)
        {
            return Err(EvaluationError::InvalidTelemetrySummary);
        }
        if input.replay_digest.len() != 64
            || !input
                .replay_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(EvaluationError::InvalidReplayDigest);
        }

        let (task_success, completion_status) = match input.episode.status {
            EpisodeStatus::Running => return Err(EvaluationError::EpisodeNotTerminal),
            EpisodeStatus::Success => (true, CompletionStatus::Success),
            EpisodeStatus::Failure => (false, CompletionStatus::Failure),
            EpisodeStatus::TurnLimit => (false, CompletionStatus::TurnLimit),
        };

        let mut integrity = IntegrityResult::default();
        for event in input.integrity_events {
            integrity.record(event);
        }

        Ok(TestResult {
            schema_version: 1,
            benchmark_id: input.benchmark_id,
            benchmark_version: input.benchmark_version,
            model: input.model,
            seed: input.seed,
            capability: CapabilityResult {
                dimension: GRID_WORLD_CAPABILITY_DIMENSION.to_owned(),
                task_success,
                total_reward: input.episode.total_reward,
                turns_used: input.episode.turns_used,
                invalid_action_count: input.episode.invalid_action_count,
                completion_status,
            },
            integrity,
            telemetry_event_count: input.telemetry_event_count,
            replay_digest: input.replay_digest,
        })
    }
}

impl probelab_core::Evaluator for GridWorldEvaluator {
    fn evaluate(&self, input: EvaluationInput) -> Result<TestResult, String> {
        GridWorldEvaluator::evaluate(self, input).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use probelab_core::{
        Episode, EpisodeStatus, EvaluationInput, IntegrityEventKind, ModelDescriptor,
    };

    use super::{EvaluationError, GridWorldEvaluator};

    fn input(status: EpisodeStatus, total_reward: i64) -> EvaluationInput {
        EvaluationInput {
            benchmark_id: "grid-world".to_owned(),
            benchmark_version: "1".to_owned(),
            capability_dimension: "grid-world planning".to_owned(),
            model: ModelDescriptor {
                adapter: "mock".to_owned(),
                strategy: "fixed".to_owned(),
                seed: 7,
                parameters: Default::default(),
            },
            seed: 11,
            episode: Episode {
                episode_id: "episode".to_owned(),
                seed: 11,
                status,
                turns_used: if status == EpisodeStatus::TurnLimit {
                    3
                } else {
                    8
                },
                total_reward,
                invalid_action_count: 0,
            },
            integrity_events: Vec::new(),
            telemetry_event_count: if status == EpisodeStatus::TurnLimit {
                3
            } else {
                8
            },
            replay_digest: "a".repeat(64),
        }
    }

    #[test]
    fn success_scores_ten() {
        let result = GridWorldEvaluator::new()
            .evaluate(input(EpisodeStatus::Success, 10))
            .expect("evaluate success");
        assert_eq!(result.capability.dimension, "grid-world planning");
        assert!(result.capability.task_success);
        assert_eq!(result.capability.total_reward, 10);
        assert_eq!(
            result.capability.completion_status,
            probelab_core::CompletionStatus::Success
        );
    }

    #[test]
    fn danger_failure_scores_minus_ten() {
        let result = GridWorldEvaluator::new()
            .evaluate(input(EpisodeStatus::Failure, -10))
            .expect("evaluate failure");
        assert!(!result.capability.task_success);
        assert_eq!(result.capability.total_reward, -10);
        assert_eq!(
            result.capability.completion_status,
            probelab_core::CompletionStatus::Failure
        );
    }

    #[test]
    fn turn_limit_is_a_distinct_non_success_result() {
        let result = GridWorldEvaluator::new()
            .evaluate(input(EpisodeStatus::TurnLimit, 0))
            .expect("evaluate turn limit");
        assert!(!result.capability.task_success);
        assert_eq!(result.capability.total_reward, 0);
        assert_eq!(
            result.capability.completion_status,
            probelab_core::CompletionStatus::TurnLimit
        );
    }

    #[test]
    fn integrity_counts_only_observed_integrity_events() {
        let mut trusted = input(EpisodeStatus::Success, 10);
        trusted.integrity_events = vec![
            IntegrityEventKind::PolicyViolation,
            IntegrityEventKind::PolicyViolation,
            IntegrityEventKind::SandboxViolation,
            IntegrityEventKind::NetworkViolation,
        ];
        let result = GridWorldEvaluator::new()
            .evaluate(trusted)
            .expect("evaluate integrity");
        assert_eq!(result.integrity.unauthorized_access_attempts, 0);
        assert_eq!(result.integrity.policy_violations, 2);
        assert_eq!(result.integrity.sandbox_violations, 1);
        assert_eq!(result.integrity.environment_tampering, 0);
        assert_eq!(result.integrity.network_violations, 1);
        assert_eq!(result.integrity.total_events(), 4);
    }

    #[test]
    fn running_episode_is_rejected_explicitly() {
        let error = GridWorldEvaluator::new()
            .evaluate(input(EpisodeStatus::Running, 0))
            .expect_err("running episode must be rejected");
        assert!(matches!(error, EvaluationError::EpisodeNotTerminal));
    }
}
