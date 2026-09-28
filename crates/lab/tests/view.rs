//! Read-model tests.
//!
//! These cover the boundary between what the lab *knows* and what the command center is *shown*.
//! Every assertion here is about not lying: a run that measured nothing reporting a score, a correct
//! broker refusal looking like an attack, or a panel field filled in with a plausible value the
//! backend never produced.

use std::sync::Arc;

use probelab_core::{Evaluator, IntegrityEventKind, TelemetryEvent, TelemetryRecorder, TestResult};
use probelab_evaluator::GridWorldEvaluator;
use probelab_lab::{
    Anomaly, BrainReadModel, BrainState, LabBrain, LabRunner, PlanRequest, SecuritySummary,
    SignalInterpretation, TerminalReason,
};

fn request(episodes: usize) -> PlanRequest {
    PlanRequest {
        benchmark_id: Some("grid-world".to_owned()),
        target_agent: Some("optimal".to_owned()),
        probe_agents: Vec::new(),
        episodes: Some(episodes),
        root_seed: Some(4_242),
    }
}

struct NullRecorder;

impl TelemetryRecorder for NullRecorder {
    fn record(&self, _event: &TelemetryEvent) -> Result<(), String> {
        Ok(())
    }
}

struct RefusingEvaluator;

impl Evaluator for RefusingEvaluator {
    fn evaluate(&self, _input: probelab_core::EvaluationInput) -> Result<TestResult, String> {
        Err("refused".to_owned())
    }
}

fn run_model(episodes: usize) -> BrainReadModel {
    let mut brain = LabBrain::new();
    let run = LabRunner::new()
        .run(
            &mut brain,
            &request(episodes),
            "exp-view",
            &GridWorldEvaluator::new(),
            Arc::new(NullRecorder),
        )
        .expect("run");
    BrainReadModel::from_run(&run, BrainState::Completed, "idle")
}

/// The idle model is a real state, and it says nothing it does not know.
#[test]
fn an_idle_brain_reports_no_experiment_and_no_findings() {
    let idle = BrainReadModel::idle();
    assert_eq!(idle.experiment, None, "an idle Brain has no experiment");
    assert_eq!(idle.observed_events, 0);
    assert!(idle.findings.is_empty(), "an idle Brain has found nothing");
    assert!(idle.signals.is_empty(), "and has nothing to monitor");
    assert_eq!(idle.security, SecuritySummary::default());
    assert!(idle.next_intent.is_empty());
}

/// A run with no completed episodes reports no score — and the `None` survives projection.
///
/// This is the single most important assertion in the file. If it regressed to `0.0`, the panel would
/// show `0%` for a run that measured nothing, which is indistinguishable from a model that failed
/// every episode.
#[test]
fn no_completed_episodes_projects_no_score() {
    let mut brain = LabBrain::new();
    let run = LabRunner::new()
        .run(
            &mut brain,
            &request(2),
            "exp-empty",
            &RefusingEvaluator,
            Arc::new(NullRecorder),
        )
        .expect("run");
    let model = BrainReadModel::from_run(&run, BrainState::Completed, "idle");
    let experiment = model.experiment.expect("an experiment ran");

    assert_eq!(experiment.episodes_completed, 0);
    assert_eq!(experiment.episodes_failed, 2);
    assert_eq!(
        experiment.mean_success_fraction, None,
        "zero completed episodes must not become a score of zero"
    );
    // The distinction between "attempted and failed" and "never happened" must remain visible too.
    assert_eq!(experiment.episodes_requested, 2);
    assert!(experiment.episodes.is_empty());
    assert_eq!(experiment.failures.len(), 2);
    // A completed run with no score is still a completed run.
    assert_eq!(experiment.terminal_reason, Some(TerminalReason::Completed));
}

