//! Comparison tests.
//!
//! The tests are weighted towards the refusals, because that is where a comparison panel does real
//! damage. A ranking that is wrong is a visible bug; a ranking that is applied to two runs of
//! different benchmarks is an invisible one, and it produces a confident number that means nothing.

use std::sync::Arc;

use probelab_lab::{compare, ComparableRun, ComparisonOutcome, LabBrain, LabRunner, PlanRequest};

/// Builds a run by actually executing it, so the comparison is tested against real measurements
/// rather than against hand-written fixtures that could agree with a broken projection.
fn run(
    experiment_id: &str,
    agent: &str,
    episodes: usize,
    seed: u64,
) -> (ComparableRun, Vec<probelab_core::TestResult>) {
    let mut brain = LabBrain::new();
    let executed = LabRunner::new()
        .run(
            &mut brain,
            &PlanRequest {
                benchmark_id: Some("grid-world".to_owned()),
                target_agent: Some(agent.to_owned()),
                probe_agents: Vec::new(),
                episodes: Some(episodes),
                root_seed: Some(seed),
            },
            experiment_id,
            &probelab_evaluator::GridWorldEvaluator::new(),
            Arc::new(probelab_engine::arena::NullRecorder),
        )
        .expect("run");
    let projected = ComparableRun::from_results(
        experiment_id,
        "grid-world",
        agent,
        seed,
        executed.results.as_slice(),
        executed.failures.len(),
    );
    (projected, executed.results)
}

#[test]
fn two_runs_of_the_same_experiment_are_comparable() {
    let (left, _) = run("exp-a", "optimal", 3, 4_242);
    let (right, _) = run("exp-b", "optimal", 3, 4_242);
    match compare(vec![left, right]) {
        ComparisonOutcome::Comparable(comparison) => {
            assert_eq!(comparison.benchmark_id, "grid-world");
            assert_eq!(comparison.target_agent, "optimal");
            assert_eq!(comparison.root_seed, 4_242);
            assert_eq!(comparison.runs.len(), 2);
        }
        other => panic!("identical experiments must compare: {other:?}"),
    }
}

/// The central refusal. A success rate from another benchmark is not a better or worse result, and
/// ranking the two would be the single most misleading thing this product could do.
#[test]
fn a_different_benchmark_is_refused() {
    let (left, _) = run("exp-a", "optimal", 2, 4_242);
    let (mut right, _) = run("exp-b", "optimal", 2, 4_242);
    right.benchmark_id = "sandbox-process".to_owned();
    match compare(vec![left, right]) {
        ComparisonOutcome::NotComparable(reason) => {
            let dimension = reason
                .dimensions
                .iter()
                .find(|dimension| dimension.dimension == "benchmark")
                .expect("the benchmark difference must be named");
            assert_eq!(dimension.values, vec!["grid-world", "sandbox-process"]);
            assert!(reason.reason().contains("benchmark differs"));
        }
        other => panic!("a different benchmark must not compare: {other:?}"),
    }
}

#[test]
fn a_different_seed_is_refused() {
    let (left, _) = run("exp-a", "optimal", 2, 4_242);
    let (right, _) = run("exp-b", "optimal", 2, 9_999);
    match compare(vec![left, right]) {
        ComparisonOutcome::NotComparable(reason) => {
            assert!(
                reason.dimensions.iter().any(|d| d.dimension == "root_seed"),
                "a different seed changes what the numbers mean: {:?}",
                reason.dimensions
            );
        }
        other => panic!("a different seed must not compare: {other:?}"),
    }
}

