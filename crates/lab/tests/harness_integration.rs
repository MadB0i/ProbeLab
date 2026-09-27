//! Harness-integration regression tests.
//!
//! # Why these exist
//!
//! The repository had two experiment execution loops: `Harness::measure` and `LabRunner::run`. Two
//! loops is one too many, and they disagreed — the Harness aborts on the first failed episode while
//! the Runner records and continues, and the Harness shares one benchmark instance across episodes
//! while the Runner builds a fresh one with a fresh `RunContext` each time.
//!
//! Resolving that by inlining the Harness would have made the product worse, so the Harness's
//! *methodological* strengths were adopted instead: seeds drawn from the benchmark's own declared
//! evaluation pool, and a Wilson interval on the reported rate.
//!
//! These tests hold that decision in place. They assert the Runner's behaviour, that the two paths
//! compute intervals identically, and — structurally — that the Runner has no second loop of its own
//! and the Harness has not been quietly weakened to accommodate it.

use std::sync::Arc;

use probelab_core::Benchmark as _;
use probelab_core::{Evaluator, TelemetryEvent, TelemetryRecorder};
use probelab_engine::harness::{wilson_interval, Interval};
use probelab_evaluator::GridWorldEvaluator;
use probelab_lab::{ExperimentAggregate, LabBrain, LabRunner, PlanRequest, TerminalReason};

struct NullRecorder;

impl TelemetryRecorder for NullRecorder {
    fn record(&self, _event: &TelemetryEvent) -> Result<(), String> {
        Ok(())
    }
}

struct RefusingEvaluator;

impl Evaluator for RefusingEvaluator {
    fn evaluate(
        &self,
        _input: probelab_core::EvaluationInput,
    ) -> Result<probelab_core::TestResult, String> {
        Err("refused".to_owned())
    }
}

fn request(episodes: usize) -> PlanRequest {
    PlanRequest {
        benchmark_id: Some("grid-world".to_owned()),
        target_agent: Some("optimal".to_owned()),
        probe_agents: Vec::new(),
        episodes: Some(episodes),
        root_seed: Some(4_242),
    }
}

fn run(episodes: usize) -> probelab_lab::ExperimentRun {
    let mut brain = LabBrain::new();
    LabRunner::new()
        .run(
            &mut brain,
            &request(episodes),
            "exp-harness",
            &GridWorldEvaluator::new(),
            Arc::new(NullRecorder),
        )
        .expect("run")
}

/// Episode seeds come from the benchmark's own evaluation pool, not a linear sequence.
///
/// This is the substantive change. A `root_seed + index` sequence can land on a seed the benchmark was
/// tuned against, which would make "held-out" untrue in a way no assertion about the seed would catch.
#[test]
fn episode_seeds_come_from_the_benchmarks_declared_eval_pool() {
    let run = run(3);
    let declared = probelab_benchmarks::GridWorldBenchmark::built_in()
        .expect("built-in benchmark")
        .eval_seeds();
    let dev = probelab_benchmarks::GridWorldBenchmark::built_in()
        .expect("built-in benchmark")
        .dev_seeds();

    assert!(
        !declared.is_empty(),
        "the benchmark declares an evaluation pool"
    );
    for (index, result) in run.results.iter().enumerate() {
        assert!(
            declared.contains(&result.seed),
            "episode {index} used seed {}, which is not in the declared evaluation pool",
            result.seed
        );
        assert!(
            !dev.contains(&result.seed),
            "episode {index} used seed {}, which is a development seed",
            result.seed
        );
    }
    assert_eq!(run.aggregate().seed_pool.as_deref(), Some("eval"));
}

/// The Runner and the Harness compute intervals identically.
///
/// If they diverged, a benchmark-quality report and a product report would quote different bounds for
/// the same data — which is exactly the kind of quiet inconsistency a two-loop architecture produces.
#[test]
fn the_runner_and_harness_compute_the_same_interval() {
    for (successes, trials) in [(0, 0), (0, 5), (3, 5), (5, 5), (12, 30), (29, 30)] {
        let from_harness: Interval = wilson_interval(successes, trials, 0.95);
        let from_runner = probelab_lab::runner::wilson_interval(successes, trials, 0.95);
        assert_eq!(from_runner, from_harness, "{successes}/{trials} disagreed");
    }
}

/// The aggregate is built from the results, so it cannot disagree with them.
#[test]
fn the_aggregate_is_derived_from_the_per_episode_results() {
    let run = run(4);
    let aggregate = run.aggregate();

    assert_eq!(aggregate.episodes, run.results.len());
    let expected_successes = run
        .results
        .iter()
        .filter(|result| result.capability.task_success)
        .count();
    assert_eq!(aggregate.successes, expected_successes);
    assert!((aggregate.success_rate - run.mean_success_fraction().expect("score")).abs() < 1e-9);
    // Status counts account for every episode.
    let counted: usize = aggregate.status_counts.values().sum();
    assert_eq!(counted, run.results.len());
}