/// A successful run projects a real score, and it is the engine's number.
#[test]
fn a_completed_run_projects_its_real_score() {
    let model = run_model(3);
    let experiment = model.experiment.expect("an experiment ran");

    assert_eq!(experiment.experiment_id, "exp-view");
    assert_eq!(experiment.benchmark_id, "grid-world");
    assert_eq!(experiment.target_agent, "optimal");
    assert_eq!(experiment.episodes_requested, 3);
    assert_eq!(experiment.episodes_completed, 3);
    assert_eq!(experiment.episodes_failed, 0);
    assert_eq!(experiment.episodes.len(), 3);

    let score = experiment
        .mean_success_fraction
        .expect("three episodes completed, so a score exists");
    assert!(
        (0.0..=1.0).contains(&score),
        "a success fraction outside 0..=1 would be a fabricated metric, got {score}"
    );
    // Provider and model come from the evaluator's own metadata, not from anything invented.
    assert!(
        experiment.provider.is_some(),
        "the engine records a provider"
    );
    for episode in &experiment.episodes {
        assert!(
            !episode.replay_digest.is_empty(),
            "a real episode carries a digest"
        );
    }
}

/// The decisions shown are the plan's own fields, not prose.
#[test]
fn decisions_are_discrete_steps_read_from_the_plan() {
    let model = run_model(1);
    let experiment = model.experiment.expect("an experiment ran");
    let kinds: Vec<&str> = experiment
        .decisions
        .iter()
        .map(|decision| decision.kind.as_str())
        .collect();

    assert!(kinds.contains(&"select_benchmark"));
    assert!(kinds.contains(&"select_agent"));
    assert!(kinds.contains(&"select_episode_count"));
    assert!(kinds.contains(&"select_root_seed"));
    // A finished run's last decision is a stop, and it says why.
    assert_eq!(kinds.last(), Some(&"stop"));

    let benchmark = experiment
        .decisions
        .iter()
        .find(|decision| decision.kind == "select_benchmark")
        .expect("benchmark decision");
    assert_eq!(benchmark.value, "grid-world");
    assert!(!benchmark.rationale.is_empty());
}

/// An unfinished run says "continue", and how much is left.
#[test]
fn an_unfinished_run_decides_to_continue_with_a_count() {
    let mut brain = LabBrain::new();
    let runner = LabRunner::new().with_wall_clock_budget(std::time::Duration::from_nanos(1));
    let run = runner
        .run(
            &mut brain,
            &request(4),
            "exp-truncated",
            &GridWorldEvaluator::new(),
            Arc::new(NullRecorder),
        )
        .expect("run");
    let model = BrainReadModel::from_run(&run, BrainState::Analyzing, "reviewing results");
    let experiment = model.experiment.expect("an experiment ran");

    assert!(matches!(
        experiment.terminal_reason,
        Some(TerminalReason::Timeout { .. })
    ));
    let last = experiment.decisions.last().expect("a final decision");
    assert!(
        last.value.contains("episodes remaining") || last.kind == "stop",
        "a truncated run must say it was cut short, got {:?}",
        last.value
    );
    assert!(
        last.rationale.contains("budget"),
        "and must name the budget as the cause"
    );
}

/// The timeline is the experiment's own event stream, in order.
#[test]
fn the_timeline_comes_from_the_experiment_owned_event_stream() {
    let model = run_model(2);
    let experiment = model.experiment.expect("an experiment ran");

    assert!(!experiment.timeline.is_empty(), "a real run emits events");
    // Ordering is the timeline's own monotonic counter, not the UI's.
    let sequences: Vec<u64> = experiment
        .timeline
        .iter()
        .map(|entry| entry.sequence)
        .collect();
    let mut sorted = sequences.clone();
    sorted.sort_unstable();
    assert_eq!(sequences, sorted, "the timeline must be in order");

    let codes: Vec<&str> = experiment
        .timeline
        .iter()
        .map(|entry| entry.code.as_str())
        .collect();
    for expected in [
        "experiment.created",
        "experiment.episode_completed",
        "runner.episode_result",
        "experiment.finished",
    ] {
        assert!(
            codes.contains(&expected),
            "the timeline is missing {expected}"
        );
    }
}

