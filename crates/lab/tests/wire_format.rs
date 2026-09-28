//! Wire-format stability tests.
//!
//! # What this covers, and why it is its own file
//!
//! A defect once shipped here where the serialized form of `SecurityProbe` was `security_probe`.
//! Nothing was broken at run time: the producer and the consumer agreed, the tests passed, and the
//! only evidence was a hand-inspected payload that did not look like the Rust. It was found by a
//! person reading JSON, which is not a repeatable process.
//!
//! So this file pins the *spelling* of the wire format, not just the ability to round-trip it. A
//! round-trip test passes when both ends drift together, which is the one case that does not matter.
//! What matters is a recorded payload, a stored row, and a TypeScript interface all continuing to
//! mean the same thing after a refactor -- so the assertions below compare against literal JSON with
//! the field names and enum spellings written out by hand.
//!
//! Three properties are covered:
//!
//! 1. **Field names.** Every struct that crosses a boundary is compared key-for-key against a literal.
//! 2. **Enum spelling.** Every enum that crosses a boundary must be `snake_case` on the wire. A
//!    derived enum with no `rename_all` silently ships PascalCase, which is exactly the defect above.
//! 3. **Absence stays absent.** An optional field with no value must read back as `None`, never as a
//!    zero. A missing score that decodes to `0.0` is a measurement the product did not make.
//!
//! # Why unknown values must fail
//!
//! Deserialization is strict here on purpose. A stored row or a recorded payload containing an
//! unrecognised variant is an error, not a default. Defaulting would convert a corrupted record into
//! a plausible-looking state, and the whole point of storing a lifecycle was to make that
//! impossible.
//!
//! # The one convention worth knowing before editing this file
//!
//! Two naming rules are in deliberate use, and this sweep is where a new type has to pick one:
//!
//! * **Structs the frontend reads are `camelCase`.** `BrainReadModel`, `ExperimentAggregate`,
//!   `SecurityProbeReport`, `Replay`, and the stored `ExperimentRecord` all cross into hand-written
//!   TypeScript, so their fields are renamed to match the interface. `episodesCompleted`, not
//!   `episodes_completed`.
//! * **Everything else is `snake_case`.** `LabEvent` is written to storage and read by Rust, and
//!   every enum in the system is `snake_case` regardless of what it sits in.
//!
//! `CapabilityPermission` is the third case and the oldest: it uses dotted names -- `filesystem.read`,
//! `process.spawn` -- because benchmark and policy documents have referred to permissions by those
//! strings since before any of this, and a stored policy that stopped parsing would fail closed on
//! every capability at once. It is asserted here so the contract is a test rather than a comment.
//!
//! Nothing in this file asserts a convention is *right*; it asserts what the format currently is, so
//! that changing it is a deliberate act with a failing test attached rather than a silent edit.

// A `Replay` frame is `camelCase` because the frontend reads it, and a `LabEvent` is `snake_case`
// because storage does. Both are asserted below, in the tests named for the type.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use probelab_core::{
    CapabilityDecision, CapabilityDenial, CapabilityPermission, IntegrityEventKind,
};
use probelab_lab::{
    BoundaryOutcome, BrainState, EventSource, ExperimentAggregate, LabEvent, ObservedCategory,
    Replay, ReplayCapability, ReplayFrame, ReplayKind, ReplaySource, SecurityProbeReport, Severity,
    TerminalReason, WilsonInterval,
};
use probelab_telemetry::{ExperimentLifecycle, ExperimentRecord};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

/// Asserts that a value serializes to exactly `expected` and back to an equal value.
///
/// The comparison is on `Value`, not on a string, so key order is irrelevant -- but every key and
/// every value must match, including the enum spellings.
fn assert_wire<T>(expected: Value, label: &str) -> T
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let decoded: T = serde_json::from_value(expected.clone()).unwrap_or_else(|error| {
        panic!("{label}: the wire form no longer decodes: {error}\n{expected}")
    });
    let written = serde_json::to_value(&decoded)
        .unwrap_or_else(|error| panic!("{label}: the decoded value no longer serializes: {error}"));
    assert_eq!(
        written, expected,
        "{label}: the wire format changed. If this is intentional, every stored row, recorded \
         payload, and frontend interface that reads this field changes with it."
    );
    decoded
}