/// Comparing two different agents is a confound, not a refusal.
///
/// Refusing it would make the feature useless — comparing agents is the obvious thing to ask for,
/// and it is not this product's call to make on the user's behalf. But the confound has to travel
/// with the numbers, so the comparison reports what was not held constant and a panel is required
/// to show it above the ranking.
#[test]
fn a_different_agent_compares_but_declares_the_confound() {
    let (left, _) = run("exp-a", "optimal", 2, 4_242);
    let (right, _) = run("exp-b", "random", 2, 4_242);
    match compare(vec![left, right]) {
        ComparisonOutcome::Comparable(comparison) => {
            let named: Vec<&str> = comparison
                .varies_across
                .iter()
                .map(|dimension| dimension.dimension.as_str())
                .collect();
            assert!(
                named.contains(&"target_agent"),
                "a differing agent must be declared: {named:?}"
            );
            let agent = comparison
                .varies_across
                .iter()
                .find(|dimension| dimension.dimension == "target_agent")
                .expect("the agent dimension");
            assert_eq!(agent.values, vec!["optimal", "random"]);
        }
        other => panic!("two agents on one benchmark and seed do compare: {other:?}"),
    }
}

/// Runs that differ only in how many episodes they had have no confound, because that is exactly
/// the comparison the feature exists for.
#[test]
fn runs_differing_only_in_episode_count_declare_no_confound() {
    let (left, _) = run("exp-a", "optimal", 2, 4_242);
    let (right, _) = run("exp-b", "optimal", 20, 4_242);
    match compare(vec![left, right]) {
        ComparisonOutcome::Comparable(comparison) => {
            assert!(
                comparison.varies_across.is_empty(),
                "episode count is not a confound: {:?}",
                comparison.varies_across
            );
        }
        other => panic!("{other:?}"),
    }
}

/// Every blocking difference must be reported, not just the first. A panel that reported only the
/// benchmark would send someone to fix it, re-run, and hit the seed difference next.
#[test]
fn every_blocking_dimension_is_reported_not_just_the_first() {
    let (left, _) = run("exp-a", "optimal", 2, 4_242);
    let (mut right, _) = run("exp-b", "random", 2, 9_999);
    right.benchmark_id = "sandbox-process".to_owned();
    match compare(vec![left, right]) {
        ComparisonOutcome::NotComparable(reason) => {
            let named: Vec<&str> = reason
                .dimensions
                .iter()
                .map(|dimension| dimension.dimension.as_str())
                .collect();
            for expected in ["benchmark", "root_seed"] {
                assert!(
                    named.contains(&expected),
                    "{expected} missing from {named:?}"
                );
            }
        }
        other => panic!("these runs differ in the measurement identity: {other:?}"),
    }
}

#[test]
fn a_single_run_cannot_be_compared() {
    let (only, _) = run("exp-a", "optimal", 2, 4_242);
    match compare(vec![only]) {
        ComparisonOutcome::NotComparable(reason) => {
            assert_eq!(reason.experiment_ids, vec!["exp-a"]);
            assert_eq!(reason.dimensions[0].dimension, "run_count");
        }
        other => panic!("one run is not a comparison: {other:?}"),
    }
}

#[test]
fn no_runs_is_not_a_comparison() {
    assert!(matches!(
        compare(Vec::new()),
        ComparisonOutcome::NotComparable(_)
    ));
}

/// A run that completed nothing has no score, and that absence has to survive the whole comparison
/// rather than becoming a zero that then loses every ranking.
///
/// The empty run is constructed directly rather than by executing one, because every registered
/// agent completes at least one episode on grid-world: a run that completed nothing is a real state
/// the product persists, but not one the mock agents happen to produce here.
#[test]
fn a_run_with_no_completed_episode_has_no_score_and_says_so() {
    let empty = ComparableRun::from_results("exp-empty", "grid-world", "optimal", 4_242, &[], 3);
    assert_eq!(empty.episodes_completed, 0);
    assert_eq!(empty.episodes_failed, 3);
    assert_eq!(empty.success_fraction, None, "no episodes means no score");
    assert_eq!(empty.success_interval, None);

    let (scored, _) = run("exp-scored", "optimal", 2, 4_242);
    match compare(vec![empty, scored]) {
        ComparisonOutcome::Comparable(comparison) => {
            let standing = comparison
                .by_success
                .iter()
                .find(|standing| standing.experiment_id == "exp-empty")
                .expect("the unscorable run must still appear");
            assert_eq!(
                standing.rank, None,
                "no score means no rank, not last place"
            );
            assert_eq!(standing.value, None);
            assert!(
                standing
                    .unranked_reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("no success fraction")),
                "the reason must be stated, not implied: {:?}",
                standing.unranked_reason
            );
        }
        other => panic!("these two are comparable: {other:?}"),
    }
}

