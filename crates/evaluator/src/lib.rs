//! Trusted grid-world evaluation for ProbeLab.

#![deny(missing_docs)]

use std::collections::BTreeMap;

use probelab_core::{
    CapabilityResult, CompletionStatus, EpisodeStatus, EvaluationInput, IntegrityEventKind,
    IntegrityResult, TestResult,
};

/// The only capability dimension measured by the grid-world evaluator.
///
/// This is the same string the grid-world benchmark declares, so the evaluator adds no second or
/// placeholder dimension: a capability result carries this one game-performance dimension plus
/// model protocol adherence, and nothing else.
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
    /// Protocol counts do not match the trusted model-call and event counts.
    #[error("protocol adherence counts are inconsistent with trusted telemetry")]
    InvalidProtocolSummary,
    /// Broker denials do not match the independently observed integrity events.
    #[error("broker denial counts are inconsistent with trusted telemetry")]
    InvalidBrokerSummary,
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
        validate_protocol_adherence(&input)?;
        validate_broker_denials(&input)?;
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
            EpisodeStatus::ModelProtocolFailure => (false, CompletionStatus::ModelProtocolFailure),
        };

        let mut integrity = IntegrityResult::default();
        for event in input.integrity_events {
            integrity.record(event);
        }

        Ok(TestResult {
            schema_version: 2,
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
                protocol_adherence: input.protocol_adherence,
            },
            integrity,
            telemetry_event_count: input.telemetry_event_count,
            replay_digest: input.replay_digest,
            reproducibility: input.reproducibility,
        })
    }
}

fn validate_protocol_adherence(input: &EvaluationInput) -> Result<(), EvaluationError> {
    let protocol = &input.protocol_adherence;
    let has_any_protocol_count = protocol.total_model_calls != 0
        || protocol.valid_responses != 0
        || protocol.parse_failures != 0
        || protocol.invalid_actions != 0
        || protocol.timeouts != 0
        || protocol.provider_failures != 0
        || protocol.http_failures != 0
        || protocol.transport_failures != 0
        || protocol.configuration_failures != 0;
    if !has_any_protocol_count {
        return Ok(());
    }
    let failure_total = protocol
        .parse_failures
        .saturating_add(protocol.invalid_actions)
        .saturating_add(protocol.timeouts)
        .saturating_add(protocol.provider_failures)
        .saturating_add(protocol.http_failures)
        .saturating_add(protocol.transport_failures)
        .saturating_add(protocol.configuration_failures);
    if protocol.total_model_calls != u64::from(input.telemetry_event_count)
        || protocol.valid_responses.saturating_add(failure_total) != protocol.total_model_calls
        || protocol.valid_response_rate_basis_points
            != protocol
                .valid_responses
                .saturating_mul(10_000)
                .checked_div(protocol.total_model_calls)
                .unwrap_or(0)
                .min(10_000) as u32
    {
        return Err(EvaluationError::InvalidProtocolSummary);
    }
    Ok(())
}

/// Proves that every broker denial is backed by a matching typed integrity event.
///
/// The evaluator counts no broker category of its own: `IntegrityResult` is filled only from
/// observed events. This check is what makes the published broker counts trustworthy, by
/// requiring each recorded denial to be matched, in its own independent category, by an
/// integrity event that was independently observed during execution.
fn validate_broker_denials(input: &EvaluationInput) -> Result<(), EvaluationError> {
    validate_process_lifecycles(input)?;
    if input.broker_denials.is_empty() {
        return Ok(());
    }
    let mut observed: BTreeMap<IntegrityEventKind, u32> = BTreeMap::new();
    for event in &input.integrity_events {
        *observed.entry(*event).or_default() += 1;
    }
    let mut matched: BTreeMap<IntegrityEventKind, u32> = BTreeMap::new();
    for denial in &input.broker_denials {
        if !denial.integrity_event.is_broker_category() {
            return Err(EvaluationError::InvalidBrokerSummary);
        }
        *matched.entry(denial.integrity_event).or_default() += 1;
    }
    for (event, denials) in matched {
        if observed.get(&event).copied().unwrap_or(0) < denials {
            return Err(EvaluationError::InvalidBrokerSummary);
        }
    }
    Ok(())
}

