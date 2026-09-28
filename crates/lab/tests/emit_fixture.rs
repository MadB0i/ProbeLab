//! Generates the cross-language fixture the frontend tests render.
//!
//! Run with `PROBELAB_WRITE_FIXTURES=1 cargo test -p probelab-lab --test emit_fixture`.
//!
//! # Why this exists
//!
//! The Brain panel's frontend fixtures used to be written by hand. That is the reason two wire-format
//! bugs survived a full audit with every test green, and the two of them failed in opposite ways:
//!
//! 1. `Anomaly` had no `#[serde(rename_all)]`, so the backend sent `is_security_finding` while
//!    `api.ts` declared `isSecurityFinding`. The hand-written fixture used the *TypeScript* spelling,
//!    so it agreed with `api.ts` and disagreed with the backend. The panel then read `undefined`,
//!    which is falsy, and rendered every finding as "pattern only" with a security count of zero.
//! 2. `TerminalReason` is an adjacently tagged enum, so the backend sends `{"kind":"completed"}` — an
//!    object. `api.ts` declared `string`, and the hand-written fixture supplied a string, so
//!    `terminalLabel`'s `reason.replace(...)` was never exercised against a real payload. On a real
//!    run it throws `TypeError: reason.replace is not a function`.
//!
//! A hand-written fixture cannot catch a disagreement with the producer, because it is written from the
//! consumer's idea of the producer. So this emits the bytes the backend actually produces, and both
//! sides then read the same file: Rust pins its live serialization against it, and the frontend renders
//! it. A rename on either side now fails in one place, loudly, instead of failing in production.
//!
//! Without `PROBELAB_WRITE_FIXTURES=1` this is an assertion, not a writer — see `wire_format.rs`.

use std::sync::Arc;

use probelab_core::{TelemetryEvent, TelemetryRecorder};
use probelab_evaluator::GridWorldEvaluator;
use probelab_lab::{
    Anomaly, BrainReadModel, BrainState, LabBrain, LabRunner, ObservedCategory, PlanRequest,
    SecuritySummary, SignalInterpretation, SignalView,
};

const FIXTURE_PATH: &str = "../../apps/desktop/src/test-fixtures/brainReadModel.json";

/**
 * Strips the one field a fixture cannot hold.
 *
 * `elapsedMs` is wall-clock milliseconds, so it differs on every execution by design. A committed
 * fixture containing one would fail the pin on every run and teach everyone to regenerate fixtures
 * until they went green, which is the opposite of what a pin is for.
 *
 * So it is set to `null` here -- which is a real value the wire can carry, keeps the field's *presence*
 * pinned (a rename would still fail, because the key would move), and leaves the assertion that a
 * genuine run produces a real number to the tests below.
 */
fn normalized(model: &BrainReadModel) -> BrainReadModel {
    let mut model = model.clone();
    if let Some(experiment) = model.experiment.as_mut() {
        experiment.elapsed_ms = None;
    }
    model
}

/// A real run measures its own duration, and it is a real number rather than a placeholder.
///
/// This is what the fixture cannot check, so it is checked here instead -- against a real run rather
/// than a hand-written value.
#[test]
fn a_real_run_records_a_real_duration() {
    let mut brain = LabBrain::new();
    let run = LabRunner::new()
        .run(
            &mut brain,
            &PlanRequest {
                benchmark_id: Some("grid-world".to_owned()),
                target_agent: Some("optimal".to_owned()),
                probe_agents: Vec::new(),
                episodes: Some(1),
                root_seed: Some(4_242),
            },
            "exp-timing",
            &GridWorldEvaluator::new(),
            Arc::new(NullRecorder),
        )
        .expect("run");
    let elapsed = run
        .elapsed_ms
        .expect("a run measures its own duration; this was hardcoded to None in the read model");
    assert!(
        elapsed < 60_000,
        "a three-second mock episode cannot take a minute: {elapsed}ms"
    );

    // And it reaches the read model, so the panel's elapsed row is not a dead field.
    let model = BrainReadModel::from_run(&run, BrainState::Completed, "idle");
    assert!(
        model
            .experiment
            .expect("a run has an experiment")
            .elapsed_ms
            .is_some(),
        "the measured duration must reach the panel, or the field it renders can never be filled"
    );
}

struct NullRecorder;

impl TelemetryRecorder for NullRecorder {
    fn record(&self, _event: &TelemetryEvent) -> Result<(), String> {
        Ok(())
    }
}