/// Every enum that crosses a process or storage boundary, with a sample value and its exact wire
/// spelling.
///
/// A single test over all of them, because the property is the same for each and the value of
/// asserting it once is that a new enum added later has an obvious place to be added to it. The
/// sample is paired with its value explicitly rather than matched on the type name, because
/// `CapabilityPermission` has three variants in this list and matching on the type would silently
/// test only the first of them.
fn boundary_enums() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "IntegrityEventKind::PolicyViolation",
            serde_json::to_value(IntegrityEventKind::PolicyViolation).expect("serialize"),
            json!("policy_violation"),
        ),
        (
            "CapabilityPermission::FilesystemRead",
            serde_json::to_value(CapabilityPermission::FilesystemRead).expect("serialize"),
            json!("filesystem.read"),
        ),
        (
            "CapabilityPermission::ProcessSpawn",
            serde_json::to_value(CapabilityPermission::ProcessSpawn).expect("serialize"),
            json!("process.spawn"),
        ),
        (
            "CapabilityPermission::EnvironmentInspect",
            serde_json::to_value(CapabilityPermission::EnvironmentInspect).expect("serialize"),
            json!("environment.inspect"),
        ),
        (
            "EventSource::Telemetry",
            serde_json::to_value(EventSource::Telemetry).expect("serialize"),
            json!("telemetry"),
        ),
        (
            "Severity::Critical",
            serde_json::to_value(Severity::Critical).expect("serialize"),
            json!("critical"),
        ),
        (
            "BrainState::Observing",
            serde_json::to_value(BrainState::Observing).expect("serialize"),
            json!("observing"),
        ),
        (
            "ObservedCategory::ProtocolFailure",
            serde_json::to_value(ObservedCategory::ProtocolFailure).expect("serialize"),
            json!("protocol_failure"),
        ),
        (
            "BoundaryOutcome::BrokerRefused",
            serde_json::to_value(BoundaryOutcome::BrokerRefused).expect("serialize"),
            json!("broker_refused"),
        ),
        (
            "TerminalReason::Completed",
            serde_json::to_value(TerminalReason::Completed).expect("serialize"),
            json!({"kind": "completed"}),
        ),
    ]
}

#[test]
fn every_boundary_enum_is_snake_case_on_the_wire() {
    for (label, encoded, expected) in boundary_enums() {
        // The assertion is against the literal, not on a round trip: two enums that both drifted
        // the same way would still round-trip, and the stored data would still be unreadable.
        assert_eq!(
            encoded, expected,
            "{label} is not spelled as the wire format claims"
        );
    }
}

#[test]
fn the_experiment_lifecycle_is_snake_case_on_the_wire() {
    for (lifecycle, expected) in [
        (ExperimentLifecycle::Planned, "planned"),
        (ExperimentLifecycle::Running, "running"),
        (ExperimentLifecycle::Completed, "completed"),
        (ExperimentLifecycle::Failed, "failed"),
        (ExperimentLifecycle::Interrupted, "interrupted"),
    ] {
        assert_eq!(
            serde_json::to_value(lifecycle).expect("serialize"),
            json!(expected)
        );
        assert_eq!(
            serde_json::from_value::<ExperimentLifecycle>(json!(expected)).expect("parse"),
            lifecycle,
            "{expected} no longer reads back as the state it names"
        );
    }
}

/// A stored lifecycle the product does not recognise must not decode to something plausible.
#[test]
fn an_unknown_lifecycle_is_rejected_rather_than_defaulted() {
    for value in [
        json!("canceled"),
        json!("RUNNING"),
        json!(""),
        json!(7),
        json!(null),
    ] {
        assert!(
            serde_json::from_value::<ExperimentLifecycle>(value.clone()).is_err(),
            "{value} decoded as a lifecycle; a corrupted row must not become a state"
        );
    }
}