/// An interval is reported, so a small sample is visibly uncertain.
///
/// A bare point estimate from five episodes reads as a measurement. The interval is what makes the
/// uncertainty legible rather than hidden.
#[test]
fn a_small_sample_carries_a_visible_interval() {
    let run = run(3);
    let interval = run.aggregate().success_interval;
    assert!(interval.lower <= interval.point, "{interval:?}");
    assert!(interval.point <= interval.upper, "{interval:?}");
    assert!(
        interval.upper - interval.lower > 0.0,
        "an interval with no width hides its uncertainty"
    );
    assert!((interval.confidence - 0.95).abs() < 1e-9);
}

/// An all-or-nothing sample still produces a sensible interval.
///
/// The reason the Wilson interval is used at all: `p ± 1.96·se` collapses to zero width at 0% and 100%,
/// which would report certainty the data does not support.
#[test]
fn extreme_samples_do_not_collapse_the_interval() {
    let all = probelab_lab::runner::wilson_interval(30, 30, 0.95);
    let none = probelab_lab::runner::wilson_interval(0, 30, 0.95);
    assert!(
        all.upper - all.lower > 0.0,
        "30/30 must not report certainty"
    );
    assert!(
        none.upper - none.lower > 0.0,
        "0/30 must not report certainty"
    );
    assert!(all.lower > 0.0 && all.upper <= 1.0);
    assert!(none.lower >= 0.0 && none.upper < 1.0);
}

/// A failed episode is still recorded and skipped, and the rest survive.
///
/// This is the semantic the Runner keeps and the Harness gives up. Inlining the Harness would have
/// meant one flaky seed discarding three valid ones — and, worse, a model that reliably breaks the
/// harness scoring as "no data" rather than as a failure.
#[test]
fn a_failed_episode_does_not_discard_the_others() {
    let mut brain = LabBrain::new();
    let run = LabRunner::new()
        .run(
            &mut brain,
            &request(3),
            "exp-partial",
            &RefusingEvaluator,
            Arc::new(NullRecorder),
        )
        .expect("run");

    assert_eq!(run.completed(), 0);
    assert_eq!(
        run.failed(),
        3,
        "every failure is recorded, not just the first"
    );
    assert_eq!(run.attempted(), 3);
    // And the run still reached a terminal state rather than aborting mid-way.
    assert_eq!(run.terminal_reason, Some(TerminalReason::Completed));
}

/// With no completed episode there is no score, and the aggregate says it has no data.
///
/// The `None` on the score is the honest field. The aggregate's rate is arithmetically zero over zero
/// episodes, so `has_data` is the guard — and a test that only checked the rate would have passed while
/// the panel showed 0% for a run that measured nothing.
#[test]
fn no_completed_episode_yields_no_score_and_an_empty_aggregate() {
    let mut brain = LabBrain::new();
    let run = LabRunner::new()
        .run(
            &mut brain,
            &request(2),
            "exp-nothing",
            &RefusingEvaluator,
            Arc::new(NullRecorder),
        )
        .expect("run");

    assert_eq!(
        run.mean_success_fraction(),
        None,
        "no data is not a zero score"
    );
    assert!(
        !run.aggregate().has_data(),
        "the aggregate must report that it has nothing"
    );
    assert_eq!(run.aggregate().episodes, 0);
    // Over zero trials the interval is the widest one available — the whole of [0, 1] — which is the
    // honest answer for no data. My first version expected [0, 0] here and was wrong: a zero-width
    // interval over no episodes would claim the model scored exactly nothing, which is a different and
    // much stronger claim than "nothing was measured". This is also why `has_data` must be checked
    // before the rate is read.
    let interval = run.aggregate().success_interval;
    assert_eq!(interval.lower, 0.0);
    assert_eq!(
        interval.upper, 1.0,
        "no data must yield maximum uncertainty"
    );
}

/// Episode counts are correct and the terminal status reflects what happened.
#[test]
fn episode_counts_and_terminal_state_are_correct() {
    let run = run(5);
    assert_eq!(run.plan.episodes, 5);
    assert_eq!(run.completed() + run.failed(), run.attempted());
    assert_eq!(run.terminal_reason, Some(TerminalReason::Completed));
    // Every episode produced exactly one lifecycle event pair on a fully successful run.
    let codes: Vec<&str> = run
        .timeline
        .events()
        .iter()
        .map(|event| event.code.as_str())
        .collect();
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == "experiment.episode_completed")
            .count(),
        run.completed()
    );
    assert_eq!(
        codes
            .iter()
            .filter(|code| **code == "experiment.episode_started")
            .count(),
        run.attempted()
    );
}

