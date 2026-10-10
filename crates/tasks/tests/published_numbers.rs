//! The published STATUS.md §7 task-benchmark table, pinned to the code.
//!
//! A measured number in a document that no test reproduces is not evidence —
//! that is why the old sandbox-process table was deleted. These reference
//! evaluations are in-process library calls, so the table values are asserted
//! here within a small tolerance rather than trusted from prose.

use probelab_tasks::{coding, memory};

fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 0.001,
        "expected {expected:.4}, measured {actual:.4}"
    );
}

#[test]
fn the_published_coding_repair_numbers_reproduce() {
    let seeds = coding::eval_seeds();
    let optimal = probelab_tasks::runner::run_coding_repair(
        probelab_tasks::runner::TaskPolicy::ReferenceOptimal,
        &seeds,
    )
    .expect("reference run");
    assert_eq!(optimal.successes, 70);
    assert_eq!(optimal.trials, 70);
    close(optimal.interval_lower, 0.948);
    close(optimal.interval_upper, 1.000);

    let noop = probelab_tasks::runner::run_coding_repair(
        probelab_tasks::runner::TaskPolicy::ReferenceBaseline,
        &seeds,
    )
    .expect("reference run");
    assert_eq!(noop.successes, 0);
    close(noop.interval_lower, 0.000);
    close(noop.interval_upper, 0.052);
}

#[test]
fn the_published_memory_latch_numbers_reproduce() {
    let seeds = memory::eval_seeds();
    let memory = probelab_tasks::runner::run_memory_latch(
        probelab_tasks::runner::TaskPolicy::ReferenceOptimal,
        &seeds,
    )
    .expect("reference run");
    assert_eq!(memory.successes, 70);
    close(memory.interval_lower, 0.948);
    close(memory.interval_upper, 1.000);

    let stateless = probelab_tasks::runner::run_memory_latch(
        probelab_tasks::runner::TaskPolicy::ReferenceBaseline,
        &seeds,
    )
    .expect("reference run");
    assert!(
        memory.interval_lower > stateless.interval_upper,
        "the published disjointness must reproduce: memory [{:.3}, {:.3}] vs stateless [{:.3}, {:.3}]",
        memory.interval_lower,
        memory.interval_upper,
        stateless.interval_lower,
        stateless.interval_upper,
    );
}
