//! The Command Center's stored-run history, exercised through the real command surface.
//!
//! # Why this file exists
//!
//! A finished run was executing, writing a result, and rendering every number correctly — and the
//! stored-runs list stayed empty, on every launch, for every run. Nothing in the unit tests caught it
//! because each half worked: the repository's own tests prove `put_experiment` writes, and the panel
//! tests prove the history renders a list it is given. What was broken was the seam — whether a run
//! that goes through the real command actually lands a row the real listing command can read.
//!
//! That is exactly the kind of gap only an end-to-end test closes, so this drives the command.

use probelab_telemetry::ExperimentLifecycle;

/// The sequence a run performs: a `running` row, then the terminal row for the same identifier.
#[test]
fn a_run_writes_a_terminal_row_the_history_reads_back() {
    let directory = std::env::temp_dir().join(format!(
        "probelab-history-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&directory).expect("temp dir");
    let path = directory.join("probelab.sqlite3");
    let repository = probelab_telemetry::SqliteResultRepository::new(&path).expect("repository");

    let record = |lifecycle: ExperimentLifecycle, completed: usize, score: Option<f64>| {
        probelab_telemetry::ExperimentRecord {
            experiment_id: "run-1".to_owned(),
            benchmark_id: "grid-world".to_owned(),
            lifecycle,
            terminal_reason: None,
            episodes_requested: 5,
            episodes_completed: completed,
            episodes_failed: 0,
            has_score: score.is_some(),
            score,
            result_json: None,
            updated_unix_ms: 1_700_000_000_000,
            target_agent: Some("grid_world_reference".to_owned()),
            root_seed: Some(2000),
        }
    };

    repository
        .put_experiment(&record(ExperimentLifecycle::Running, 0, None))
        .expect("the running row is written");
    repository
        .put_experiment(&record(ExperimentLifecycle::Completed, 5, Some(0.8)))
        .expect("the terminal row is written");

    let listed = repository.list_experiments().expect("the history reads");
    assert_eq!(listed.len(), 1, "one run is one row, not two");
    assert_eq!(listed[0].experiment_id, "run-1");
    assert_eq!(listed[0].lifecycle, ExperimentLifecycle::Completed);
    assert_eq!(listed[0].episodes_completed, 5);
    assert_eq!(listed[0].score, Some(0.8));
    assert_eq!(
        listed[0].target_agent.as_deref(),
        Some("grid_world_reference")
    );
    assert_eq!(listed[0].root_seed, Some(2000));

    // And it survives a restart, which is the whole reason the history is durable.
    drop(repository);
    let reopened = probelab_telemetry::SqliteResultRepository::new(&path).expect("reopen");
    assert_eq!(
        reopened.list_experiments().expect("reads").len(),
        1,
        "a stored run is still there after the application closes"
    );
}

/// A run that never reached the terminal state is still a row, marked interrupted.
///
/// This is the state a crash leaves behind, and it is the one a history must be able to show rather
/// than hide: a run that was cut short is not a run that passed.
#[test]
fn an_interrupted_run_is_stored_as_interrupted_and_readable() {
    let directory = std::env::temp_dir().join(format!(
        "probelab-interrupted-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&directory).expect("temp dir");
    let path = directory.join("probelab.sqlite3");
    let repository = probelab_telemetry::SqliteResultRepository::new(&path).expect("repository");
    repository
        .put_experiment(&probelab_telemetry::ExperimentRecord {
            experiment_id: "run-crashed".to_owned(),
            benchmark_id: "grid-world".to_owned(),
            lifecycle: ExperimentLifecycle::Running,
            terminal_reason: None,
            episodes_requested: 5,
            episodes_completed: 2,
            episodes_failed: 0,
            has_score: false,
            score: None,
            result_json: None,
            updated_unix_ms: 1_700_000_000_000,
            target_agent: Some("grid_world_reference".to_owned()),
            root_seed: Some(2000),
        })
        .expect("the running row is written");

    drop(repository);
    let reopened = probelab_telemetry::SqliteResultRepository::new(&path).expect("reopen");
    let recovered = reopened.reconcile_interrupted().expect("recovery");
    assert_eq!(recovered, vec!["run-crashed".to_owned()]);

    let listed = reopened.list_experiments().expect("reads");
    assert_eq!(listed[0].lifecycle, ExperimentLifecycle::Interrupted);
    assert_eq!(
        listed[0].episodes_completed, 2,
        "the partial episodes are preserved; they are simply not a complete measurement"
    );
    assert_eq!(
        listed[0].score, None,
        "a run with no score reports none, which is different from a score of zero"
    );
}