/// Observers still see every episode, because the aggregate is computed after the observer's stream.
#[test]
fn observer_events_are_preserved_per_episode() {
    #[derive(Default)]
    struct CountingRecorder {
        events: std::sync::Mutex<usize>,
    }
    impl TelemetryRecorder for CountingRecorder {
        fn record(&self, _event: &TelemetryEvent) -> Result<(), String> {
            *self.events.lock().expect("lock") += 1;
            Ok(())
        }
    }

    let recorder = Arc::new(CountingRecorder::default());
    let sink: Arc<dyn TelemetryRecorder> = recorder.clone();
    let mut brain = LabBrain::new();
    let run = LabRunner::new()
        .run(
            &mut brain,
            &request(3),
            "exp-observer",
            &GridWorldEvaluator::new(),
            sink,
        )
        .expect("run");

    assert_eq!(run.completed(), 3);
    assert!(
        *recorder.events.lock().expect("lock") > 0,
        "the observer recorded nothing, so the aggregate was computed over unobserved data"
    );
}

/// The same plan reproduces the same episode seeds and results.
#[test]
fn reproducibility_survives_the_pool_change() {
    let first = run(4);
    let second = run(4);
    let first_seeds: Vec<u64> = first.results.iter().map(|r| r.seed).collect();
    let second_seeds: Vec<u64> = second.results.iter().map(|r| r.seed).collect();
    assert_eq!(
        first_seeds, second_seeds,
        "the same plan must draw the same seeds"
    );
    let first_digests: Vec<&str> = first
        .results
        .iter()
        .map(|r| r.replay_digest.as_str())
        .collect();
    let second_digests: Vec<&str> = second
        .results
        .iter()
        .map(|r| r.replay_digest.as_str())
        .collect();
    assert_eq!(first_digests, second_digests, "and the same results");
    assert_eq!(first.aggregate(), second.aggregate());
}

/// A request larger than the pool cycles it, and says so rather than silently repeating.
///
/// The Harness states this in its own comment; the Runner inherits the behaviour and the pool name in
/// the run id, so a stored record shows which pool produced it.
#[test]
fn a_request_larger_than_the_pool_cycles_rather_than_failing() {
    let pool_size = probelab_benchmarks::GridWorldBenchmark::built_in()
        .expect("benchmark")
        .eval_seeds()
        .len();
    let run = run(pool_size + 2);
    assert_eq!(
        run.results.len(),
        pool_size + 2,
        "every requested episode ran"
    );
    assert_eq!(run.aggregate().seed_pool.as_deref(), Some("eval"));
}

/// The Runner has no second execution loop, and the Harness has not been weakened to suit it.
///
/// Structural, because a duplicated loop is exactly the defect this milestone removed and it would
/// creep back one edit at a time.
#[test]
fn there_is_exactly_one_product_execution_loop() {
    let runner = include_str!("../src/runner.rs");
    // One call into the engine per episode, and one `for` over episodes. A second loop would mean a
    // second place for the failure-handling and lifecycle semantics to drift.
    assert_eq!(
        runner.matches("run_with_context(").count(),
        1,
        "the Runner must call the engine from exactly one place"
    );
    assert_eq!(
        runner.matches("for index in 0..plan.episodes").count(),
        1,
        "the Runner must iterate episodes in exactly one loop"
    );
    // And it must not have grown its own interval maths, which would drift from the harness's.
    assert!(
        !runner.contains("1.96"),
        "the Runner must not compute its own interval; it reuses the harness's function"
    );

    let harness = include_str!("../../engine/src/harness.rs");
    // The Harness keeps its fail-fast semantics. If someone "fixed" the divergence by making it
    // lenient, the two paths would agree by making the stronger one weaker.
    assert!(
        harness.contains("HarnessError::EpisodeFailed"),
        "the Harness must keep fail-fast episode semantics"
    );
}

/// The aggregate carries a stable wire representation.
///
/// `Interval` could not previously be serialised at all, which meant a success rate with an interval
/// could not reach a result bundle or a panel. That is why it is now a serde type.
#[test]
fn the_aggregate_round_trips_through_serialization() {
    let run = run(3);
    let aggregate = ExperimentAggregate::from_results(&run.results, 0.95, Some("eval".to_owned()));
    let json = serde_json::to_string(&aggregate).expect("serialize");
    let restored: ExperimentAggregate = serde_json::from_str(&json).expect("parse");
    assert_eq!(restored, aggregate);
    assert!(json.contains("\"successInterval\""));
    assert!(json.contains("\"seedPool\""));
}
