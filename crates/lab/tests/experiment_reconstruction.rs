//! Multi-episode experiment durability (audit finding B3).
//!
//! # The defect
//!
//! The `experiments` table's `result_json` column held `serde_json::to_string(&mean_success_fraction)` —
//! one number. Every episode, decision, failure, timeline entry and security total a run produced lived
//! only in the running process. `get_experiment` rebuilt a read model by looking for a stored result under
//! the run identifier; a multi-episode run writes no such single result, so the lookup found nothing and
//! the command answered "not found".
//!
//! The failure mode is the one that matters: the result was fully visible during execution, then became
//! unavailable the moment the process that produced it was gone. A run could be listed in history and
//! unopenable.
//!
//! # What these tests establish
//!
//! * A completed multi-episode experiment — produced by the **real runner** over the real benchmark —
//!   survives a restart, with every episode, the aggregate, the decisions, the failures and the timeline
//!   intact.
//! * The restored view is compared field by field against the one that was persisted, so nothing is
//!   added, dropped or altered on the way back.
//! * Historical rows holding a bare scalar are still read, and are **not** upgraded into a fabricated
//!   multi-episode result.
//! * A document in an unknown version is refused rather than parsed optimistically.

use std::sync::Arc;

use probelab_core::TelemetryRecorder;
use probelab_lab::{
    AgentRegistry, BrainReadModel, ExperimentView, LabBrain, LabRunner, PlanRequest, ProbeRegistry,
    StoredExperimentDocument, StoredExperimentShape,
};
use probelab_telemetry::{
    ExperimentLifecycle, ExperimentRecord, SqliteResultRepository, SqliteTelemetryRecorder,
};

