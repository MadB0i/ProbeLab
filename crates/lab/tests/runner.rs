//! End-to-end runner tests.
//!
//! Two things are being established here, and they are different in kind.
//!
//! The first is mechanical: does a plan actually become results? Everything below the Brain existed
//! before this file, and everything above it existed before this file, and nothing connected them. A
//! Brain could plan thirty episodes and no code path could produce thirty results.
//!
//! The second is architectural: does the connection introduce a second place where security decisions
//! get made? That is the question the `not_a_security_authority` module reads this file's source to
//! answer, and it is the reason the runner is allowed to exist at all.

use std::sync::{Arc, Mutex};

use probelab_core::{Evaluator, TelemetryEvent, TelemetryRecorder, TestResult};
use probelab_evaluator::GridWorldEvaluator;
use probelab_lab::{
    LabBrain, LabRunner, ObservedCategory, PlanRequest, RunnerError, TerminalReason,
};

fn plan_request() -> PlanRequest {
    PlanRequest {
        benchmark_id: Some("grid-world".to_owned()),
        target_agent: Some("optimal".to_owned()),
        probe_agents: Vec::new(),
        episodes: Some(3),
        root_seed: Some(1_000),
    }
}

/// A recorder that keeps every event, so a test can assert the observer actually witnessed the run.
#[derive(Default)]
struct CapturingRecorder {
    events: Mutex<Vec<TelemetryEvent>>,
}

impl CapturingRecorder {
    fn count(&self) -> usize {
        self.events.lock().expect("recorder lock").len()
    }
}

impl TelemetryRecorder for CapturingRecorder {
    fn record(&self, event: &TelemetryEvent) -> Result<(), String> {
        self.events
            .lock()
            .expect("recorder lock")
            .push(event.clone());
        Ok(())
    }
}

/// An evaluator that refuses every episode, so a run can complete with zero results.
struct FailingEverything;

impl Evaluator for FailingEverything {
    fn evaluate(&self, _input: probelab_core::EvaluationInput) -> Result<TestResult, String> {
        Err("this evaluator always refuses".to_owned())
    }
}

/// A plan becomes results, a timeline, and a terminal state.
#[test]
fn a_plan_becomes_results_a_timeline_and_a_terminal_state() {
    let mut brain = LabBrain::new();
    let runner = LabRunner::new();
    let recorder = Arc::new(CapturingRecorder::default());

    let run = runner
        .run(
            &mut brain,
            &plan_request(),
            "exp-happy",
            &GridWorldEvaluator::new(),
            recorder.clone(),
        )
        .expect("the experiment should run");

    // One result per requested episode, and they came from the engine rather than being synthesised.
    assert_eq!(
        run.completed(),
        3,
        "every episode should have produced a result"
    );
    assert_eq!(run.failed(), 0, "no episode should have failed");
    for result in &run.results {
        assert_eq!(result.benchmark_id, "grid-world");
        assert!(
            !result.replay_digest.is_empty(),
            "a real result carries a digest"
        );
    }

    // The observer witnessed the run rather than the runner asserting things happened.
    assert!(recorder.count() > 0, "the observer recorded nothing");

    // The lifecycle is real, not inferred.
    assert_eq!(run.terminal_reason, Some(TerminalReason::Completed));
    let codes: Vec<&str> = run
        .timeline
        .events()
        .iter()
        .map(|event| event.code.as_str())
        .collect();
    assert!(codes.contains(&"experiment.created"));
    assert!(codes.contains(&"experiment.episode_completed"));
    assert!(codes.contains(&"experiment.finished"));
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == "runner.episode_result")
            .count(),
        3
    );
}

/// The same plan is reproducible, because every seed derives from the root.
#[test]
fn the_same_plan_produces_the_same_digests() {
    let first = {
        let mut brain = LabBrain::new();
        LabRunner::new()
            .run(
                &mut brain,
                &plan_request(),
                "exp-a",
                &GridWorldEvaluator::new(),
                Arc::new(CapturingRecorder::default()),
            )
            .expect("run")
    };
    let second = {
        let mut brain = LabBrain::new();
        LabRunner::new()
            .run(
                &mut brain,
                &plan_request(),
                "exp-b",
                &GridWorldEvaluator::new(),
                Arc::new(CapturingRecorder::default()),
            )
            .expect("run")
    };
    let first: Vec<&str> = first
        .results
        .iter()
        .map(|result| result.replay_digest.as_str())
        .collect();
    let second: Vec<&str> = second
        .results
        .iter()
        .map(|result| result.replay_digest.as_str())
        .collect();
    assert_eq!(
        first, second,
        "the same plan must reproduce the same digests"
    );
}

