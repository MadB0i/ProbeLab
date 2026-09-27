//! Float-precision regression tests.
//!
//! # What this found
//!
//! A read-model round-trip failed on a Wilson interval bound, differing in the last unit in the last
//! place. The cause was not in this code: `serde_json`'s default writer and parser can disagree by one
//! ULP on some `f64` values, and the workspace now enables the opt-in `float_roundtrip` feature to stop
//! it.
//!
//! That matters more here than it would in most repositories. This one stores `f64` success rates,
//! Wilson interval bounds, mean rewards, and mean turn counts in result bundles and read models. A
//! measurement that came back subtly different from the one recorded is a defect in recorded data, and
//! it is the kind that erodes trust in every number the lab reports without ever looking obviously
//! wrong.
//!
//! The tests below pin the behaviour so a dependency change cannot silently reintroduce the drift.

use probelab_lab::runner::wilson_interval;

/// The specific value that exposed the drift, kept as a literal.
///
/// If this test ever needs its literal updated, the dependency's behaviour changed — which is exactly
/// when someone should be told.
const EXPOSED_LOWER_BOUND: f64 = 0.09453120573423071;

/// An interval survives a JSON round trip bit-exactly.
#[test]
fn an_interval_round_trips_bit_exactly() {
    let interval = wilson_interval(1, 2, 0.95);
    let json = serde_json::to_string(&interval).expect("serialize");
    let restored: probelab_engine::harness::Interval = serde_json::from_str(&json).expect("parse");
    assert_eq!(restored, interval);
    // Bit equality, not approximate equality. An epsilon comparison here would have hidden the very
    // defect this file exists to prevent.
    for (name, original, back) in [
        ("point", interval.point, restored.point),
        ("lower", interval.lower, restored.lower),
        ("upper", interval.upper, restored.upper),
    ] {
        assert_eq!(
            original.to_bits(),
            back.to_bits(),
            "the {name} bound drifted by a ULP through JSON: {original:?} -> {back:?}"
        );
    }
}

/// The specific value that exposed the defect still round-trips exactly.
#[test]
fn the_exposing_value_is_still_exact() {
    let json = serde_json::to_string(&EXPOSED_LOWER_BOUND).expect("serialize");
    let back: f64 = serde_json::from_str(&json).expect("parse");
    assert_eq!(
        EXPOSED_LOWER_BOUND.to_bits(),
        back.to_bits(),
        "serde_json drifted on the value that exposed the problem: wrote {json}"
    );
}

/// A wide spread of values round-trips exactly, not just the one that broke.
#[test]
fn many_values_round_trip_exactly() {
    // Interval bounds, ratios, and the sort of quantity this lab stores. Sampling broadly means a
    // regression is caught even if it only affects values this particular run did not produce.
    let mut values: Vec<f64> = vec![0.0, 1.0, 0.5, -0.0];
    for successes in 0..=30usize {
        for trials in [1usize, 5, 30, 97] {
            let interval = wilson_interval(successes, trials, 0.95);
            values.extend([interval.point, interval.lower, interval.upper]);
            if trials > 0 {
                values.push(successes as f64 / trials as f64);
            }
        }
    }
    values.extend([f64::MIN_POSITIVE, f64::EPSILON, 1e-300, 1e300]);

    for value in values {
        let json = serde_json::to_string(&value).expect("serialize");
        if !value.is_finite() {
            // serde_json writes a non-finite f64 as null, which is a documented lossy encoding rather
            // than a round-trip guarantee. Asserted separately so it is a known limitation and not a
            // surprise.
            assert_eq!(json, "null", "non-finite {value:?} should encode as null");
            continue;
        }
        let back: f64 = serde_json::from_str(&json).expect("parse");
        assert_eq!(
            value.to_bits(),
            back.to_bits(),
            "{value:?} did not survive JSON: wrote {json}, read {back:?}"
        );
    }
}

/// The aggregate round-trips exactly, floats included.
///
/// The same guarantee one level up, on the type that actually reaches a result bundle.
#[test]
fn the_aggregate_round_trips_exactly() {
    let mut brain = probelab_lab::LabBrain::new();
    let run = probelab_lab::LabRunner::new()
        .run(
            &mut brain,
            &probelab_lab::PlanRequest {
                benchmark_id: Some("grid-world".to_owned()),
                target_agent: Some("optimal".to_owned()),
                probe_agents: Vec::new(),
                episodes: Some(3),
                root_seed: Some(4_242),
            },
            "exp-precision",
            &probelab_evaluator::GridWorldEvaluator::new(),
            std::sync::Arc::new(probelab_engine::arena::NullRecorder),
        )
        .expect("run");
    let aggregate = run.aggregate().clone();
    let json = serde_json::to_string(&aggregate).expect("serialize");
    let restored: probelab_lab::ExperimentAggregate = serde_json::from_str(&json).expect("parse");

    assert_eq!(
        restored.success_rate.to_bits(),
        aggregate.success_rate.to_bits()
    );
    assert_eq!(
        restored.success_interval.lower.to_bits(),
        aggregate.success_interval.lower.to_bits()
    );
    assert_eq!(
        restored.success_interval.upper.to_bits(),
        aggregate.success_interval.upper.to_bits()
    );
    assert_eq!(
        restored.mean_reward.to_bits(),
        aggregate.mean_reward.to_bits()
    );
    assert_eq!(restored, aggregate);
}