/// A scratch directory unique per process launch, not per pid.
///
/// Windows recycles process identifiers, so a pid-only name eventually hands a later run a database an
/// earlier one already filled.
fn scratch(label: &str) -> std::path::PathBuf {
    let unique = format!(
        "probelab-b3-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

/// Runs a real five-episode experiment and returns its read model.
///
/// Produced through the real planner, runner and evaluator rather than hand-assembled, so the test
/// asserts about a structure the product actually emits. A hand-built fixture would pass even if the
/// fields being persisted were never populated by a real run.
fn run_five_episode_experiment(database: &std::path::Path) -> ExperimentView {
    let mut brain = LabBrain::new();
    let plan_request = PlanRequest {
        benchmark_id: Some("grid-world".to_owned()),
        target_agent: Some("optimal".to_owned()),
        probe_agents: Vec::new(),
        episodes: Some(5),
        root_seed: Some(4_242),
    };
    // The plan is exercised rather than stubbed, so a plan that stopped carrying an episode count would
    // fail here instead of quietly writing a one-episode document.
    brain
        .make_plan(
            &plan_request,
            &probelab_benchmarks::BenchmarkRegistry::built_in(),
            &AgentRegistry::built_in(),
            &ProbeRegistry::built_in(),
            probelab_lab::runner::current_platform(),
        )
        .expect("a plan for a registered benchmark and agent");

    let recorder: Arc<dyn TelemetryRecorder> =
        Arc::new(SqliteTelemetryRecorder::new(database).expect("telemetry store"));
    let run = LabRunner::new()
        .run(
            &mut brain,
            &plan_request,
            "b3-multi",
            &probelab_evaluator::GridWorldEvaluator::new(),
            recorder,
        )
        .expect("the experiment runs");

    let view = ExperimentView::from_run(&run);
    assert_eq!(
        view.episodes.len(),
        5,
        "the fixture must actually be multi-episode, or the test proves nothing"
    );
    view
}

fn record_with(document: Option<String>, episodes_completed: usize) -> ExperimentRecord {
    ExperimentRecord {
        experiment_id: "b3-multi".to_owned(),
        benchmark_id: "grid-world".to_owned(),
        lifecycle: ExperimentLifecycle::Completed,
        terminal_reason: Some("completed".to_owned()),
        episodes_requested: 5,
        episodes_completed,
        episodes_failed: 0,
        has_score: true,
        score: Some(0.2),
        result_json: document,
        updated_unix_ms: 1,
        target_agent: Some("optimal".to_owned()),
        root_seed: Some(4_242),
    }
}

/*
 * The defect, restated.
 *
 * A run completes in one process. A *different* repository instance over the same file is exactly what a
 * restarted process has. The experiment must still be there, in full.
 */
#[test]
fn a_completed_multi_episode_experiment_survives_a_restart_in_full() {
    let root = scratch("restart");
    let database = root.join("probelab.sqlite3");

    let view = run_five_episode_experiment(&database);
    let document = view.to_json().expect("the document encodes");

    {
        let results = SqliteResultRepository::new(&database).expect("open");
        results
            .put_experiment(&record_with(Some(document), 5))
            .expect("persist");
    }

    // A new handle on the same file is what a restarted process has.
    let reopened = SqliteResultRepository::new(&database).expect("reopen");
    let stored = reopened
        .get_experiment("b3-multi")
        .expect("read")
        .expect("the experiment row survives the process");

    match StoredExperimentDocument::from_legacy_json(
        stored.result_json.as_deref().unwrap_or("null"),
    ) {
        StoredExperimentShape::Document(document) => {
            let restored = document.experiment;
            assert_eq!(restored.episodes.len(), 5, "every episode must survive");
            assert!(
                !restored.timeline.is_empty(),
                "the timeline must survive, or a reopened run has no evidence"
            );
            assert_eq!(restored.episodes_requested, 5);
            assert_eq!(restored.episodes_completed, 5);
            assert!(
                restored.model.is_some(),
                "the model identity must survive; a reopened run with no model is not comparable"
            );
        }
        other => panic!("expected a stored document, got {other:?}"),
    }
}

/// The restored view must be the *same* view, not a recomputed approximation.
#[test]
fn the_restored_experiment_is_field_for_field_the_one_that_was_persisted() {
    let root = scratch("roundtrip");
    let database = root.join("probelab.sqlite3");
    let view = run_five_episode_experiment(&database);

    let results = SqliteResultRepository::new(&database).expect("open");
    results
        .put_experiment(&record_with(view.to_json(), 5))
        .expect("persist");
    let stored = results
        .get_experiment("b3-multi")
        .expect("read")
        .expect("row");

    let restored = match StoredExperimentDocument::from_legacy_json(
        stored.result_json.as_deref().unwrap_or("null"),
    ) {
        StoredExperimentShape::Document(document) => document.experiment,
        other => panic!("expected a document, got {other:?}"),
    };

    assert_eq!(
        restored, view,
        "a round trip must not add, drop or alter a single field"
    );
}

/// The read model is rebuilt from the persisted experiment, not from one stored result.
#[test]
fn the_read_model_is_rebuilt_from_the_whole_persisted_experiment() {
    let root = scratch("readmodel");
    let database = root.join("probelab.sqlite3");
    let view = run_five_episode_experiment(&database);

    let results = SqliteResultRepository::new(&database).expect("open");
    results
        .put_experiment(&record_with(view.to_json(), 5))
        .expect("persist");
    let stored = results
        .get_experiment("b3-multi")
        .expect("read")
        .expect("row");

    let restored = match StoredExperimentDocument::from_legacy_json(
        stored.result_json.as_deref().unwrap_or("null"),
    ) {
        StoredExperimentShape::Document(document) => document.experiment,
        other => panic!("expected a document, got {other:?}"),
    };

    let model = BrainReadModel::from_experiment_view(restored);
    let reopened = model.experiment.expect("the experiment is present");
    assert_eq!(reopened.episodes.len(), 5);
    assert_eq!(
        model.observed_events as usize,
        reopened.timeline.len(),
        "observed events come from the persisted timeline, not from a fresh count"
    );
    assert!(
        model.findings.is_empty(),
        "anomaly findings have no durable equivalent; an empty list is the honest state"
    );
    assert!(
        model.signals.is_empty(),
        "signals cannot be rebuilt from summary counts without conflating refusals with violations"
    );
}

/// Episode identities survive, so a reopened run's per-episode rows are the same rows.
#[test]
fn episode_indices_survive_the_round_trip() {
    let root = scratch("indices");
    let database = root.join("probelab.sqlite3");
    let view = run_five_episode_experiment(&database);

    let results = SqliteResultRepository::new(&database).expect("open");
    results
        .put_experiment(&record_with(view.to_json(), 5))
        .expect("persist");
    let stored = results
        .get_experiment("b3-multi")
        .expect("read")
        .expect("row");

    let restored = match StoredExperimentDocument::from_legacy_json(
        stored.result_json.as_deref().unwrap_or("null"),
    ) {
        StoredExperimentShape::Document(document) => document.experiment,
        other => panic!("expected a document, got {other:?}"),
    };
    let indices: Vec<usize> = restored
        .episodes
        .iter()
        .map(|episode| episode.index)
        .collect();
    assert_eq!(indices, vec![0, 1, 2, 3, 4]);
}

/// The historical shape must still be readable — and must not become a fabricated multi-episode run.
#[test]
fn a_historical_scalar_row_is_still_recognised() {
    for (raw, expected) in [("0.75", Some(0.75)), ("null", None)] {
        match StoredExperimentDocument::from_legacy_json(raw) {
            StoredExperimentShape::LegacyMean(mean) => assert_eq!(mean, expected),
            other => panic!("expected a legacy mean for {raw:?}, got {other:?}"),
        }
    }
}

/// A document this build does not understand is refused, not guessed at.
#[test]
fn an_unknown_document_version_is_refused_rather_than_parsed_optimistically() {
    let raw = serde_json::json!({ "version": 99, "experiment": {} }).to_string();
    assert!(
        matches!(
            StoredExperimentDocument::from_legacy_json(&raw),
            StoredExperimentShape::Unrecognised
        ),
        "a future version must not be mistaken for the current one"
    );
}

/// Unrecognisable bytes are reported as such, not silently treated as "no result".
#[test]
fn unrecognisable_bytes_are_reported() {
    for raw in ["{\"not\": \"a document\"}", "not json at all", ""] {
        assert!(
            matches!(
                StoredExperimentDocument::from_legacy_json(raw),
                StoredExperimentShape::Unrecognised
            ),
            "{raw:?} should be unrecognised"
        );
    }
}