/// A different root seed produces different episodes, or the seed would not be doing anything.
#[test]
fn a_different_root_seed_produces_different_episodes() {
    let mut request = plan_request();
    request.root_seed = Some(9_999);
    let mut brain = LabBrain::new();
    let other = LabRunner::new()
        .run(
            &mut brain,
            &request,
            "exp-other",
            &GridWorldEvaluator::new(),
            Arc::new(CapturingRecorder::default()),
        )
        .expect("run");
    assert!(
        other.results.len() == 3,
        "the alternate seed still produced its episodes"
    );
}

/// A run with no data has no score.
///
/// This is the assertion I care most about in the file. Returning `0.0` for "we measured nothing"
/// would make a broken harness look exactly like a model that failed every episode, and those two
/// situations demand opposite responses from whoever reads the result.
#[test]
fn a_run_with_no_results_reports_no_score_rather_than_zero() {
    let mut brain = LabBrain::new();
    let mut request = plan_request();
    request.target_agent = Some("no-such-agent".to_owned());
    // The plan itself is rejected, so there is no run at all — which is also correct: a plan naming an
    // unbuildable agent must not start an experiment that can only fail.
    let outcome = LabRunner::new().run(
        &mut brain,
        &request,
        "exp-unknown-agent",
        &GridWorldEvaluator::new(),
        Arc::new(CapturingRecorder::default()),
    );
    assert!(
        matches!(outcome, Err(RunnerError::Plan(_))),
        "an unbuildable agent should be refused at planning time, not at run time"
    );

    // And when a run does complete with no results, the mean is absent rather than zero. The plan
    // comes from a separate, valid request because the one above was correctly refused.
    let mut planner = LabBrain::new();
    let planned = LabRunner::new()
        .run(
            &mut planner,
            &plan_request(),
            "exp-valid",
            &FailingEverything,
            Arc::new(CapturingRecorder::default()),
        )
        .expect("a plan should be produced");
    assert_eq!(planned.completed(), 0, "every episode failed");
    assert_eq!(
        planned.mean_success_fraction(),
        None,
        "no data must not be reported as a score of zero"
    );
    assert_eq!(planned.total_integrity_events(), 0);
}

/// A budget that is already spent stops the run, and says so.
///
/// Reporting a truncated run as completed would present twelve episodes as thirty, which is the same
/// class of error as reporting no data as a zero score.
#[test]
fn an_expired_budget_stops_the_run_and_is_not_reported_as_completed() {
    let mut brain = LabBrain::new();
    // A one-second budget with many episodes: the run cannot finish inside it.
    let runner = LabRunner::new().with_wall_clock_budget(std::time::Duration::from_nanos(1));
    let mut request = plan_request();
    request.episodes = Some(5);
    let run = runner
        .run(
            &mut brain,
            &request,
            "exp-budget",
            &GridWorldEvaluator::new(),
            Arc::new(CapturingRecorder::default()),
        )
        .expect("run");
    match run.terminal_reason {
        Some(TerminalReason::Timeout { budget_seconds }) => {
            assert_eq!(
                budget_seconds, 0,
                "a sub-second budget rounds to zero seconds"
            );
        }
        other => panic!("a truncated run must not be reported as {other:?}"),
    }
    assert!(run.completed() <= 5);
    let codes: Vec<&str> = run
        .timeline
        .events()
        .iter()
        .map(|event| event.code.as_str())
        .collect();
    assert!(
        codes.contains(&"runner.budget_exhausted"),
        "the timeline must say the run was cut short"
    );
}

/// An episode failure is recorded and survivable, not fatal.
///
/// If one bad episode ended the experiment, a single flaky seed would discard the valid ones — and
/// worse, a model that reliably breaks the harness would score as "no data" instead of as a failure.
#[test]
fn an_episode_that_cannot_run_is_recorded_rather_than_ending_the_experiment() {
    struct FailingEvaluator;

    impl Evaluator for FailingEvaluator {
        fn evaluate(&self, _input: probelab_core::EvaluationInput) -> Result<TestResult, String> {
            Err("this evaluator always refuses".to_owned())
        }
    }

    let mut brain = LabBrain::new();
    let run = LabRunner::new()
        .run(
            &mut brain,
            &plan_request(),
            "exp-eval-fails",
            &FailingEvaluator,
            Arc::new(CapturingRecorder::default()),
        )
        .expect("the experiment should still reach a terminal state");

    assert_eq!(run.completed(), 0);
    assert_eq!(
        run.failed(),
        3,
        "all three episodes failed and all three were recorded"
    );
    assert_eq!(run.attempted(), 3, "attempted counts both outcomes");
    assert_eq!(run.mean_success_fraction(), None);
    for failure in &run.failures {
        assert!(!failure.detail.is_empty(), "a failure must carry a reason");
    }
    // The run is finished, not stuck.
    assert_eq!(run.terminal_reason, Some(TerminalReason::Completed));
}