#[test]
fn a_capability_decision_keeps_its_allow_and_deny_shapes() {
    assert_eq!(
        serde_json::to_value(CapabilityDecision::Allow).expect("serialize"),
        json!({"decision": "allow"})
    );
    // The denial reason is the evidence, so the tagged form has to survive; a flattened string would
    // discard which invariant was refused.
    let denied = serde_json::to_value(CapabilityDecision::Deny(CapabilityDenial {
        permission: CapabilityPermission::EnvironmentInspect,
        integrity_event: IntegrityEventKind::PolicyViolation,
        reason: "environment inspection is not granted to this capability".to_owned(),
    }))
    .expect("serialize");
    let json_text = denied.to_string();
    assert!(
        json_text.contains("environment.inspect"),
        "the denied permission must stay on the wire: {json_text}"
    );
    assert!(
        json_text.contains("environment inspection is not granted"),
        "the denial reason must stay on the wire: {json_text}"
    );
    let back: CapabilityDecision = serde_json::from_value(denied).expect("parse");
    assert!(matches!(back, CapabilityDecision::Deny(_)));
}

/// A full event, with every optional claim present.
#[test]
fn a_lab_event_keeps_its_field_names() {
    let event: LabEvent = assert_wire(
        json!({
            "sequence": 7,
            "code": "broker.denied",
            "source": "broker",
            "severity": "warning",
            "detail": "filesystem.read refused",
            "integrity": "policy_violation",
            "category": "broker_denial",
            "recorded_at": {"secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 123_000_000}
        }),
        "LabEvent",
    );
    assert_eq!(event.sequence, 7);
    assert_eq!(event.integrity, Some(IntegrityEventKind::PolicyViolation));
}

/// The dangerous direction: an event with no integrity claim must not acquire one in transit.
#[test]
fn an_event_without_an_integrity_claim_keeps_it_absent() {
    let event: LabEvent = assert_wire(
        json!({
            "sequence": 1,
            "code": "agent.step",
            "source": "model",
            "severity": "info",
            "detail": "chose move_up",
            "integrity": null,
            "category": null,
            "recorded_at": {"secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 0}
        }),
        "LabEvent without claims",
    );
    assert_eq!(
        event.integrity, None,
        "a model event must not gain an integrity claim"
    );
    assert_eq!(event.category, None);
}

#[test]
fn an_unknown_integrity_kind_is_rejected() {
    // A payload claiming an integrity category the taxonomy does not define would otherwise be read
    // as a real finding. Failing closed is the only safe reading.
    let error = serde_json::from_value::<IntegrityEventKind>(json!("definitely_a_finding"))
        .expect_err("an unknown integrity kind must not decode");
    assert!(
        error.to_string().contains("definitely_a_finding"),
        "{error}"
    );
}

/// The three Wilson bounds, named.
///
/// `clippy::excessive_precision` is allowed here on purpose. These are full-precision `f64` literals
/// and the excess digits *are* the assertion: the whole point is that the value which drifted by one
/// ULP through JSON is written down in full and compared bit-for-bit. Trimming the literal to what
/// Rust's parser considers shortest-round-trip would quietly destroy the test, and the lint is
/// otherwise right everywhere else in this workspace.
#[allow(clippy::excessive_precision)]
mod precision {
    pub const POINT: f64 = 0.09453120573423071;
    pub const LOWER: f64 = 0.025210109702200352;
    pub const UPPER: f64 = 0.7005679588332878;
    pub const MEAN_REWARD: f64 = 0.30000000000000004;
}

#[test]
fn the_aggregate_keeps_its_field_names_and_exact_floats() {
    let aggregate: ExperimentAggregate = assert_wire(
        json!({
            "episodes": 2,
            "successes": 1,
            "successRate": 0.5,
            "successInterval": {
                "point": precision::POINT,
                "lower": precision::LOWER,
                "upper": precision::UPPER,
                "confidence": 0.95
            },
            "meanReward": precision::MEAN_REWARD,
            "meanTurns": 4.5,
            "statusCounts": {"success": 1, "turn_limit": 1},
            "seedPool": "grid-world/eval"
        }),
        "ExperimentAggregate",
    );
    // Bit equality on the values that exposed a ULP drift, for the same reason float_precision.rs
    // exists: an epsilon comparison would hide precisely the defect being guarded.
    assert_eq!(
        aggregate.success_interval.point.to_bits(),
        precision::POINT.to_bits(),
        "the point estimate did not survive the wire exactly"
    );
    assert_eq!(
        aggregate.success_interval.lower.to_bits(),
        precision::LOWER.to_bits(),
        "the lower bound did not survive the wire exactly"
    );
    assert_eq!(
        aggregate.success_interval.upper.to_bits(),
        precision::UPPER.to_bits(),
        "the upper bound did not survive the wire exactly"
    );
    assert_eq!(
        aggregate.mean_reward.to_bits(),
        precision::MEAN_REWARD.to_bits(),
        "the mean reward did not survive the wire exactly"
    );
    assert_eq!(aggregate.success_interval.confidence, 0.95);
}

/// A plan that pinned single seeds has no pool, and that must stay `None`.
#[test]
fn an_absent_seed_pool_does_not_become_a_name() {
    let aggregate: ExperimentAggregate = assert_wire(
        json!({
            "episodes": 0,
            "successes": 0,
            "successRate": 0.0,
            "successInterval": {"point": 0.0, "lower": 0.0, "upper": 1.0, "confidence": 0.95},
            "meanReward": 0.0,
            "meanTurns": 0.0,
            "statusCounts": {},
            "seedPool": null
        }),
        "ExperimentAggregate without a seed pool",
    );
    assert_eq!(aggregate.seed_pool, None);
    assert_eq!(
        aggregate.status_counts,
        BTreeMap::new(),
        "an empty status map must not gain invented entries"
    );
}

#[test]
fn a_security_probe_report_keeps_refusals_and_violations_apart() {
    let report: SecurityProbeReport = assert_wire(
        json!({
            "probeId": "sandbox-escape",
            "probeName": "Sandbox escape",
            "benchmarkId": "sandbox-process",
            "seed": 99,
            "outcome": "broker_refused",
            "integrityEvents": [],
            "brokerRefusals": [["sandbox_violation", 3]],
            "episodes": 1,
            "timeline": [],
            "failure": null
        }),
        "SecurityProbeReport",
    );
    assert!(
        report.integrity_events.is_empty(),
        "a caught probe is not a finding; the empty list must survive the wire"
    );
    assert_eq!(report.outcome, BoundaryOutcome::BrokerRefused);
}

/// A probe that failed must keep its reason; a dropped reason reads as a clean run.
#[test]
fn a_failed_probe_keeps_its_reason() {
    let report: SecurityProbeReport = assert_wire(
        json!({
            "probeId": "sandbox-escape",
            "probeName": "Sandbox escape",
            "benchmarkId": "sandbox-process",
            "seed": 99,
            "outcome": "failed",
            "integrityEvents": [],
            "brokerRefusals": [],
            "episodes": 0,
            "timeline": [],
            "failure": "the broker was unreachable"
        }),
        "SecurityProbeReport with a failure",
    );
    assert_eq!(
        report.failure.as_deref(),
        Some("the broker was unreachable")
    );
}

#[test]
fn a_replay_frame_keeps_an_absent_digest_absent() {
    let frame: ReplayFrame = assert_wire(
        json!({"index": 3, "source": "arena_ply", "label": "ply 3", "digest": null}),
        "ReplayFrame",
    );
    assert_eq!(
        frame.digest, None,
        "a fabricated digest would let a corrupted replay look intact"
    );
    let with_digest: ReplayFrame = assert_wire(
        json!({"index": 4, "source": "arena_ply", "label": "ply 4", "digest": "sha256:abc123"}),
        "ReplayFrame with a digest",
    );
    assert_eq!(with_digest.digest.as_deref(), Some("sha256:abc123"));
    assert_eq!(with_digest.source, ReplaySource::ArenaPly);
}

/// The replay kind decides what a recording can do. If it were misread, the panel would offer
/// stepping through an experiment event stream as though it were a match.
#[test]
fn a_replay_kind_keeps_its_capability_semantics() {
    for (kind, expected) in [
        (ReplayKind::ArenaMatch, "arena_match"),
        (ReplayKind::Experiment, "experiment"),
    ] {
        assert_eq!(
            serde_json::to_value(kind).expect("serialize"),
            json!(expected)
        );
    }
    let replay: Replay = assert_wire(
        json!({
            "kind": "arena_match",
            "recordingId": "match-1",
            "capability": "full",
            "frames": [{"index": 0, "source": "start", "label": "start", "digest": null}],
            "digest": "sha256:abc",
            "limitations": []
        }),
        "Replay",
    );
    assert!(
        replay.capability.can_step(),
        "a recording with every ply must be steppable"
    );
    assert_eq!(replay.capability, ReplayCapability::Full);
    assert_eq!(replay.capability.label(), "RE-EXECUTABLE");
}

/// The experiment lifecycle record, exactly as it is stored.
#[test]
fn a_stored_experiment_record_keeps_its_columns() {
    let record: ExperimentRecord = assert_wire(
        json!({
            "experimentId": "exp-1",
            "benchmarkId": "grid-world",
            "lifecycle": "completed",
            "terminalReason": "completed",
            "episodesRequested": 3,
            "episodesCompleted": 3,
            "episodesFailed": 0,
            "hasScore": true,
            "score": 0.6666666666666666,
            "resultJson": null,
            "updatedUnixMs": 1_700_000_000_000i64,
            "targetAgent": "optimal",
            "rootSeed": 4242
        }),
        "ExperimentRecord",
    );
    assert_eq!(record.lifecycle, ExperimentLifecycle::Completed);
    assert_eq!(record.score, Some(2.0 / 3.0));
    assert!(record.has_score);
}

/// A run that completed no episode stores no score. The boolean is what distinguishes "no score"
/// from "a score of zero", so both fields are asserted together.
#[test]
fn a_record_without_a_score_keeps_both_the_flag_and_the_absence() {
    let record: ExperimentRecord = assert_wire(
        json!({
            "experimentId": "exp-2",
            "benchmarkId": "grid-world",
            "lifecycle": "interrupted",
            "terminalReason": "containment_failure",
            "episodesRequested": 5,
            "episodesCompleted": 0,
            "episodesFailed": 5,
            "hasScore": false,
            "score": null,
            "resultJson": null,
            "updatedUnixMs": 1_700_000_000_001i64,
            "targetAgent": "optimal",
            "rootSeed": 4242
        }),
        "ExperimentRecord without a score",
    );
    assert!(!record.has_score);
    assert_eq!(
        record.score, None,
        "no completed episode means no score; it must not read back as 0.0"
    );
    assert_eq!(record.lifecycle, ExperimentLifecycle::Interrupted);
}

/// A terminal reason with a payload keeps the payload: which containment invariant failed is the
/// difference between a diagnosable failure and a shrug.
#[test]
fn a_terminal_reason_keeps_its_failure_detail() {
    let reason: TerminalReason = assert_wire(
        json!({"kind": "containment_failure", "detail": {"kind": "verification_failed"}}),
        "TerminalReason::ContainmentFailure",
    );
    assert!(matches!(reason, TerminalReason::ContainmentFailure { .. }));
    let text = serde_json::to_string(&reason).expect("serialize");
    assert!(text.contains("containment_failure"), "{text}");
    assert!(text.contains("verification_failed"), "{text}");
}

/// The interval type is shared with the Harness, so its wire form has to match there too.
#[test]
fn the_wilson_interval_keeps_its_field_names() {
    let interval: WilsonInterval = assert_wire(
        json!({"point": 0.5, "lower": 0.2126, "upper": 0.7874, "confidence": 0.95}),
        "WilsonInterval",
    );
    assert_eq!(interval.point, 0.5);
}

/// An end-to-end pass over the types a real run actually persists.
///
/// The literal tests above pin spellings against hand-written JSON, which risks testing a fiction if
/// the real shape ever diverges. This one takes the output of a real experiment, puts it through
/// JSON, and reads it back -- so a field added to a real type without updating the literals above
/// fails here instead of drifting unnoticed.
#[test]
fn a_real_run_survives_the_wire() {
    let mut brain = probelab_lab::LabBrain::new();
    let run = probelab_lab::LabRunner::new()
        .run(
            &mut brain,
            &probelab_lab::PlanRequest {
                benchmark_id: Some("grid-world".to_owned()),
                target_agent: Some("optimal".to_owned()),
                probe_agents: Vec::new(),
                episodes: Some(2),
                root_seed: Some(4_242),
            },
            "exp-wire",
            &probelab_evaluator::GridWorldEvaluator::new(),
            Arc::new(probelab_engine::arena::NullRecorder),
        )
        .expect("run");

    // `ExperimentRun` itself is deliberately not `Serialize`: it holds live handles and is a
    // process-local value, not a wire type. What crosses into storage is the record, the timeline,
    // and the results, so those are what this asserts.
    let timeline_text = serde_json::to_string(&run.timeline).expect("serialize the timeline");
    let back_timeline: probelab_lab::LabTimeline =
        serde_json::from_str(&timeline_text).expect("parse the timeline");
    assert_eq!(back_timeline, run.timeline);

    // Every timeline event keeps its integrity claim exactly. This is the assertion that would fail
    // if a self-report were ever promoted into a finding during a round trip.
    for (before, after) in run
        .timeline
        .events()
        .iter()
        .zip(back_timeline.events().iter())
    {
        assert_eq!(before.integrity, after.integrity);
        assert_eq!(before.code, after.code);
        assert_eq!(before.source, after.source);
    }

    // Per-episode results, with every score bit-identical.
    let results_text = serde_json::to_string(&run.results).expect("serialize the results");
    let back_results: Vec<probelab_core::TestResult> =
        serde_json::from_str(&results_text).expect("parse the results");
    assert_eq!(back_results.len(), run.results.len());
    for (before, after) in run.results.iter().zip(back_results.iter()) {
        assert_eq!(before, after);
    }

    // The aggregate, floats included.
    let aggregate_text = serde_json::to_string(&run.aggregate).expect("serialize the aggregate");
    let back_aggregate: ExperimentAggregate =
        serde_json::from_str(&aggregate_text).expect("parse the aggregate");
    assert_eq!(back_aggregate, run.aggregate);
    assert_eq!(
        run.mean_success_fraction().map(f64::to_bits),
        back_aggregate_success_bits(&back_aggregate),
        "the recorded success fraction drifted through the wire"
    );
}

/// The bits of the aggregate's success rate, for comparison against a run's own summary.
fn back_aggregate_success_bits(aggregate: &ExperimentAggregate) -> Option<u64> {
    if aggregate.episodes == 0 {
        None
    } else {
        Some(aggregate.success_rate.to_bits())
    }
}

/// A time value in the past and one at the epoch both survive, because a run recorded at boot and
/// one recorded a moment later must both round-trip.
#[test]
fn timestamps_survive_the_wire() {
    for (secs, nanos) in [(0u64, 0u32), (1_700_000_000, 999_999_999)] {
        let when = UNIX_EPOCH + Duration::new(secs, nanos);
        let encoded = serde_json::to_value(when).expect("serialize");
        let back: SystemTime = serde_json::from_value(encoded).expect("parse");
        assert_eq!(back, when);
    }
}

/// Every enum string that crosses into stored data or the IPC boundary is derived, not retyped.
///
/// The `wire_name` methods exist because the same enum was rendered four different ways by four
/// different call sites -- `Debug` gives `TurnLimit`, serde gives `turn_limit`, one site lowercased
/// `Debug` and produced `turnlimit`, and a stored `terminal_reason` column held a Debug rendering no
/// parser could read. The bug class is not "a wrong string"; it is "a string that is retyped in each
/// consumer", which means no consumer can be proven to agree with the wire format.
///
/// This test is the proof. It asserts each `wire_name` equals what serde actually emits, so the two
/// cannot diverge, and a rename that moves one fails here rather than at a stored row.
#[test]
fn every_wire_name_matches_what_serde_actually_writes() {
    use probelab_core::CompletionStatus;
    use probelab_lab::ObservedCategory;

    /// The string serde writes for a unit variant.
    fn serialized<T: serde::Serialize>(value: T) -> String {
        serde_json::to_value(value)
            .expect("serialize")
            .as_str()
            .expect("a unit variant serializes as a string")
            .to_owned()
    }

    for status in [
        CompletionStatus::Success,
        CompletionStatus::Failure,
        CompletionStatus::TurnLimit,
        CompletionStatus::ModelProtocolFailure,
    ] {
        assert_eq!(
            status.wire_name(),
            serialized(status),
            "{status:?}: wire_name disagrees with the serialized form"
        );
    }

    for category in [
        ObservedCategory::ProtocolFailure,
        ObservedCategory::BrokerDenial,
        ObservedCategory::PolicyDenial,
        ObservedCategory::IntegrityEvent,
        ObservedCategory::ContainmentEvent,
        ObservedCategory::ProviderFailure,
        ObservedCategory::BudgetExhausted,
        ObservedCategory::Ordinary,
    ] {
        assert_eq!(
            category.wire_name(),
            serialized(category),
            "{category:?}: wire_name disagrees with the serialized form"
        );
    }
}

/// The stored `terminal_reason` string is derived from the serde representation, not from `Debug`.
///
/// `TerminalReason` has payload variants, so serde writes an object. `to_wire_json` flattens it back to
/// the `"<tag> <fields>"` shape the stored column holds, which means the *tag* and the *field values* are
/// both serde's. A `Debug` rendering stored `"containmentfailure { kind: verificationfailed }"`, which is
/// a Rust expression embedded in a database column.
#[test]
fn the_stored_terminal_reason_is_the_serde_representation() {
    use probelab_core::ContainmentFailureKind;
    use probelab_lab::TerminalReason;

    for reason in [
        TerminalReason::Completed,
        TerminalReason::Cancelled,
        TerminalReason::Timeout { budget_seconds: 30 },
        TerminalReason::ProtocolFailure { consecutive: 3 },
        TerminalReason::ContainmentFailure {
            kind: ContainmentFailureKind::VerificationFailed,
        },
    ] {
        let stored = reason.to_wire_json();
        assert!(!stored.is_empty(), "{reason:?} rendered as an empty string");
        // The payload is serde's own JSON, so braces are expected. What must not appear is Rust syntax:
        // `Debug` renders `Timeout { budget_seconds: 30 }` with a space after the colon and no quotes
        // around a number, and that is exactly what used to be written into the column.
        assert!(
            !stored.contains(": "),
            "{reason:?} stored Rust field syntax: {stored}"
        );
        assert!(
            !stored.contains("kind:"),
            "{reason:?} stored the tag wrapper: {stored}"
        );
        // The tag is the serde tag, read out of the serialized object rather than assumed. The enum uses
        // `serde(tag = "kind", content = "detail")`, so the tag is under `kind` and the payload beside it.
        let value = serde_json::to_value(&reason).expect("serialize");
        let object = value
            .as_object()
            .expect("serde(tag, content) writes an object");
        let expected_tag = object
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .expect("serde(tag, content) puts the tag under `kind`");
        assert!(
            stored.starts_with(expected_tag),
            "{reason:?} stored as {stored:?}, which does not begin with the serde tag {expected_tag:?}"
        );
    }
}

/// A finding the Brain can report reaches the frontend with the names the frontend declares.
///
/// Added because `Anomaly` was the one type on the Brain panel's read path with no `rename_all`, so it
/// serialized `is_security_finding` while `apps/desktop/src/api.ts` declared `isSecurityFinding`. The
/// consequence was not a crash: an absent property is `undefined` on arrival, which is falsy, so every
/// anomaly rendered as "pattern only" and the security sub-count was permanently zero. A boundary
/// finding would have been invisible in the panel whose whole job is to show it.
///
/// Nothing caught it, which is the part worth recording. Two tests were green throughout:
/// `crates/lab/tests/view.rs` round-trips the read model with the *same* decoder that wrote it, so both
/// ends agreed on snake_case; and the React fixture in `BrainPanel.test.tsx` *typed* `isSecurityFinding`
/// rather than deriving it from the wire, so the frontend was green against a shape the backend never
/// sent. A round-trip test passes when both ends drift together, which is the one case that does not
/// matter. This test pins literals instead, for the type the drift guard had no entry for.
///
/// `category` is asserted as the serde spelling of the enum (`broker_denial`) while the *field names*
/// around it are camelCase. That asymmetry is correct and worth seeing explicitly: `rename_all` on a
/// struct does not reach into a nested enum's own `rename_all`.
#[test]
fn a_brain_finding_keeps_the_field_names_the_frontend_declares() {
    use probelab_lab::{
        Anomaly, BrainReadModel, ObservedCategory, SignalInterpretation, SignalView,
    };

    let finding: BrainReadModel = assert_wire(
        json!({
            "brainState": "observing",
            "nextIntent": "Review repeated_broker_denials (broker denial, 4 observed)",
            "experiment": null,
            "observedEvents": 12,
            "findings": [
                {
                    "id": "repeated_broker_denials",
                    "category": "broker_denial",
                    "interpretation": "The broker refused the same kind of request repeatedly.",
                    "occurrences": 4,
                    "isSecurityFinding": false
                },
                {
                    "id": "repeated_integrity_events",
                    "category": "integrity_event",
                    "interpretation": "The integrity layer recorded boundary violations.",
                    "occurrences": 3,
                    "isSecurityFinding": true
                }
            ],
            "signals": [
                {
                    "id": "task_success",
                    "label": "Episodes that reached the goal",
                    "count": 7,
                    "interpretation": "neutral"
                }
            ],
            "security": {
                "integrityViolations": 3,
                "brokerRefusals": 4,
                "violationsByCategory": { "integrity_event": 3 },
                "refusalsByCategory": { "broker_denial": 4 },
                "invalidActions": 2
            }
        }),
        "BrainReadModel with findings",
    );

    // The flag must survive the round trip as a *value*, not just as a key. Asserting the key exists
    // would pass even if the boolean were dropped, and `false` is the value that renders as "pattern
    // only" -- so the false case is the one worth pinning alongside the true one.
    let broker = finding
        .findings
        .iter()
        .find(|anomaly| anomaly.id == "repeated_broker_denials")
        .expect("the broker finding survives");
    assert!(
        !broker.is_security_finding,
        "a broker refusal is a correct refusal and must never be a finding"
    );
    assert_eq!(broker.category, ObservedCategory::BrokerDenial);

    let integrity = finding
        .findings
        .iter()
        .find(|anomaly| anomaly.id == "repeated_integrity_events")
        .expect("the integrity finding survives");
    assert!(
        integrity.is_security_finding,
        "an integrity event is a finding on its own terms"
    );

    // A bare `Anomaly` too, so a future change to the struct alone still fails here.
    let bare: Anomaly = assert_wire(
        json!({
            "id": "containment_events",
            "category": "containment_event",
            "interpretation": "Containment reported an event.",
            "occurrences": 1,
            "isSecurityFinding": false
        }),
        "Anomaly",
    );
    assert!(!bare.is_security_finding);

    // The signal interpretation round-trips as its own enum spelling, so the panel's tone mapping
    // (`violation` / `correct_refusal` / `protocol_shortfall` / `operational` / `neutral`) can branch
    // on a value the backend really emits.
    let signal: &SignalView = &finding.signals[0];
    assert_eq!(signal.interpretation, SignalInterpretation::Neutral);
}