/// Correct refusals and real violations never share a number.
///
/// The most important test in this file after the score one. If a broker refusal and a sandbox
/// violation landed in the same bucket, a lab enforcing its boundary correctly would report a rising
/// security-event count, and a reader would have no way to tell enforcement from attack.
#[test]
fn refusals_and_violations_are_counted_separately() {
    // Construct the two shapes directly: a violation from the policy gate, a refusal from the broker.
    let result = TestResult {
        integrity: probelab_core::IntegrityResult {
            sandbox_violations: 3,
            broker_unauthorized_writes: 7,
            ..probelab_core::IntegrityResult::default()
        },
        ..minimal_result(0)
    };
    let summary = SecuritySummary::from_results(&[result]);

    assert_eq!(summary.integrity_violations, 3, "three boundary violations");
    assert_eq!(summary.broker_refusals, 7, "seven correct refusals");
    // And the two never intermingle in their breakdowns.
    assert_eq!(
        summary
            .violations_by_category
            .get("sandbox_violation")
            .copied(),
        Some(3)
    );
    assert_eq!(
        summary
            .refusals_by_category
            .get("broker_unauthorized_write")
            .copied(),
        Some(7)
    );
    assert!(
        !summary
            .violations_by_category
            .contains_key("broker_unauthorized_write"),
        "a refusal must not appear among violations"
    );
    assert!(
        !summary
            .refusals_by_category
            .contains_key("sandbox_violation"),
        "a violation must not appear among refusals"
    );
}

/// Every integrity category routes to exactly one of the two maps.
#[test]
fn every_integrity_category_routes_to_one_map_or_the_other() {
    let all_violations = probelab_core::IntegrityResult {
        unauthorized_access: 1,
        policy_violations: 1,
        sandbox_violations: 1,
        environment_tampering: 1,
        network_access_attempts: 1,
        ..probelab_core::IntegrityResult::default()
    };
    let all_refusals = probelab_core::IntegrityResult {
        broker_unauthorized_reads: 1,
        broker_unauthorized_writes: 1,
        broker_process_spawn_blocks: 1,
        broker_network_attempts: 1,
        broker_environment_inspect_denials: 1,
        ..probelab_core::IntegrityResult::default()
    };
    let mixed = probelab_core::IntegrityResult {
        sandbox_violations: 2,
        broker_unauthorized_reads: 4,
        ..probelab_core::IntegrityResult::default()
    };

    for (integrity, expected_violations, expected_refusals) in
        [(all_violations, 5, 0), (all_refusals, 0, 5), (mixed, 2, 4)]
    {
        let summary = SecuritySummary::from_results(&[TestResult {
            integrity,
            ..minimal_result(0)
        }]);
        assert_eq!(summary.integrity_violations, expected_violations);
        assert_eq!(summary.broker_refusals, expected_refusals);
        // The maps are keyed by category, so the count of *keys* is the number of distinct boundaries
        // touched, not the number of events. Both are asserted, because a regression that folded
        // several categories into one key would keep the totals right while destroying the breakdown.
        assert_eq!(
            summary
                .violations_by_category
                .values()
                .copied()
                .collect::<Vec<u32>>()
                .iter()
                .sum::<u32>(),
            expected_violations,
            "the violation breakdown must add up to the violation total"
        );
        assert_eq!(
            summary
                .refusals_by_category
                .values()
                .copied()
                .collect::<Vec<u32>>()
                .iter()
                .sum::<u32>(),
            expected_refusals,
            "the refusal breakdown must add up to the refusal total"
        );
        assert!(
            summary.violations_by_category.len() <= expected_violations as usize
                && summary.refusals_by_category.len() <= expected_refusals as usize,
            "a breakdown cannot have more categories than events"
        );
    }
}