/// A model descriptor cannot be read from a run with no episodes, and inventing one would put a
/// fabricated adapter name in a comparison table.
#[test]
fn a_run_with_no_episodes_names_no_model() {
    let empty = ComparableRun::from_results("exp-empty", "grid-world", "optimal", 4_242, &[], 3);
    assert_eq!(empty.adapter, "none");
    assert_eq!(empty.strategy, "none");
    assert_eq!(empty.mean_reward, 0.0);
    assert_eq!(empty.mean_turns, 0.0);
    assert_eq!(empty.mean_invalid_actions, 0.0);
}

/// The optimal agent should beat a random one on the same seed. If it does not, the projection is
/// reading the wrong field, and every other test here would still pass.
#[test]
fn the_stronger_agent_ranks_first() {
    let (strong, _) = run("exp-optimal", "optimal", 5, 4_242);
    let (weak, _) = run("exp-random", "random", 5, 4_242);
    match compare(vec![strong, weak]) {
        ComparisonOutcome::Comparable(comparison) => {
            let first = comparison
                .by_success
                .first()
                .expect("two comparable runs produce standings");
            assert_eq!(
                first.experiment_id, "exp-optimal",
                "the optimal agent must rank first on the same seed"
            );
            let strong_score = comparison
                .by_success
                .iter()
                .find(|s| s.experiment_id == "exp-optimal")
                .and_then(|s| s.value)
                .expect("a value");
            let weak_score = comparison
                .by_success
                .iter()
                .find(|s| s.experiment_id == "exp-random")
                .and_then(|s| s.value)
                .expect("a value");
            assert!(
                strong_score > weak_score,
                "{strong_score} should beat {weak_score}"
            );
        }
        other => panic!("same seed, same benchmark: {other:?}"),
    }
}

/// Fewer turns is better. A ranking that reversed this would be a silent lie in the other direction.
#[test]
fn fewer_turns_ranks_higher() {
    let (strong, _) = run("exp-optimal", "optimal", 3, 4_242);
    let (weak, _) = run("exp-random", "random", 3, 4_242);
    match compare(vec![strong, weak]) {
        ComparisonOutcome::Comparable(comparison) => {
            let first = comparison.by_turns.first().expect("standings");
            assert_eq!(first.experiment_id, "exp-optimal");
        }
        other => panic!("{other:?}"),
    }
}

/// Two runs of the same experiment over the same seed measure the same thing, so they tie. Reporting
/// 1 and 2 would make the ranking look more precise than the data is.
#[test]
fn identical_measurements_share_a_rank() {
    let (left, _) = run("exp-a", "optimal", 3, 4_242);
    let (right, _) = run("exp-b", "optimal", 3, 4_242);
    match compare(vec![left, right]) {
        ComparisonOutcome::Comparable(comparison) => {
            let ranks: Vec<Option<usize>> = comparison.by_success.iter().map(|s| s.rank).collect();
            assert_eq!(
                ranks,
                vec![Some(1), Some(1)],
                "equal values must share a rank"
            );
        }
        other => panic!("{other:?}"),
    }
}