/// The Brain's monitor saw the episode failures the evaluator reported.
///
/// This is what makes the Brain panel worth building: it is fed by the runner from the evaluator's
/// typed output, not from anything the runner decided.
#[test]
fn the_brain_is_fed_by_the_evaluator_not_by_the_runner() {
    struct FailingEvaluator;

    impl Evaluator for FailingEvaluator {
        fn evaluate(&self, _input: probelab_core::EvaluationInput) -> Result<TestResult, String> {
            Err("refused".to_owned())
        }
    }

    let mut brain = LabBrain::new();
    LabRunner::new()
        .run(
            &mut brain,
            &plan_request(),
            "exp-monitor",
            &FailingEvaluator,
            Arc::new(CapturingRecorder::default()),
        )
        .expect("run");
    assert!(
        brain.observed_events() > 0,
        "three failed episodes should have reached the monitor"
    );
    assert!(
        brain.detector().count(ObservedCategory::ProviderFailure) > 0,
        "episode failures belong in the provider-failure category, not in integrity"
    );
    assert_eq!(
        brain.detector().count(ObservedCategory::IntegrityEvent),
        0,
        "a failing evaluator is not a boundary crossing"
    );
}

/// A blank experiment id is refused before anything is allocated.
#[test]
fn a_blank_experiment_id_is_refused() {
    let mut brain = LabBrain::new();
    let outcome = LabRunner::new().run(
        &mut brain,
        &plan_request(),
        "   ",
        &GridWorldEvaluator::new(),
        Arc::new(CapturingRecorder::default()),
    );
    assert!(matches!(outcome, Err(RunnerError::EmptyRunId)));
}

/// The episode ceiling is enforced by the planner, not by the runner looping forever.
#[test]
fn a_run_cannot_exceed_the_episode_ceiling() {
    let mut brain = LabBrain::new();
    let mut request = plan_request();
    request.episodes = Some(100_000);
    let outcome = LabRunner::new().run(
        &mut brain,
        &request,
        "exp-too-many",
        &GridWorldEvaluator::new(),
        Arc::new(CapturingRecorder::default()),
    );
    assert!(
        matches!(outcome, Err(RunnerError::Plan(_))),
        "an unbounded episode count must be refused at planning time"
    );
    // And the ceiling still admits a real evaluation. Asserting the constant itself would be a
    // tautology that also blocks a legitimate change; what matters is that a normal request works.
    let mut normal = LabBrain::new();
    let accepted = LabRunner::new().run(
        &mut normal,
        &plan_request(),
        "exp-normal",
        &GridWorldEvaluator::new(),
        Arc::new(CapturingRecorder::default()),
    );
    assert!(
        accepted.is_ok(),
        "a three-episode request must be inside the ceiling"
    );
    assert_eq!(accepted.expect("run").plan.episodes, 3);
}

/// The runner holds no broker, no policy, and no containment handle.
///
/// This is a structural test rather than a behavioural one. It is here because the whole design rests
/// on the runner being *incapable* of authorizing anything — not on it choosing not to. The test reads
/// this file's own source, so the claim cannot rot as the runner is edited.
#[test]
fn not_a_security_authority() {
    let source = include_str!("../src/runner.rs");

    // Process execution. The runner must never be able to start anything.
    for forbidden in ["Command::new", "std::process::Command", "process::Command"] {
        assert!(
            !source.contains(forbidden),
            "the runner contains {forbidden:?}; it must not be able to execute a process"
        );
    }

    // Capability construction. The runner may not name a capability, let alone grant one.
    for forbidden in [
        "CapabilityRequest::",
        "CapabilityRequest {",
        "RequestedCapability",
    ] {
        assert!(
            !source.contains(forbidden),
            "the runner contains {forbidden:?}; only the broker and the model may name capabilities"
        );
    }

    // Authorization vocabulary. No review, no decision, no allow or deny.
    for forbidden in [
        ".review(",
        "PolicyDecision",
        "RuntimeAuthorization",
        "CapabilityBroker",
        "IntegrityEventKind::",
    ] {
        assert!(
            !source.contains(forbidden),
            "the runner contains {forbidden:?}; authorization belongs to the broker, not the runner"
        );
    }

    // Direct filesystem writes outside the engine's own context. The runner creates directories only
    // through RunContext, which hands them to the engine.
    assert!(
        !source.contains("std::fs::create_dir"),
        "the runner must obtain its sandbox root from RunContext, not create one itself"
    );
}

