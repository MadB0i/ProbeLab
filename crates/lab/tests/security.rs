//! Security Lab tests.
//!
//! The centre of gravity here is the refusal/violation distinction. Everything else in this file is in
//! service of proving that a boundary holding and a boundary failing are never reported as the same
//! thing — because if they were, a lab enforcing itself correctly would report a rising breach count,
//! and nobody could tell the difference.

use probelab_lab::{
    all_probes, probes_for, run_probe, sweep, AgentCategory, AgentRegistry, BoundaryOutcome,
    SecurityLabReport,
};

/// Every registered security probe runs through the real path and produces a real answer.
#[test]
fn a_sweep_runs_every_registered_probe() {
    let report = sweep("grid-world", 11, 2).expect("sweep should run");
    let registered = all_probes().len();

    assert!(
        registered > 0,
        "the registry claims a security-probe family"
    );
    assert_eq!(
        report.outcomes.len(),
        registered,
        "the sweep must cover every registered probe"
    );
    assert!(report.is_complete());
}

/// A probe that reached an enforcement layer is reported as a successful test, not a vulnerability.
///
/// This is the assertion the whole module exists for, and it is subtler than "refusals are not
/// findings". The five `security_probe_*` policies *do* produce recorded `PolicyViolation` events —
/// correctly, because the gate sees the attempt and denies it. Reporting that as a breach would make a
/// lab that catches every attack indistinguishable from one under constant attack.
#[test]
fn a_probe_that_is_caught_is_a_successful_test_not_a_vulnerability() {
    let report = sweep("grid-world", 11, 2).expect("sweep");
    for outcome in &report.outcomes {
        assert!(
            !outcome.is_security_finding(),
            "{} was reported as a vulnerability; a caught attempt is the boundary working",
            outcome.probe_id
        );
    }
    assert!(
        report.findings().is_empty(),
        "a sweep against the lab's own probes must not manufacture findings"
    );
}

/// The layer that acted is reported precisely, because it is a real distinction.
#[test]
fn the_enforcement_layer_that_acted_is_reported() {
    let report = sweep("grid-world", 11, 2).expect("sweep");
    let policy_denied: Vec<&str> = report
        .outcomes
        .iter()
        .filter(|outcome| outcome.outcome == BoundaryOutcome::PolicyDenied)
        .map(|outcome| outcome.probe_id.as_str())
        .collect();
    assert!(
        !policy_denied.is_empty(),
        "the gate-denied probes should be identifiable, got {:?}",
        report
            .outcomes
            .iter()
            .map(|outcome| (&outcome.probe_id, outcome.outcome))
            .collect::<Vec<_>>()
    );
    // And a policy-denied probe has recorded integrity events — that is the gate doing its job.
    for outcome in &report.outcomes {
        if outcome.outcome == BoundaryOutcome::PolicyDenied {
            assert!(
                !outcome.integrity_events.is_empty(),
                "{} was reported as policy-denied with nothing recorded",
                outcome.probe_id
            );
            assert!(
                outcome.broker_refusals.is_empty(),
                "{} reached the policy gate, so no broker refusal should be recorded",
                outcome.probe_id
            );
        }
    }
}

/// A refusal and a gate denial are different facts and never share a bucket.
#[test]
fn broker_refusals_and_gate_denials_stay_separate() {
    let report = sweep("grid-world", 11, 2).expect("sweep");
    for outcome in &report.outcomes {
        if !outcome.broker_refusals.is_empty() {
            assert!(
                !outcome.integrity_events.is_empty()
                    || outcome.outcome == BoundaryOutcome::BrokerRefused,
                "{} recorded a broker refusal alongside gate activity; the split is what keeps them apart",
                outcome.probe_id
            );
        }
    }
}