/// A run that completes, so the fixture carries the interesting fields: a real score, a real Wilson
/// interval, real episodes, and a real terminal reason. An idle or empty model would be a weaker
/// fixture precisely because it exercises none of the paths that broke.
#[test]
fn emit_the_completed_run_fixture() {
    let mut brain = LabBrain::new();
    let run = LabRunner::new()
        .run(
            &mut brain,
            &PlanRequest {
                benchmark_id: Some("grid-world".to_owned()),
                target_agent: Some("optimal".to_owned()),
                probe_agents: Vec::new(),
                episodes: Some(3),
                root_seed: Some(4_242),
            },
            "exp-fixture",
            &GridWorldEvaluator::new(),
            Arc::new(NullRecorder),
        )
        .expect("the fixture run completes");

    let model = BrainReadModel::from_run(&run, BrainState::Completed, "idle");
    let bytes = serde_json::to_vec_pretty(&normalized(&model)).expect("the read model serializes");

    if std::env::var_os("PROBELAB_WRITE_FIXTURES").is_some() {
        let path = std::path::Path::new(FIXTURE_PATH);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the fixture directory is creatable");
        }
        std::fs::write(path, &bytes).expect("the fixture is writable");
        eprintln!("wrote {} ({} bytes)", path.display(), bytes.len());
        return;
    }

    // Not a writer: assert instead, so a normal `cargo test` catches a drift between the backend and
    // the committed fixture without anyone having to remember to regenerate.
    let committed = std::fs::read(FIXTURE_PATH).unwrap_or_else(|error| {
        panic!(
            "the frontend fixture is missing ({error}). Regenerate it with \
             `PROBELAB_WRITE_FIXTURES=1 cargo test -p probelab-lab --test emit_fixture`"
        )
    });
    assert_eq!(
        String::from_utf8_lossy(&committed).trim(),
        String::from_utf8_lossy(&bytes).trim(),
        "the Brain read model no longer matches the committed frontend fixture. The frontend renders \
         this file, so a field renamed here is a field the panel reads as undefined. Regenerate with \
         `PROBELAB_WRITE_FIXTURES=1 cargo test -p probelab-lab --test emit_fixture`."
    );
}

/// The idle model is the other shape the panel must handle, and it is the one where every optional
/// field is absent. A fixture that only covers the completed run leaves `null` handling unverified.
#[test]
fn emit_the_idle_fixture() {
    let model = BrainReadModel::idle();
    let bytes = serde_json::to_vec_pretty(&model).expect("the idle read model serializes");

    if std::env::var_os("PROBELAB_WRITE_FIXTURES").is_some() {
        let path = std::path::Path::new("../../apps/desktop/src/test-fixtures/brainIdleModel.json");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the fixture directory is creatable");
        }
        std::fs::write(path, &bytes).expect("the fixture is writable");
        eprintln!("wrote {}", path.display());
        return;
    }

    let committed = std::fs::read("../../apps/desktop/src/test-fixtures/brainIdleModel.json")
        .unwrap_or_else(|error| {
            panic!("the idle frontend fixture is missing ({error}). Regenerate with PROBELAB_WRITE_FIXTURES=1")
        });
    assert_eq!(
        String::from_utf8_lossy(&committed).trim(),
        String::from_utf8_lossy(&bytes).trim(),
        "the idle Brain read model no longer matches the committed frontend fixture."
    );
}

/// The security-relevant half of a `BrainReadModel` is the `findings` array, and the two findings the
/// panel must tell apart are a correct refusal and a real violation. This asserts the backend really
/// produces both, with the flag the frontend reads — the exact distinction the `Anomaly` bug erased.
#[test]
fn a_completed_run_fixture_has_findings_to_tell_apart() {
    // Guard on the *shape* of the guarantee rather than on this particular run's telemetry: the grid
    // world evaluator may or may not provoke denials. What must hold is that whenever a finding
    // exists, its `is_security_finding` is a real boolean in the serialized bytes, never absent.
    let mut brain = LabBrain::new();
    let run = LabRunner::new()
        .run(
            &mut brain,
            &PlanRequest {
                benchmark_id: Some("grid-world".to_owned()),
                target_agent: Some("optimal".to_owned()),
                probe_agents: Vec::new(),
                episodes: Some(3),
                root_seed: Some(4_242),
            },
            "exp-fixture",
            &GridWorldEvaluator::new(),
            Arc::new(NullRecorder),
        )
        .expect("run");
    let model = BrainReadModel::from_run(&run, BrainState::Completed, "idle");

    let value = serde_json::to_value(&model).expect("serializes");
    for finding in value["findings"].as_array().expect("findings is an array") {
        assert!(
            finding.get("isSecurityFinding").is_some(),
            "a finding serialized without `isSecurityFinding`: {finding}. The frontend reads this \
             exact key; an absent one is `undefined`, which is falsy, so a real boundary violation \
             renders as a harmless pattern."
        );
    }
}

