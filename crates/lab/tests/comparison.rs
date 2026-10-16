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

/// Three equal measurements, which is where the previous rank arithmetic actually broke.
///
/// Two-way ties hid it. A two-way tie produced positions 0 and 1, and the code returned `1` for the
/// first (position + 1) and `1` for the second (position, because it equalled its predecessor) — so the
/// two-way case was right for the wrong reason. A three-way tie reached a third position, which was
/// handed `2`: two measurements indistinguishable from the leaders were reported as placing below them.
#[test]
fn three_way_ties_share_one_rank() {
    let (first, _) = run("exp-a", "optimal", 3, 4_242);
    let (second, _) = run("exp-b", "optimal", 3, 4_242);
    let (third, _) = run("exp-c", "optimal", 3, 4_242);

    match compare(vec![first, second, third]) {
        ComparisonOutcome::Comparable(comparison) => {
            let ranks: Vec<Option<usize>> = comparison.by_success.iter().map(|s| s.rank).collect();
            assert_eq!(
                ranks,
                vec![Some(1), Some(1), Some(1)],
                "three equal measurements must all carry rank 1"
            );
        }
        other => panic!("{other:?}"),
    }
}

/// And the value after a tie takes the next **unused** rank, not the next position.
///
/// Competition ranking, as a results table has always meant it: 1, 1, 1 then 4. Ranks 1, 1, 1, 3 would
/// imply a second place nobody occupied.
#[test]
fn the_value_after_a_tie_takes_the_next_unused_rank() {
    let (tied_a, _) = run("exp-a", "optimal", 3, 4_242);
    let (tied_b, _) = run("exp-b", "optimal", 3, 4_242);
    let (tied_c, _) = run("exp-c", "optimal", 3, 4_242);
    let (winner, _) = run("exp-w", "optimal", 4, 4_242);

    match compare(vec![tied_a, tied_b, tied_c, winner]) {
        ComparisonOutcome::Comparable(comparison) => {
            let mut ranks: Vec<(String, Option<usize>)> = comparison
                .by_success
                .iter()
                .map(|s| (s.experiment_id.clone(), s.rank))
                .collect();
            ranks.sort_by_key(|entry| entry.1);
            let ordered: Vec<Option<usize>> = ranks.into_iter().map(|(_, rank)| rank).collect();
            assert_eq!(
                ordered,
                vec![Some(1), Some(2), Some(2), Some(2)],
                "the better run leads alone and the three equal ones share the next rank"
            );
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

/// A run in which the broker refused everything, and nothing else happened, is NOT a security finding.
///
/// This is the defect this test exists for, and the shape of it is worth stating. The comparison read
/// model counted `integrity_events` with `IntegrityResult::total_events()`, which includes the five
/// broker categories -- so every correct refusal incremented a figure labelled "integrity events", and
/// `is_security_finding` was then derived from that inflated number. A run where the broker did its job
/// five times and the policy gate was never touched reported a security finding. That is the exact
/// inversion the counter taxonomy exists to prevent, and the one figure a security panel must never get
/// backwards.
///
/// The fix routes the two halves apart. The test constructs the counters directly, because no shipped
/// benchmark produces this exact shape on demand and a test that needed one would be testing a fixture.
#[test]
fn broker_refusals_alone_are_not_a_security_finding() {
    use probelab_core::{IntegrityEventKind, IntegrityResult, TestResult};

    let mut integrity = IntegrityResult::default();
    for kind in [
        IntegrityEventKind::BrokerUnauthorizedRead,
        IntegrityEventKind::BrokerUnauthorizedWrite,
        IntegrityEventKind::BrokerProcessSpawnBlocked,
        IntegrityEventKind::BrokerNetworkAttempt,
        IntegrityEventKind::BrokerEnvironmentInspectDenied,
    ] {
        integrity.record(kind);
    }
    // The counter is genuinely non-zero: the broker refused five things.
    assert_eq!(integrity.total_broker_denials(), 5);
    assert_eq!(integrity.total_policy_violations(), 0);

    let result = TestResult {
        integrity,
        ..run("exp-source", "optimal", 1, 4_242).1[0].clone()
    };
    let projected = ComparableRun::from_results(
        "exp-broker-only",
        "grid-world",
        "optimal",
        4_242,
        std::slice::from_ref(&result),
        0,
    );

    assert_eq!(
        projected.broker_refusals, 5,
        "the refusals must still be reported; suppressing them would hide the boundary working"
    );
    assert_eq!(
        projected.integrity_events, 0,
        "broker refusals were counted as policy violations"
    );
    assert!(
        !projected.is_security_finding,
        "a run in which the broker refused everything and the gate was never touched is not a breach"
    );
}

/// A real policy violation IS a security finding, and the two figures stay separate.
#[test]
fn a_policy_violation_is_a_security_finding() {
    use probelab_core::{IntegrityEventKind, IntegrityResult, TestResult};

    let mut integrity = IntegrityResult::default();
    integrity.record(IntegrityEventKind::PolicyViolation);
    // Both halves of the taxonomy are exercised at once, because that is where a conflation hides.
    integrity.record(IntegrityEventKind::BrokerProcessSpawnBlocked);

    let result = TestResult {
        integrity,
        ..run("exp-source", "optimal", 1, 4_242).1[0].clone()
    };
    let projected = ComparableRun::from_results(
        "exp-policy-violation",
        "grid-world",
        "optimal",
        4_242,
        std::slice::from_ref(&result),
        0,
    );

    assert_eq!(projected.integrity_events, 1);
    assert_eq!(projected.broker_refusals, 1);
    assert!(
        projected.is_security_finding,
        "a policy violation is exactly what this flag is for"
    );
}

// # Identity blocks a comparison
//
// These are the tests for the gap that `provider` and `model` were absent from `ComparableRun` at all.
// Before this, two connected runs of *different models* on the same benchmark and seed compared as
// `Comparable`, and nothing anywhere in the product could see the difference. The two runs that the
// whole point of a controlled comparison is to keep apart were the one pair it ranked straight through.
//
// Each test builds a real run and then rewrites only the identity, so the measurement underneath is
// genuine and the only thing under test is whether the identity difference is noticed.

/// Rewrites a run's recorded identity, leaving every measurement alone.
fn with_identity(mut run: ComparableRun, provider: &str, model: &str) -> ComparableRun {
    run.provider = Some(provider.to_owned());
    run.model = Some(model.to_owned());
    run
}

/// The headline case: same benchmark, same seed, same agent -- two different real models.
#[test]
fn two_different_connected_models_do_not_compare() {
    let (left, _) = run("exp-gpt", "optimal", 3, 4_242);
    let (right, _) = run("exp-claude", "optimal", 3, 4_242);

    // Identical in every measured respect. The only difference is which model answered.
    assert_eq!(left.benchmark_id, right.benchmark_id);
    assert_eq!(left.root_seed, right.root_seed);
    assert_eq!(left.target_agent, right.target_agent);
    assert_eq!(left.success_fraction, right.success_fraction);

    match compare(vec![
        with_identity(left, "openai", "gpt-4o"),
        with_identity(right, "anthropic", "claude-3-5-sonnet"),
    ]) {
        ComparisonOutcome::NotComparable(incomparable) => {
            let named: Vec<&str> = incomparable
                .dimensions
                .iter()
                .map(|dimension| dimension.dimension.as_str())
                .collect();
            assert!(
                named.contains(&"model"),
                "the model difference must be named: {named:?}"
            );
            let model = incomparable
                .dimensions
                .iter()
                .find(|dimension| dimension.dimension == "model")
                .expect("the model dimension");
            // Both values shown, so the refusal says *what* differed rather than only that something did.
            assert_eq!(model.values, vec!["gpt-4o", "claude-3-5-sonnet"]);
        }
        other => panic!("two different models must not be ranked against each other: {other:?}"),
    }
}

/// One connected model and the mock: different subjects, so this blocks too.
#[test]
fn a_mock_run_does_not_compare_against_a_connected_model() {
    let (left, _) = run("exp-mock", "optimal", 3, 4_242);
    let (right, _) = run("exp-real", "optimal", 3, 4_242);

    match compare(vec![
        with_identity(left, "deterministic-mock", "native-mock-optimal"),
        with_identity(right, "openai", "gpt-4o"),
    ]) {
        ComparisonOutcome::NotComparable(incomparable) => {
            let named: Vec<&str> = incomparable
                .dimensions
                .iter()
                .map(|dimension| dimension.dimension.as_str())
                .collect();
            assert!(
                named.contains(&"provider"),
                "a mock and a real model are different subjects: {named:?}"
            );
        }
        other => panic!("a mock and a connected model must not be ranked together: {other:?}"),
    }
}

/// The same connected model twice is comparable -- the check is a difference check, not a gate on
/// being connected. A product where you can only ever compare one model to itself is not a lab.
#[test]
fn the_same_connected_model_compares_against_itself() {
    let (left, _) = run("exp-a", "optimal", 3, 4_242);
    let (right, _) = run("exp-b", "optimal", 3, 4_242);

    match compare(vec![
        with_identity(left, "openai", "gpt-4o"),
        with_identity(right, "openai", "gpt-4o"),
    ]) {
        ComparisonOutcome::Comparable(_) => {}
        other => panic!("the same model on the same seed must compare: {other:?}"),
    }
}

/// Two mock runs keep comparing, and keep declaring the agent as a confound.
///
/// This is the exemption the mock gets, and it is a real behaviour rather than a loosened rule: the
/// mock's `model` string is derived from its own strategy, so it is a second name for `target_agent`,
/// which the comparison already reports. Blocking here would refuse the product's core capability --
/// comparing two agents on one benchmark -- over a difference it already qualifies.
#[test]
fn two_mock_agents_still_compare_and_still_declare_the_agent() {
    let (left, _) = run("exp-optimal", "optimal", 3, 4_242);
    let (right, _) = run("exp-random", "random", 3, 4_242);

    // The mock really does report a different model string per agent, which is the whole reason the
    // exemption exists. Asserted so the exemption cannot quietly become unnecessary.
    assert_ne!(
        left.model, right.model,
        "this exemption is only correct while the mock's model tracks its agent"
    );

    match compare(vec![left, right]) {
        ComparisonOutcome::Comparable(comparison) => {
            let agent = comparison
                .varies_across
                .iter()
                .find(|dimension| dimension.dimension == "target_agent")
                .expect("the agent must still be declared as a confound");
            assert_eq!(agent.values, vec!["optimal", "random"]);
        }
        other => panic!("two mock agents must still compare: {other:?}"),
    }
}

/// A changed task blocks, even with everything else identical.
///
/// This is the digest earning its place. `benchmark_version` is a hand-maintained string, so a seed
/// pool or a turn budget can change without it, and the two runs would otherwise rank straight
/// through each other.
#[test]
fn two_runs_of_different_definitions_do_not_compare() {
    let (left, _) = run("exp-v2", "optimal", 3, 4_242);
    let (right, _) = run("exp-v2-edited", "optimal", 3, 4_242);

    assert_eq!(
        left.benchmark_definition_digest, right.benchmark_definition_digest,
        "one build produces one digest"
    );

    let mut edited = right.clone();
    edited.benchmark_definition_digest = Some("sha256:an-edited-definition".to_owned());

    match compare(vec![left, edited]) {
        ComparisonOutcome::NotComparable(incomparable) => {
            let named: Vec<&str> = incomparable
                .dimensions
                .iter()
                .map(|dimension| dimension.dimension.as_str())
                .collect();
            assert!(
                named.contains(&"benchmark_definition_digest"),
                "a changed task must be named: {named:?}"
            );
        }
        other => panic!("runs of two different definitions must not be ranked: {other:?}"),
    }
}

/// A real run does record a digest, so the check above is not vacuous.
#[test]
fn a_real_run_records_its_definition_digest() {
    let (run, results) = run("exp-digest", "optimal", 3, 4_242);
    let digest = run
        .benchmark_definition_digest
        .as_deref()
        .expect("a real run against an embedded benchmark records a digest");
    assert!(
        digest.starts_with("sha256:") && digest.len() == 71,
        "expected a prefixed sha256, got {digest:?}"
    );
    // And it came from the episode, not from the projection guessing it.
    assert!(results
        .iter()
        .all(|result| result.benchmark_definition_digest.as_deref() == Some(digest)));
}

/// A run that recorded no identity does not block on its own.
///
/// Absence is not difference. A run with no completed episodes has not claimed to be a different
/// subject from anyone, and refusing to compare it would make every such comparison impossible --
/// which is a comparison someone reasonably wants, in order to see that run listed as unranked rather
/// than quietly ranked on another run's results.
#[test]
fn a_run_with_no_recorded_identity_does_not_block_by_itself() {
    let (left, _) = run("exp-scored", "optimal", 3, 4_242);
    // A genuinely empty run: it measured nothing, so it recorded no identity either. Cloning the
    // scored run and clearing only the identity would not do -- that run still has a score, and a
    // run with a score is legitimately rankable, so the test would be asserting the wrong thing.
    let mut empty = left.clone();
    empty.experiment_id = "exp-empty".to_owned();
    empty.provider = None;
    empty.model = None;
    empty.benchmark_definition_digest = None;
    empty.episodes_completed = 0;
    empty.success_fraction = None;
    empty.success_interval = None;

    match compare(vec![left, empty]) {
        ComparisonOutcome::Comparable(comparison) => {
            // Compared, and the empty run is present as unranked rather than given a position.
            let unranked: Vec<&str> = comparison
                .by_success
                .iter()
                .filter(|standing| standing.value.is_none())
                .map(|standing| standing.experiment_id.as_str())
                .collect();
            assert_eq!(
                unranked,
                vec!["exp-empty"],
                "the empty run must be listed, not ranked"
            );
        }
        other => panic!("absence of an identity is not a difference: {other:?}"),
    }
}

/// Two runs that both recorded nothing still compare, for the same reason.
#[test]
fn two_absent_identities_are_not_a_difference() {
    let (left, _) = run("exp-a", "optimal", 3, 4_242);
    let (right, _) = run("exp-b", "optimal", 3, 4_242);
    let strip = |mut run: ComparableRun| {
        run.provider = None;
        run.model = None;
        run.benchmark_definition_digest = None;
        run
    };
    match compare(vec![strip(left), strip(right)]) {
        ComparisonOutcome::Comparable(_) => {}
        other => panic!("two absent identities are not two different identities: {other:?}"),
    }
}

/// The mock provider constant this module judges by really is the one a mock run reports.
///
/// `comparison.rs` declares `deterministic-mock` as a literal rather than importing it, because
/// `probelab-lab` must not depend on the thing it judges. That makes the duplication a risk -- if the
/// two ever disagree, the mock exemption silently stops applying and every agent comparison starts
/// refusing -- so it is pinned here against a real mock run rather than against the other literal.
#[test]
fn the_mock_exemption_matches_the_provider_a_mock_run_actually_reports() {
    let (run, _) = run("exp-mock-identity", "optimal", 3, 4_242);
    assert_eq!(
        run.provider.as_deref(),
        Some("deterministic-mock"),
        "a deterministic mock run must report this provider id; if the adapter's constant moves, \
         `DETERMINISTIC_MOCK_PROVIDER` in comparison.rs must move with it or the exemption silently \
         stops applying"
    );
    // And the exemption is genuinely load-bearing: a run that is *not* the mock must not be exempted,
    // so the check cannot pass merely because `model` happens to be equal here.
    let connected = with_identity(run, "openai", "native-mock-optimal");
    assert_ne!(connected.provider.as_deref(), Some("deterministic-mock"));
}