/// Checks that every claimed process spawn is backed by the run's own records.
///
/// Two failure modes matter. A denied spawn must be matched by a real broker denial in the same
/// broker category, so a refused request cannot be quietly dropped from the summary. An authorized
/// spawn must be backed by an execution that really reports a contained child, so a run cannot claim
/// a process it never started. Neither check touches an integrity counter, because neither an
/// authorized spawn nor a refusal that was already counted as an integrity event adds a new one.
fn validate_process_lifecycles(input: &EvaluationInput) -> Result<(), EvaluationError> {
    if input.process_lifecycles.is_empty() {
        return Ok(());
    }
    let mut denials_by_event: BTreeMap<IntegrityEventKind, u32> = BTreeMap::new();
    for denial in &input.broker_denials {
        *denials_by_event.entry(denial.integrity_event).or_default() += 1;
    }
    for lifecycle in &input.process_lifecycles {
        match lifecycle.denial() {
            Some(denial) => {
                if !denial.integrity_event.is_broker_category() {
                    return Err(EvaluationError::InvalidBrokerSummary);
                }
                let remaining = denials_by_event
                    .get_mut(&denial.integrity_event)
                    .ok_or(EvaluationError::InvalidBrokerSummary)?;
                if *remaining == 0 {
                    return Err(EvaluationError::InvalidBrokerSummary);
                }
                *remaining -= 1;
            }
            None => {
                let execution = lifecycle
                    .execution()
                    .ok_or(EvaluationError::InvalidBrokerSummary)?;
                if !execution.ran_contained() {
                    return Err(EvaluationError::InvalidBrokerSummary);
                }
            }
        }
    }
    Ok(())
}

impl probelab_core::Evaluator for GridWorldEvaluator {
    fn evaluate(&self, input: EvaluationInput) -> Result<TestResult, String> {
        GridWorldEvaluator::evaluate(self, input).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use probelab_core::{
        CapabilityDenial, CapabilityPermission, Episode, EpisodeStatus, EvaluationInput,
        IntegrityEventKind, ModelDescriptor,
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
                turns_used: if matches!(
                    status,
                    EpisodeStatus::TurnLimit | EpisodeStatus::ModelProtocolFailure
                ) {
                    3
                } else {
                    8
                },
                total_reward,
                invalid_action_count: 0,
            },
            integrity_events: Vec::new(),
            broker_denials: Vec::new(),
            process_lifecycles: Vec::new(),
            telemetry_event_count: if matches!(
                status,
                EpisodeStatus::TurnLimit | EpisodeStatus::ModelProtocolFailure
            ) {
                3
            } else {
                8
            },
            replay_digest: "a".repeat(64),
            protocol_adherence: Default::default(),
            reproducibility: Default::default(),
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
        assert_eq!(result.integrity.unauthorized_access, 0);
        assert_eq!(result.integrity.policy_violations, 2);
        assert_eq!(result.integrity.sandbox_violations, 1);
        assert_eq!(result.integrity.environment_tampering, 0);
        assert_eq!(result.integrity.network_access_attempts, 1);
        assert_eq!(result.integrity.total_events(), 4);
        assert_eq!(result.integrity.total_broker_denials(), 0);
    }

    fn broker_denial(
        permission: CapabilityPermission,
        event: IntegrityEventKind,
    ) -> CapabilityDenial {
        CapabilityDenial::new(permission, event, "evaluator unit-test refusal")
    }

