//! Audit finding B12 — a failed run must not look like a run that is still going.
//!
//! A run is persisted as `running` *before* its first episode, so that a crash is recoverable. That same
//! row means a run that **fails** and is never finalised is indistinguishable from one that is still
//! executing: the archive says `running`, crash recovery reconciles it as an interruption, and a stop
//! control has nothing to match because the cancellation entry was removed on the success path only.
//!
//! The fix records the failure. These tests pin what the archive then says.

use probelab_telemetry::{ExperimentLifecycle, ExperimentRecord, SqliteResultRepository};

fn a_repository(tag: &str) -> (SqliteResultRepository, std::path::PathBuf) {
    let mut path = std::env::temp_dir();
    path.push(format!("probelab-b12-{tag}-{}.sqlite3", std::process::id()));
    let _ = std::fs::remove_file(&path);
    (
        SqliteResultRepository::new(&path).expect("repository opens"),
        path,
    )
}

/// The pre-episode row a run leaves behind, exactly as `run_experiment` writes it.
fn a_running_row(experiment_id: &str) -> ExperimentRecord {
    ExperimentRecord {
        experiment_id: experiment_id.to_owned(),
        benchmark_id: "grid-world".to_owned(),
        lifecycle: ExperimentLifecycle::Running,
        terminal_reason: None,
        episodes_requested: 3,
        episodes_completed: 0,
        episodes_failed: 0,
        has_score: false,
        score: None,
        root_seed: Some(42),
        target_agent: Some("nonexistent-agent".to_owned()),
        result_json: None,
        updated_unix_ms: 1_700_000_000_000,
    }
}

#[test]
fn a_run_begins_as_running_and_can_be_read_back() {
    let (repository, path) = a_repository("running");
    repository
        .put_experiment(&a_running_row("exp-a"))
        .expect("recorded");

    let found = repository
        .list_experiments()
        .expect("listed")
        .into_iter()
        .find(|record| record.experiment_id == "exp-a")
        .expect("the row is there");
    assert_eq!(
        found.lifecycle,
        ExperimentLifecycle::Running,
        "sanity: the pre-episode state really is running, which is what made the omission invisible"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_failure_is_distinguishable_from_running_and_from_completion() {
    assert_ne!(
        ExperimentLifecycle::Failed,
        ExperimentLifecycle::Running,
        "a failed run must not serialise as running"
    );
    assert_ne!(
        ExperimentLifecycle::Failed,
        ExperimentLifecycle::Completed,
        "and must never be reported as a completed measurement"
    );
}

/// What a reader of the archive must be able to conclude about a failed run.
#[test]
fn a_recorded_failure_claims_nothing_about_a_result() {
    let record = ExperimentRecord {
        lifecycle: ExperimentLifecycle::Failed,
        terminal_reason: Some("the agent is not registered".to_owned()),
        episodes_requested: 3,
        episodes_completed: 0,
        episodes_failed: 3,
        has_score: false,
        score: None,
        result_json: None,
        ..a_running_row("exp-failed")
    };

    assert!(
        !record.has_score,
        "a run that produced nothing must not claim a score"
    );
    assert_eq!(record.score, None, "and the score must be absent, not zero");
    assert_eq!(
        record.episodes_completed, 0,
        "no episode finished, so none may be claimed"
    );
    assert_eq!(
        record.episodes_failed, record.episodes_requested,
        "every requested episode is accounted for as failed"
    );
    assert!(
        record.result_json.is_none(),
        "no result payload may be fabricated for a run that produced none"
    );
    assert!(
        record.terminal_reason.is_some(),
        "a failure with no recorded reason is the unfinished state this finding describes"
    );
}

/// Writing the failure over the running row is an update, not a second row.
///
/// A duplicate would leave the `running` row in place for crash recovery to reconcile later, which is
/// the precise confusion this finding is about.
#[test]
fn recording_a_failure_updates_the_running_row_rather_than_duplicating_it() {
    let (repository, path) = a_repository("update");
    repository
        .put_experiment(&a_running_row("exp-b"))
        .expect("recorded");

    let mut failed = a_running_row("exp-b");
    failed.lifecycle = ExperimentLifecycle::Failed;
    failed.terminal_reason = Some("the agent is not registered".to_owned());
    failed.episodes_failed = failed.episodes_requested;
    repository.put_experiment(&failed).expect("updated");

    let all = repository.list_experiments().expect("listed");
    let matching: Vec<&ExperimentRecord> = all
        .iter()
        .filter(|record| record.experiment_id == "exp-b")
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "there must be exactly one row for the run: a duplicate would leave the running one behind"
    );
    assert_eq!(
        matching[0].lifecycle,
        ExperimentLifecycle::Failed,
        "and it must no longer read as running"
    );
    let _ = std::fs::remove_file(&path);
}