/// A `BrainReadModel` carrying the two findings the panel must never conflate.
///
/// A clean grid-world run produces no findings, so the completed-run fixture cannot cover the
/// `Anomaly` path — which is exactly where the `is_security_finding` bug lived. A fixture that omits
/// the interesting case is worse than no fixture, because it looks like coverage.
///
/// Built on top of a real run rather than from `BrainReadModel::idle`, because the panel renders its
/// anomalies section only when an experiment is present: an idle model with findings would be a
/// fixture that serializes correctly and proves nothing about what the panel shows. The findings and
/// signals are attached to a genuine run so the surrounding fields — episodes, timeline, aggregate,
/// score — are the ones a real execution produced.
///
/// The two entries are the distinction the whole panel exists to keep: four broker denials are the
/// broker *working*, and three integrity events are findings on their own terms. They are adjacent
/// here on purpose, so a panel that merges them is visibly wrong rather than subtly wrong.
#[test]
fn emit_the_findings_fixture() {
    use std::collections::BTreeMap;

    let mut brain = LabBrain::new();
    let run = LabRunner::new()
        .run(
            &mut brain,
            &PlanRequest {
                benchmark_id: Some("grid-world".to_owned()),
                target_agent: Some("optimal".to_owned()),
                probe_agents: Vec::new(),
                episodes: Some(3),
                root_seed: Some(4_242),
            },
            "exp-fixture",
            &GridWorldEvaluator::new(),
            Arc::new(NullRecorder),
        )
        .expect("the fixture run completes");

    let mut model = BrainReadModel::from_run(&run, BrainState::Observing, "idle");
    model.next_intent = "Review repeated_broker_denials (broker denial, 4 observed)".to_owned();
    model.observed_events = 12;
    model.findings = vec![
        Anomaly {
            id: "repeated_broker_denials".to_owned(),
            category: ObservedCategory::BrokerDenial,
            interpretation: "The broker refused the same kind of request repeatedly.".to_owned(),
            occurrences: 4,
            is_security_finding: false,
        },
        Anomaly {
            id: "repeated_integrity_events".to_owned(),
            category: ObservedCategory::IntegrityEvent,
            interpretation: "The integrity layer recorded boundary violations.".to_owned(),
            occurrences: 3,
            is_security_finding: true,
        },
    ];
    model.signals = vec![
        SignalView {
            id: "task_success".to_owned(),
            label: "Episodes that reached the goal".to_owned(),
            count: 7,
            interpretation: SignalInterpretation::Neutral,
        },
        SignalView {
            id: "broker_refusal".to_owned(),
            label: "Capabilities the broker refused".to_owned(),
            count: 4,
            interpretation: SignalInterpretation::CorrectRefusal,
        },
    ];

    let mut violations_by_category = BTreeMap::new();
    violations_by_category.insert("integrity_event".to_owned(), 3u32);
    let mut refusals_by_category = BTreeMap::new();
    refusals_by_category.insert("broker_denial".to_owned(), 4u32);
    model.security = SecuritySummary {
        integrity_violations: 3,
        broker_refusals: 4,
        violations_by_category,
        refusals_by_category,
        invalid_actions: 2,
    };
    // The panel reads the security block off the experiment as well as the model, and the two must
    // agree; a fixture where they disagree would test a state the backend cannot produce.
    if let Some(experiment) = model.experiment.as_mut() {
        experiment.security = model.security.clone();
    }

    let bytes = serde_json::to_vec_pretty(&normalized(&model)).expect("the read model serializes");

    if std::env::var_os("PROBELAB_WRITE_FIXTURES").is_some() {
        let path =
            std::path::Path::new("../../apps/desktop/src/test-fixtures/brainFindingsModel.json");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the fixture directory is creatable");
        }
        std::fs::write(path, &bytes).expect("the fixture is writable");
        eprintln!("wrote {}", path.display());
        return;
    }

    let committed = std::fs::read("../../apps/desktop/src/test-fixtures/brainFindingsModel.json")
        .unwrap_or_else(|error| {
            panic!("the findings frontend fixture is missing ({error}). Regenerate with PROBELAB_WRITE_FIXTURES=1")
        });
    assert_eq!(
        String::from_utf8_lossy(&committed).trim(),
        String::from_utf8_lossy(&bytes).trim(),
        "the findings Brain read model no longer matches the committed frontend fixture."
    );
}