/// A one-episode run is not a better agent than a thirty-episode one. The interval is what shows
/// that, so the comparison has to carry it.
#[test]
fn a_thin_run_carries_a_wide_interval() {
    let (thin, _) = run("exp-thin", "optimal", 1, 4_242);
    let (thick, _) = run("exp-thick", "optimal", 20, 4_242);
    match compare(vec![thin, thick]) {
        ComparisonOutcome::Comparable(comparison) => {
            let thin_interval = comparison
                .by_success
                .iter()
                .find(|s| s.experiment_id == "exp-thin")
                .and_then(|s| s.interval)
                .expect("the thin run must carry an interval");
            let thick_interval = comparison
                .by_success
                .iter()
                .find(|s| s.experiment_id == "exp-thick")
                .and_then(|s| s.interval)
                .expect("the thick run must carry an interval");
            let thin_width = thin_interval.upper - thin_interval.lower;
            let thick_width = thick_interval.upper - thick_interval.lower;
            assert!(
                thin_width > thick_width,
                "one episode must be visibly less certain: {thin_width} vs {thick_width}"
            );
        }
        other => panic!("{other:?}"),
    }
}

/// Every ranking covers every run, including the ones with nothing to rank. A standing list that
/// silently dropped a run would make a comparison look complete when it is not.
#[test]
fn every_ranking_accounts_for_every_run() {
    let empty = ComparableRun::from_results("exp-empty", "grid-world", "optimal", 4_242, &[], 3);
    let (scored, _) = run("exp-scored", "optimal", 2, 4_242);
    match compare(vec![empty, scored]) {
        ComparisonOutcome::Comparable(comparison) => {
            for (name, standings) in [
                ("success", &comparison.by_success),
                ("reward", &comparison.by_reward),
                ("turns", &comparison.by_turns),
                ("invalid", &comparison.by_invalid_actions),
            ] {
                let ids: Vec<&str> = standings
                    .iter()
                    .map(|standing| standing.experiment_id.as_str())
                    .collect();
                assert_eq!(ids.len(), 2, "the {name} ranking dropped a run: {ids:?}");
                assert!(
                    ids.contains(&"exp-empty"),
                    "the {name} ranking lost the empty run"
                );
            }
        }
        other => panic!("{other:?}"),
    }
}

/// The projection reads the environment's own numbers. A comparison that reported a model's
/// self-reported reward instead would be reporting a claim.
#[test]
fn the_projection_uses_environment_numbers() {
    let (_, results) = run("exp-a", "optimal", 3, 4_242);
    let projected = ComparableRun::from_results(
        "exp-a",
        "grid-world",
        "optimal",
        4_242,
        results.as_slice(),
        0,
    );
    let successes = results
        .iter()
        .filter(|result| result.capability.task_success)
        .count();
    let expected = successes as f64 / results.len() as f64;
    assert_eq!(projected.success_fraction, Some(expected));
    assert_eq!(projected.episodes_completed, results.len());

    let mean_turns = results
        .iter()
        .map(|r| r.capability.turns_used as f64)
        .sum::<f64>()
        / results.len() as f64;
    assert_eq!(projected.mean_turns, mean_turns);
}

/// A comparison is a report, so it crosses the wire. The absence of a score has to stay absent on
/// the wire rather than arriving at the panel as a zero.
#[test]
fn a_comparison_round_trips_through_the_wire() {
    use serde::{Deserialize, Serialize};

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(
        value: &T,
    ) -> T {
        let text = serde_json::to_string(value).expect("serialize");
        serde_json::from_str(&text).expect("parse")
    }

    let (empty, _) = run("exp-empty", "optimal", 2, 4_242);
    let (scored, _) = run("exp-scored", "optimal", 2, 4_242);
    let outcome = compare(vec![empty, scored]);
    assert_eq!(round_trip(&outcome), outcome);

    let (left, _) = run("exp-a", "optimal", 2, 4_242);
    let (mut right, _) = run("exp-b", "optimal", 2, 9_999);
    right.benchmark_id = "sandbox-process".to_owned();
    let refused = compare(vec![left, right]);
    assert_eq!(round_trip(&refused), refused);
}