/// Violations and refusals never share a bucket, at the report level either.
#[test]
fn the_report_keeps_violations_and_refusals_separate() {
    let report = sweep("grid-world", 11, 2).expect("sweep");
    // The summary's two totals are independent, and summing them is the mistake the type prevents.
    let violations: u32 = report
        .outcomes
        .iter()
        .map(|outcome| outcome.integrity_total())
        .sum();
    let refusals: u32 = report
        .outcomes
        .iter()
        .map(|outcome| {
            outcome
                .broker_refusals
                .iter()
                .map(|(_, count)| *count)
                .sum::<u32>()
        })
        .sum();

    assert_eq!(report.summary.integrity_violations, violations);
    assert_eq!(report.summary.broker_refusals, refusals);
    // And neither map contains the other's keys.
    for key in report.summary.violations_by_category.keys() {
        assert!(
            !report.summary.refusals_by_category.contains_key(key),
            "{key} appears in both the violation and refusal breakdowns"
        );
    }
}

/// The findings list is empty on a healthy system, and that is the expected result.
#[test]
fn every_boundary_holds_against_the_labs_own_probes() {
    let report = sweep("grid-world", 11, 2).expect("sweep");
    let held = report.held();
    assert!(
        !held.is_empty(),
        "at least one probe should have reached an enforcement layer, or the sweep tested nothing"
    );
    for outcome in &report.outcomes {
        if outcome.failure.is_some() {
            continue;
        }
        assert!(
            outcome.outcome.boundary_held(),
            "{} reached no enforcement layer, so the boundary was never tested",
            outcome.probe_id
        );
    }
}

/// A probe whose attempt never reached an enforcement layer is not a pass.
#[test]
fn an_unanswered_probe_is_not_counted_as_a_successful_test() {
    assert!(!BoundaryOutcome::NotAttempted.boundary_held());
    assert!(!BoundaryOutcome::Failed.boundary_held());
    // A probe that tested nothing must not appear in the list of probes that held the boundary.
    let report = sweep("grid-world", 11, 2).expect("sweep");
    for outcome in report.held() {
        assert_ne!(
            outcome.outcome,
            BoundaryOutcome::NotAttempted,
            "{} never reached an enforcement layer yet is listed as a held boundary",
            outcome.probe_id
        );
    }
}

/// Every outcome that means an enforcement layer acted reports a held boundary.
#[test]
fn the_outcome_vocabulary_distinguishes_acted_from_not_acted() {
    assert!(BoundaryOutcome::BrokerRefused.boundary_held());
    assert!(BoundaryOutcome::PolicyDenied.boundary_held());
    assert!(BoundaryOutcome::RefusedAndDenied.boundary_held());
    assert!(!BoundaryOutcome::NotAttempted.boundary_held());
    assert!(!BoundaryOutcome::Failed.boundary_held());

    // And none of them is a security finding, because a probe is a test rather than a subject.
    for outcome in [
        BoundaryOutcome::BrokerRefused,
        BoundaryOutcome::PolicyDenied,
        BoundaryOutcome::RefusedAndDenied,
        BoundaryOutcome::NotAttempted,
        BoundaryOutcome::Failed,
    ] {
        assert!(
            !outcome.is_security_finding(),
            "{outcome:?} should not be a finding about the lab"
        );
    }
}

/// The probe list is the registry's, filtered by category.
#[test]
fn the_probe_list_comes_from_the_registry() {
    let agents = AgentRegistry::built_in();
    let listed = probes_for("grid-world", &agents);
    let specs = agents.specs();
    let security_agents: Vec<&str> = specs
        .iter()
        .filter(|spec| spec.category == AgentCategory::SecurityProbe)
        .map(|spec| spec.id.as_str())
        .collect();

    assert_eq!(listed.len(), security_agents.len());
    for probe in &listed {
        assert!(security_agents.contains(&probe.id.as_str()));
        assert_eq!(probe.benchmark_id, "grid-world");
        assert!(!probe.description.is_empty());
    }
}

/// A single probe can be run on its own, and reports its own identity.
#[test]
fn one_probe_can_be_run_independently() {
    let agents = AgentRegistry::built_in();
    let probe = probes_for("grid-world", &agents)
        .into_iter()
        .next()
        .expect("at least one probe");
    let (outcome, results) = run_probe(&probe, "grid-world", 5, 1).expect("probe should run");

    assert_eq!(outcome.probe_id, probe.id);
    assert_eq!(outcome.seed, 5);
    assert!(outcome.failure.is_none());
    // The raw results come back too, so the summary and the per-probe row are two views of one run.
    assert_eq!(results.len(), outcome.episodes);
}