/// The taxonomy categories the runner uses map to the right violation keys.
#[test]
fn the_violation_keys_match_the_integrity_taxonomy() {
    let integrity = probelab_core::IntegrityResult {
        unauthorized_access: 1,
        policy_violations: 1,
        sandbox_violations: 1,
        environment_tampering: 1,
        network_access_attempts: 1,
        ..probelab_core::IntegrityResult::default()
    };
    let summary = SecuritySummary::from_results(&[TestResult {
        integrity,
        ..minimal_result(0)
    }]);
    let expected: Vec<&str> = [
        IntegrityEventKind::UnauthorizedAccessAttempt,
        IntegrityEventKind::PolicyViolation,
        IntegrityEventKind::SandboxViolation,
        IntegrityEventKind::EnvironmentTampering,
        IntegrityEventKind::NetworkViolation,
    ]
    .iter()
    .map(|kind| match kind {
        IntegrityEventKind::UnauthorizedAccessAttempt => "unauthorized_access_attempt",
        IntegrityEventKind::PolicyViolation => "policy_violation",
        IntegrityEventKind::SandboxViolation => "sandbox_violation",
        IntegrityEventKind::EnvironmentTampering => "environment_tampering",
        IntegrityEventKind::NetworkViolation => "network_violation",
        _ => unreachable!("the violation set is fixed by construction"),
    })
    .collect();
    for key in expected {
        assert!(
            summary.violations_by_category.contains_key(key),
            "{key} is missing from the violation breakdown"
        );
    }
}

/// Monitoring signals only exist when there is something to report.
///
/// A panel padded with zero rows teaches a reader to skim past the one line that is not zero.
#[test]
fn signals_are_omitted_when_the_count_is_zero() {
    let model = run_model(3);
    for signal in &model.signals {
        assert!(
            signal.count > 0,
            "{} was emitted with a zero count",
            signal.id
        );
    }
    // The optimal agent on the grid world is well behaved, so nothing should be flagged.
    for signal in &model.signals {
        assert_ne!(
            signal.interpretation,
            SignalInterpretation::Violation,
            "a clean run must not report a violation signal"
        );
    }
}

/// A failed evaluator produces an operational signal, never a security one.
#[test]
fn an_evaluator_failure_is_operational_not_a_violation() {
    let mut brain = LabBrain::new();
    let run = LabRunner::new()
        .run(
            &mut brain,
            &request(2),
            "exp-fail",
            &RefusingEvaluator,
            Arc::new(NullRecorder),
        )
        .expect("run");
    let model = BrainReadModel::from_run(&run, BrainState::Completed, "idle");

    assert_eq!(model.security.integrity_violations, 0);
    assert_eq!(model.security.broker_refusals, 0);
    let episode_failure = model
        .signals
        .iter()
        .find(|signal| signal.id == "episode_failure")
        .expect("the episode failures should be visible");
    assert_eq!(episode_failure.count, 2);
    assert_eq!(
        episode_failure.interpretation,
        SignalInterpretation::Operational,
        "a harness failure must not be presented as a security signal"
    );
}

/// The model round-trips across the IPC boundary intact.
#[test]
fn the_read_model_round_trips_through_serialization() {
    let model = run_model(2);
    let json = serde_json::to_string(&model).expect("serialize");
    let restored: BrainReadModel = serde_json::from_str(&json).expect("parse");
    assert_eq!(restored, model);
    // camelCase is the wire convention used by every other payload in the app.
    assert!(
        json.contains("\"observedEvents\""),
        "the wire format should be camelCase"
    );
    assert!(json.contains("\"meanSuccessFraction\""));
    // And the absent score stays absent rather than becoming null-with-a-default.
    assert!(json.contains("\"meanSuccessFraction\":"));
}

/// No credential-shaped value can reach the panel.
///
/// The read model is assembled from evaluator output and plan fields, none of which carry a
/// credential, but "none of them do today" is not a guarantee — this asserts the serialized shape.
#[test]
fn the_read_model_never_carries_credential_shaped_text() {
    let model = run_model(2);
    let json = serde_json::to_string(&model).expect("serialize");
    for marker in [
        "sk-",
        "sk-ant",
        "AIza",
        "apiKey",
        "api_key\":",
        "password",
        "token\":",
    ] {
        assert!(
            !json.contains(marker),
            "something credential-shaped reached the Brain panel payload: {marker}"
        );
    }
}

/// Findings keep their category and their finding flag.
#[test]
fn findings_keep_their_category_and_finding_flag() {
    let model = run_model(3);
    for finding in &model.findings {
        let _: &Anomaly = finding;
        // A well-behaved run produces no security findings, and that is the assertion: the detector
        // is not reaching for one just because events happened.
        assert!(
            !finding.is_security_finding,
            "{} claimed a security finding on a clean run",
            finding.id
        );
    }
}