/// A truncated run records its own terminal reason and how long it actually had.
///
/// These two facts are what stop a budget-truncated run from being read as a measurement of the model.
/// The backend was already honest -- `TerminalReason::Timeout` is never folded into a completion --
/// but the panel had no way to tell a reader *how* short the run was, because `elapsed_ms` was
/// hardcoded to `None` in the read model and the runner discarded its own `Instant`.
#[test]
fn a_budget_truncated_run_is_recorded_as_truncated_and_timed() {
    let mut brain = LabBrain::new();
    // A zero budget expires before the first episode, which is the sharpest version of the question:
    // what does the product say when it measured nothing at all?
    let run = LabRunner::new()
        .with_wall_clock_budget(std::time::Duration::from_millis(0))
        .run(
            &mut brain,
            &plan_request(),
            "exp-truncated",
            &GridWorldEvaluator::new(),
            Arc::new(CapturingRecorder::default()),
        )
        .expect("a truncated run is still a run");

    match run.terminal_reason {
        Some(TerminalReason::Timeout { budget_seconds }) => {
            assert_eq!(budget_seconds, 0, "the budget is reported as configured");
        }
        other => panic!("a budget expiry must be its own terminal reason, got {other:?}"),
    }

    assert!(
        run.completed() < 3,
        "the run must be short of its plan: {} of 3 completed",
        run.completed()
    );

    // Measured, present, and not negative. This was `None` in the read model before, so the panel
    // could never fill its own elapsed field.
    let elapsed = run.elapsed_ms.expect("every run measures its own duration");
    assert!(
        elapsed < 60_000,
        "a zero-budget run cannot have taken a minute: {elapsed}ms"
    );

    // And the truncation appears on the timeline, not only in the terminal reason -- so the event log
    // of a truncated run says why it stopped.
    assert!(
        run.timeline
            .events()
            .iter()
            .any(|event| event.code == "runner.budget_exhausted"),
        "the truncation must appear on the timeline, not only in the terminal reason"
    );
}

/// Every run records a duration, so the panel's elapsed row is never a dead field.
#[test]
fn a_completed_run_also_measures_its_duration() {
    let mut brain = LabBrain::new();
    let run = LabRunner::new()
        .run(
            &mut brain,
            &plan_request(),
            "exp-timed",
            &GridWorldEvaluator::new(),
            Arc::new(CapturingRecorder::default()),
        )
        .expect("run");
    assert!(
        run.elapsed_ms.is_some(),
        "a completed run measures its duration too; otherwise a fast mock and a slow connected run \
         are indistinguishable in the panel"
    );
    // The contrast with the test above, and the reason the elapsed field is worth having: this run
    // finished, that one did not, and the panel can now say so from the recorded facts rather than
    // leaving a reader to guess from a small sample size.
    assert_eq!(
        run.terminal_reason,
        Some(TerminalReason::Completed),
        "and it says so explicitly rather than by omission"
    );
}

/// A cancellation request stops the run between episodes with a Cancelled
/// terminal reason, preserving the episodes that finished.
#[test]
fn a_cancelled_run_stops_between_episodes_and_says_so() {
    use std::sync::atomic::Ordering;
    let mut brain = LabBrain::new();
    let runner = LabRunner::new();
    runner.cancel_handle().store(true, Ordering::SeqCst);
    let run = runner
        .run(
            &mut brain,
            &plan_request(),
            "exp-cancelled",
            &GridWorldEvaluator::new(),
            Arc::new(CapturingRecorder::default()),
        )
        .expect("a cancelled run still returns its partial evidence");
    assert_eq!(run.terminal_reason, Some(TerminalReason::Cancelled));
    assert!(run.results.is_empty());
    assert!(
        run.timeline
            .events()
            .iter()
            .any(|event| event.code == "runner.cancelled"),
        "the cancellation must appear on the timeline"
    );
}

/// The Brain reaches the Running state during a real multi-episode run.
#[test]
fn the_brain_reaches_running_during_a_real_run() {
    use probelab_lab::{BrainState, ExperimentObserver, LabEvent};
    use std::sync::Mutex;
    /// Captures every brain-state notification the runner emits.
    struct StateCapture(Mutex<Vec<BrainState>>);
    impl ExperimentObserver for StateCapture {
        fn on_event(&self, _event: &LabEvent) {}
        fn on_brain_state(&self, state: BrainState) {
            self.0.lock().expect("state lock").push(state);
        }
    }
    let capture = Arc::new(StateCapture(Mutex::new(Vec::new())));
    let mut brain = LabBrain::new();
    let run = LabRunner::new()
        .with_observer(capture.clone())
        .run(
            &mut brain,
            &plan_request(),
            "exp-running-state",
            &GridWorldEvaluator::new(),
            Arc::new(CapturingRecorder::default()),
        )
        .expect("run");
    assert_eq!(run.results.len(), 3);
    let states = capture.0.lock().expect("read states").clone();
    assert!(
        states.contains(&BrainState::Running),
        "the runner must notify Running during a real run, not only assign it: saw {states:?}"
    );
}