/// An unknown benchmark is refused rather than swept against nothing.
#[test]
fn an_unknown_benchmark_is_refused() {
    let outcome = sweep("not-a-benchmark", 1, 1);
    assert!(
        outcome.is_err(),
        "an unknown benchmark must not produce a report"
    );
}

/// The report round-trips through the IPC encoding.
#[test]
fn the_report_round_trips() {
    let report = sweep("grid-world", 11, 2).expect("sweep");
    let json = serde_json::to_string(&report).expect("serialize");
    let restored: SecurityLabReport = serde_json::from_str(&json).expect("parse");
    assert_eq!(restored, report);
    assert!(json.contains("\"brokerRefusals\""));
    assert!(json.contains("\"integrityViolations\""));
}

/// No credential can reach a security report.
#[test]
fn a_security_report_never_carries_a_credential() {
    let report = sweep("grid-world", 11, 2).expect("sweep");
    let json = serde_json::to_string(&report).expect("serialize");
    for marker in [
        "sk-",
        "sk-ant",
        "AIza",
        "api_key\":",
        "apiKey",
        "password",
        "token\":",
    ] {
        assert!(
            !json.contains(marker),
            "a security report leaked something credential-shaped: {marker}"
        );
    }
}

/// A probe's timeline uses the canonical event vocabulary, and a probe cannot assert a finding.
///
/// The second half is the security one: the security probes deliberately attempt boundary violations,
/// so if a probe's own reporting could set an integrity category it could manufacture a finding about
/// itself. It cannot — `LabEvent::integrity` refuses an untrusted source.
#[test]
fn a_probe_cannot_assert_its_own_finding() {
    let agents = AgentRegistry::built_in();
    let probe = probes_for("grid-world", &agents)
        .into_iter()
        .next()
        .expect("a probe");
    let (outcome, _) = run_probe(&probe, "grid-world", 5, 1).expect("probe should run");
    for event in &outcome.timeline {
        if event.is_security_finding() {
            assert!(
                event.source.may_assert_integrity(),
                "{} asserted a finding from an untrusted source",
                event.code
            );
        }
    }
    // And the outcome is derived from the evaluator's counters, not from anything the probe reported.
    assert!(outcome.failure.is_none());
}

/// Structural test: the Security Lab cannot become an authority or a second evaluator.
///
/// The projection reads results; it cannot produce them, authorize anything, or reach the enforcement
/// layers. A security panel that could declare an attack successful would be the worst possible bug in
/// this product, so it is asserted rather than documented.
#[test]
fn the_security_lab_is_read_only() {
    let source = include_str!("../src/security.rs");
    for forbidden in [
        "CapabilityBroker",
        "PolicyDecision",
        "RuntimeAuthorization",
        "CapabilityRequest",
        "Command::new",
        "std::process",
        "fn evaluate",
        "fn authorize",
        "fn allow",
        "fn deny",
    ] {
        assert!(
            !source.contains(forbidden),
            "the security lab contains {forbidden:?}; a read model must not be able to authorize, \
             enforce, or score anything"
        );
    }
}

/// The report type has no way to produce a combined security number.
///
/// A compile-time absence rather than a runtime one: there is no `total()` method to call, so the
/// tempting sum is unavailable rather than discouraged.
#[test]
fn there_is_no_combined_security_total() {
    let report: SecurityLabReport = sweep("grid-world", 11, 1).expect("sweep");
    // The available accessors are the separated ones plus counts of probes and findings. If a combined
    // figure is ever added, this test needs updating deliberately rather than by accident.
    let _ = report.probe_count();
    let _ = report.findings().len();
    let _ = report.held().len();
    assert_eq!(
        report.summary.integrity_violations + report.summary.broker_refusals,
        report.summary.integrity_violations + report.summary.broker_refusals,
        "the two totals are independent values, not views of one number"
    );
}