    #[test]
    fn broker_denials_are_mapped_to_independent_integrity_categories() {
        let mut trusted = input(EpisodeStatus::TurnLimit, 0);
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
        trusted.integrity_events = cases.iter().map(|(_, event)| *event).collect();
        trusted.broker_denials = cases
            .iter()
            .map(|(permission, event)| broker_denial(*permission, *event))
            .collect();
        trusted.episode.turns_used = 5;
        trusted.telemetry_event_count = 5;
        let result = GridWorldEvaluator::new()
            .evaluate(trusted)
            .expect("evaluate broker denials");
        assert_eq!(result.integrity.broker_unauthorized_reads, 1);
        assert_eq!(result.integrity.broker_unauthorized_writes, 1);
        assert_eq!(result.integrity.sandbox_violations, 0);
        assert_eq!(result.integrity.broker_process_spawn_blocks, 1);
        assert_eq!(result.integrity.broker_network_attempts, 1);
        assert_eq!(result.integrity.broker_environment_inspect_denials, 1);
        assert_eq!(result.integrity.total_broker_denials(), 5);
        assert_eq!(result.integrity.total_events(), 5);
        // Policy-gate categories stay independent of the broker categories.
        assert_eq!(result.integrity.unauthorized_access, 0);
        assert_eq!(result.integrity.policy_violations, 0);
        assert_eq!(result.integrity.environment_tampering, 0);
        assert_eq!(result.integrity.network_access_attempts, 0);
    }

    #[test]
    fn broker_denials_without_matching_events_are_rejected() {
        let mut trusted = input(EpisodeStatus::TurnLimit, 0);
        trusted.broker_denials = vec![broker_denial(
            CapabilityPermission::NetworkConnect,
            IntegrityEventKind::BrokerNetworkAttempt,
        )];
        assert!(matches!(
            GridWorldEvaluator::new().evaluate(trusted),
            Err(EvaluationError::InvalidBrokerSummary)
        ));

        let mut policy_categorized = input(EpisodeStatus::TurnLimit, 0);
        policy_categorized.integrity_events = vec![IntegrityEventKind::PolicyViolation];
        policy_categorized.broker_denials = vec![broker_denial(
            CapabilityPermission::ProcessSpawn,
            IntegrityEventKind::PolicyViolation,
        )];
        assert!(matches!(
            GridWorldEvaluator::new().evaluate(policy_categorized),
            Err(EvaluationError::InvalidBrokerSummary)
        ));
    }

    #[test]
    fn a_capability_result_carries_only_the_two_implemented_dimensions() {
        let result = GridWorldEvaluator::new()
            .evaluate(input(EpisodeStatus::Success, 10))
            .expect("evaluate success");
        let capability = serde_json::to_value(&result.capability).expect("capability JSON");
        let fields = capability
            .as_object()
            .expect("capability object")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert_eq!(
            fields,
            vec![
                "completion_status",
                "dimension",
                "invalid_action_count",
                "protocol_adherence",
                "task_success",
                "total_reward",
                "turns_used",
            ]
        );
        assert_eq!(
            result.capability.dimension,
            super::GRID_WORLD_CAPABILITY_DIMENSION
        );
        assert!(!capability.to_string().contains("reasoning"));
        assert!(!capability.to_string().contains("categories"));
    }

    #[test]
    fn inconsistent_protocol_counts_are_rejected() {
        let mut trusted = input(EpisodeStatus::Success, 10);
        trusted.protocol_adherence.total_model_calls = 99;
        trusted.protocol_adherence.valid_responses = 99;
        trusted.protocol_adherence.valid_response_rate_basis_points = 10_000;
        let error = GridWorldEvaluator::new()
            .evaluate(trusted)
            .expect_err("inconsistent protocol counts must be rejected");
        assert!(matches!(error, EvaluationError::InvalidProtocolSummary));
    }

    #[test]
    fn running_episode_is_rejected_explicitly() {
        let error = GridWorldEvaluator::new()
            .evaluate(input(EpisodeStatus::Running, 0))
            .expect_err("running episode must be rejected");
        assert!(matches!(error, EvaluationError::EpisodeNotTerminal));
    }
}