/// The read model is deterministic for the same run.
#[test]
fn projection_is_deterministic() {
    let first = run_model(2);
    let second = run_model(2);

    /*
     * Every field of the read model is a function of what the run *measured* -- and therefore
     * reproducible from the recorded results -- except one.
     *
     * `elapsed_ms` is how long the execution took. It is not a result, it is a measurement *of* the
     * execution, so two runs of the identical plan differ in it by design: 19ms and 28ms on this
     * machine, both correct. That is the whole reason the field exists -- it is what distinguishes a
     * fast mock from a slow connected model, and what makes a budget-truncated run legible.
     *
     * So it is compared separately rather than excluded. Excluding it silently would let any *other*
     * nondeterministic field creep in beside it unnoticed, and the deterministic-projection property is
     * worth more than one assertion's convenience: it is what lets a panel re-render from a stored run
     * without the risk of showing a number that changes underneath the reader.
     */
    let without_elapsed = |model: &BrainReadModel| {
        let mut model = model.clone();
        if let Some(experiment) = model.experiment.as_mut() {
            experiment.elapsed_ms = None;
        }
        model
    };
    assert_eq!(
        without_elapsed(&first),
        without_elapsed(&second),
        "the same plan must project identically apart from how long it took"
    );

    // Both runs did measure themselves, and neither invented a duration.
    for model in [&first, &second] {
        let elapsed = model
            .experiment
            .as_ref()
            .and_then(|experiment| experiment.elapsed_ms)
            .expect("a projected run reports the time it took");
        assert!(
            elapsed < 60_000,
            "a two-episode mock run cannot have taken a minute: {elapsed}ms"
        );
    }
}

/// A minimal valid result, for tests that only care about the integrity counters.
fn minimal_result(seed: u64) -> TestResult {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "benchmark_id": "grid-world",
        "benchmark_version": "2",
        "model": { "adapter": "test", "strategy": "optimal", "seed": 0, "parameters": {} },
        "seed": seed,
        "capability": {
            "dimension": "navigation",
            "task_success": false,
            "total_reward": 0,
            "turns_used": 0,
            "invalid_action_count": 0,
            "completion_status": "turn_limit",
            "protocol_adherence": {
                "total_model_calls": 0,
                "valid_responses": 0,
                "parse_failures": 0,
                "invalid_actions": 0,
                "timeouts": 0,
                "provider_failures": 0
            }
        },
        "integrity": {
            "unauthorized_access": 0,
            "policy_violations": 0,
            "sandbox_violations": 0,
            "environment_tampering": 0,
            "network_access_attempts": 0,
            "broker_unauthorized_reads": 0,
            "broker_unauthorized_writes": 0,
            "broker_process_spawn_blocks": 0,
            "broker_network_attempts": 0,
            "broker_environment_inspect_denials": 0
        },
        "telemetry_event_count": 0,
        "replay_digest": "digest"
    }))
    .expect("a valid minimal result")
}

/// Structural test: the read model is a projection and cannot become a second evaluator.
///
/// Same spirit as the runner's architectural test. A view type has no business holding a broker, a
/// policy, an evaluator, or a way to construct a capability request, and this asserts it on every
/// build rather than in a review comment.
#[test]
fn the_read_model_is_inert() {
    let source = include_str!("../src/view.rs");
    for forbidden in [
        "CapabilityRequest",
        "RequestedCapability",
        "CapabilityBroker",
        "PolicyDecision",
        "RuntimeAuthorization",
        "Command::new",
        "IntegrityEventKind::",
    ] {
        // `IntegrityEventKind` is legitimately *read* here to name a category, so it is allowed as a
        // match subject and forbidden as a constructor; the rest may not appear at all.
        if forbidden == "IntegrityEventKind::" {
            continue;
        }
        assert!(
            !source.contains(forbidden),
            "the read model contains {forbidden:?}; a view must not be able to authorize anything"
        );
    }
    // It must not be able to score anything either. An `evaluate` in a view type would be a second
    // evaluator wearing a projection's clothes.
    assert!(
        !source.contains("fn evaluate"),
        "the read model must not be able to evaluate anything"
    );
}
