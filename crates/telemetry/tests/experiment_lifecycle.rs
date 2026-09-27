//! Experiment lifecycle persistence and recovery tests.
//!
//! # The problem this solves
//!
//! A run that crashes mid-flight left a row saying `running` and nothing to correct it. On the next
//! start that row was still there, indistinguishable from a run in progress — so a partial measurement
//! could be reported as a completed one. That is the worst outcome available from a persistence layer:
//! silently wrong rather than loudly absent.
//!
//! The fix has one idea in it. A row saying `running` *while the application has just started* is
//! provably false, and correcting it needs no cooperation from the process that died. That transition
//! is the only one a dead process cannot make for itself, and it is why the lifecycle has an
//! `interrupted` state at all.
//!
//! # What is deliberately not here
//!
//! Resume. An interrupted experiment is preserved, not restarted: its episodes were executed under a
//! sandbox root, a process identity, and a provider session that no longer exist. Re-running them would
//! produce a second measurement appended to the first, which is worse than an honest gap.

use probelab_telemetry::{ExperimentLifecycle, ExperimentRecord, SqliteResultRepository};

fn temp_database(label: &str) -> std::path::PathBuf {
    let unique = format!(
        "probelab-lifecycle-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&root).expect("temp root");
    root.join("results.sqlite3")
}

fn record(
    id: &str,
    lifecycle: ExperimentLifecycle,
    completed: usize,
    failed: usize,
    has_score: bool,
) -> ExperimentRecord {
    ExperimentRecord {
        experiment_id: id.to_owned(),
        benchmark_id: "grid-world".to_owned(),
        lifecycle,
        terminal_reason: match lifecycle {
            ExperimentLifecycle::Completed => Some("completed".to_owned()),
            ExperimentLifecycle::Failed => Some("protocol_failure".to_owned()),
            ExperimentLifecycle::Interrupted => Some("process_terminated".to_owned()),
            _ => None,
        },
        episodes_requested: 5,
        episodes_completed: completed,
        episodes_failed: failed,
        has_score,
        score: has_score.then_some(0.8),
        result_json: None,
        updated_unix_ms: 1_700_000_000_000,
        // The plan's identity, which a comparison needs in order to hold anything constant.
        target_agent: Some("optimal".to_owned()),
        root_seed: Some(4_242),
    }
}

/// The lifecycle round-trips through its stored string, and refuses anything unknown.
#[test]
fn the_lifecycle_has_a_stable_stored_form() {
    for lifecycle in [
        ExperimentLifecycle::Planned,
        ExperimentLifecycle::Running,
        ExperimentLifecycle::Completed,
        ExperimentLifecycle::Failed,
        ExperimentLifecycle::Interrupted,
    ] {
        let stored = lifecycle.as_str();
        assert_eq!(
            ExperimentLifecycle::parse(stored).expect("should parse"),
            lifecycle,
            "{stored:?} did not round-trip"
        );
    }
    // An unknown value is an error, never a default. Defaulting would turn a corrupted row into a
    // plausible lifecycle, which is exactly what this mechanism exists to prevent.
    assert!(ExperimentLifecycle::parse("finished").is_err());
    assert!(ExperimentLifecycle::parse("").is_err());
}

/// Only `completed` is a complete measurement.
#[test]
fn only_completion_is_a_complete_measurement() {
    assert!(ExperimentLifecycle::Completed.is_a_complete_measurement());
    for lifecycle in [
        ExperimentLifecycle::Planned,
        ExperimentLifecycle::Running,
        ExperimentLifecycle::Failed,
        ExperimentLifecycle::Interrupted,
    ] {
        assert!(
            !lifecycle.is_a_complete_measurement(),
            "{lifecycle:?} must not be reportable as a finished measurement"
        );
    }
}

/// A record survives a write and a read.
#[test]
fn a_record_round_trips_through_the_database() {
    let path = temp_database("round-trip");
    let repository = SqliteResultRepository::new(&path).expect("repository");
    repository
        .put_experiment(&record("exp-1", ExperimentLifecycle::Completed, 5, 0, true))
        .expect("write");

    let stored = repository
        .get_experiment("exp-1")
        .expect("read")
        .expect("present");
    assert_eq!(stored.lifecycle, ExperimentLifecycle::Completed);
    assert_eq!(stored.episodes_completed, 5);
    assert!(stored.has_score);
    assert_eq!(stored.score, Some(0.8));
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// A run with no score stores the absence, not a zero.
#[test]
fn an_absent_score_is_stored_as_absent() {
    let path = temp_database("no-score");
    let repository = SqliteResultRepository::new(&path).expect("repository");
    repository
        .put_experiment(&record(
            "exp-empty",
            ExperimentLifecycle::Failed,
            0,
            5,
            false,
        ))
        .expect("write");

    let stored = repository
        .get_experiment("exp-empty")
        .expect("read")
        .expect("present");
    assert!(
        !stored.has_score,
        "a run with no completed episode has no score, and that must survive persistence"
    );
    assert_eq!(stored.score, None, "not zero");
    assert_eq!(stored.episodes_failed, 5);
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// The crash sequence: start, run some episodes, die, restart, and the run is interrupted.
#[test]
fn a_crashed_experiment_is_interrupted_rather_than_reported_as_completed() {
    let path = temp_database("crash");

    // The process starts and plans.
    {
        let repository = SqliteResultRepository::new(&path).expect("repository");
        repository
            .put_experiment(&record(
                "exp-crash",
                ExperimentLifecycle::Planned,
                0,
                0,
                false,
            ))
            .expect("plan");
    }

    // It starts running, completes two episodes, and the process dies. Nothing writes `interrupted`
    // here -- the process cannot, being dead.
    {
        let repository = SqliteResultRepository::new(&path).expect("repository");
        repository
            .put_experiment(&record(
                "exp-crash",
                ExperimentLifecycle::Running,
                2,
                0,
                false,
            ))
            .expect("running");
    }

    // The application restarts.
    {
        let repository = SqliteResultRepository::new(&path).expect("repository");
        let before = repository
            .get_experiment("exp-crash")
            .expect("read")
            .expect("present");
        assert_eq!(
            before.lifecycle,
            ExperimentLifecycle::Running,
            "before reconciliation the row still claims to be running"
        );

        let reconciled = repository.reconcile_interrupted().expect("reconcile");
        assert_eq!(
            reconciled,
            vec!["exp-crash".to_owned()],
            "the restart must report what it corrected rather than rewriting history silently"
        );

        let after = repository
            .get_experiment("exp-crash")
            .expect("read")
            .expect("present");
        assert_eq!(after.lifecycle, ExperimentLifecycle::Interrupted);
        assert!(
            !after.lifecycle.is_a_complete_measurement(),
            "a crashed run must never be reportable as a finished measurement"
        );
        // The partial evidence is preserved, not discarded.
        assert_eq!(
            after.episodes_completed, 2,
            "completed episodes are real evidence"
        );
    }

    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// A completed run is never rewritten to interrupted on a later start.
#[test]
fn reconciliation_never_rewrites_a_finished_run() {
    let path = temp_database("finished");
    let repository = SqliteResultRepository::new(&path).expect("repository");
    for lifecycle in [
        ExperimentLifecycle::Completed,
        ExperimentLifecycle::Failed,
        ExperimentLifecycle::Interrupted,
        ExperimentLifecycle::Planned,
    ] {
        repository
            .put_experiment(&record(
                &format!("exp-{lifecycle:?}"),
                lifecycle,
                1,
                0,
                true,
            ))
            .expect("write");
    }

    let reconciled = repository.reconcile_interrupted().expect("reconcile");
    assert!(
        reconciled.is_empty(),
        "only a running row may be transitioned, got {reconciled:?}"
    );
    for lifecycle in [
        ExperimentLifecycle::Completed,
        ExperimentLifecycle::Failed,
        ExperimentLifecycle::Interrupted,
        ExperimentLifecycle::Planned,
    ] {
        let stored = repository
            .get_experiment(format!("exp-{lifecycle:?}"))
            .expect("read")
            .expect("present");
        assert_eq!(
            stored.lifecycle, lifecycle,
            "{lifecycle:?} was rewritten by reconciliation"
        );
    }
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// Reconciliation is idempotent, so a second restart changes nothing.
#[test]
fn reconciliation_is_idempotent() {
    let path = temp_database("idempotent");
    {
        let repository = SqliteResultRepository::new(&path).expect("repository");
        repository
            .put_experiment(&record("exp-a", ExperimentLifecycle::Running, 1, 0, false))
            .expect("write");
        assert_eq!(repository.reconcile_interrupted().expect("first").len(), 1);
        assert!(
            repository
                .reconcile_interrupted()
                .expect("second")
                .is_empty(),
            "a second reconciliation must find nothing left to correct"
        );
    }
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// Results and lifecycle are independent: rewriting a lifecycle does not disturb a stored result.
///
/// This is the reason the two live in separate tables. Results carry an immutability trigger, and
/// putting mutable lifecycle state in the same table would have forced that trigger to be relaxed.
#[test]
fn lifecycle_updates_do_not_disturb_stored_results() {
    let path = temp_database("independent");
    let repository = SqliteResultRepository::new(&path).expect("repository");
    let result = probelab_core::TestResult {
        schema_version: 1,
        benchmark_id: "grid-world".to_owned(),
        benchmark_version: "2".to_owned(),
        model: probelab_core::ModelDescriptor {
            adapter: "test".to_owned(),
            strategy: "optimal".to_owned(),
            seed: 0,
            parameters: Default::default(),
        },
        seed: 7,
        capability: probelab_core::CapabilityResult {
            dimension: "navigation".to_owned(),
            task_success: true,
            total_reward: 10,
            turns_used: 5,
            invalid_action_count: 0,
            completion_status: probelab_core::CompletionStatus::Success,
            protocol_adherence: Default::default(),
        },
        integrity: Default::default(),
        telemetry_event_count: 5,
        replay_digest: "digest".to_owned(),
        reproducibility: Default::default(),
    };
    repository.store("run-1", &result).expect("store result");

    for lifecycle in [
        ExperimentLifecycle::Running,
        ExperimentLifecycle::Interrupted,
    ] {
        repository
            .put_experiment(&record("exp-x", lifecycle, 0, 0, false))
            .expect("lifecycle");
    }
    let stored = repository.get("run-1").expect("read").expect("present");
    assert_eq!(stored.replay_digest, "digest", "the result was disturbed");
    assert!(stored.capability.task_success);
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// The plan's identity round-trips, because a comparison cannot hold constant something it cannot
/// read.
#[test]
fn the_experiment_identity_is_stored_and_read_back() {
    let path = temp_database("identity");
    let repository = SqliteResultRepository::new(&path).expect("repository");
    let entry = record("exp-identity", ExperimentLifecycle::Completed, 2, 0, true);
    repository.put_experiment(&entry).expect("write");
    let stored = repository
        .get_experiment("exp-identity")
        .expect("read")
        .expect("present");
    assert_eq!(stored.target_agent.as_deref(), Some("optimal"));
    assert_eq!(stored.root_seed, Some(4_242));
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// A record written without an identity must keep reporting that absence.
///
/// Backfilling a seed would fabricate the exact property a comparison uses to decide what it may
/// compare, and `None` is the truthful answer for a row that predates the column.
#[test]
fn a_record_without_an_identity_keeps_reporting_the_absence() {
    let path = temp_database("no-identity");
    let repository = SqliteResultRepository::new(&path).expect("repository");
    let mut entry = record("exp-anon", ExperimentLifecycle::Completed, 1, 0, true);
    entry.target_agent = None;
    entry.root_seed = None;
    repository.put_experiment(&entry).expect("write");
    let stored = repository
        .get_experiment("exp-anon")
        .expect("read")
        .expect("present");
    assert_eq!(
        stored.target_agent, None,
        "a missing agent must not be invented"
    );
    assert_eq!(
        stored.root_seed, None,
        "a missing seed must not be invented"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// A `u64` seed above `i64::MAX` must survive storage unchanged.
///
/// SQLite has no unsigned integer, so the seed is stored as a bit pattern. A plain signed cast would
/// wrap it to a negative number, and a seed that reads back as a different value is precisely what
/// would let a comparison decide two runs were the same experiment when they were not.
#[test]
fn a_seed_above_the_signed_range_round_trips() {
    let path = temp_database("big-seed");
    let repository = SqliteResultRepository::new(&path).expect("repository");
    let mut entry = record("exp-big", ExperimentLifecycle::Completed, 1, 0, true);
    entry.root_seed = Some(u64::MAX);
    repository.put_experiment(&entry).expect("write");
    let stored = repository
        .get_experiment("exp-big")
        .expect("read")
        .expect("present");
    assert_eq!(
        stored.root_seed,
        Some(u64::MAX),
        "the seed was altered in storage"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// A database written before the identity columns existed must open, keep its rows, and report the
/// absence rather than failing to read.
///
/// This is the case a migration is actually for: not a new database, but an existing one.
#[test]
fn a_database_without_the_identity_columns_still_opens() {
    let path = temp_database("pre-identity");
    {
        // Create the older schema by hand, with the identity columns absent.
        let connection = rusqlite::Connection::open(&path).expect("open");
        connection
            .execute_batch(
                "CREATE TABLE experiments (
                    experiment_id TEXT PRIMARY KEY,
                    benchmark_id TEXT NOT NULL,
                    lifecycle TEXT NOT NULL,
                    terminal_reason TEXT,
                    episodes_requested INTEGER NOT NULL,
                    episodes_completed INTEGER NOT NULL DEFAULT 0,
                    episodes_failed INTEGER NOT NULL DEFAULT 0,
                    has_score INTEGER NOT NULL DEFAULT 0,
                    score REAL,
                    result_json TEXT,
                    updated_unix_ms INTEGER NOT NULL
                );
                INSERT INTO experiments VALUES
                    ('exp-old', 'grid-world', 'completed', 'completed', 5, 5, 0, 1, 0.8, NULL, 1);",
            )
            .expect("create the older schema");
    }

    // Opening migrates it, and the existing row survives.
    let repository = SqliteResultRepository::new(&path).expect("migrate");
    let stored = repository
        .get_experiment("exp-old")
        .expect("read")
        .expect("the pre-existing row must survive the migration");
    assert_eq!(stored.lifecycle, ExperimentLifecycle::Completed);
    assert_eq!(stored.episodes_completed, 5);
    assert_eq!(
        stored.target_agent, None,
        "the old row has no recorded agent"
    );
    assert_eq!(stored.root_seed, None, "the old row has no recorded seed");

    // And a new write against the migrated schema fills the columns in.
    repository
        .put_experiment(&record(
            "exp-new",
            ExperimentLifecycle::Completed,
            1,
            0,
            true,
        ))
        .expect("write after migration");
    let fresh = repository
        .get_experiment("exp-new")
        .expect("read")
        .expect("present");
    assert_eq!(fresh.root_seed, Some(4_242));
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// Listings are newest-first and include every state, so a panel can show an interrupted run.
#[test]
fn listings_include_every_state() {
    let path = temp_database("listing");
    let repository = SqliteResultRepository::new(&path).expect("repository");
    for (index, lifecycle) in [
        ExperimentLifecycle::Completed,
        ExperimentLifecycle::Running,
        ExperimentLifecycle::Interrupted,
    ]
    .into_iter()
    .enumerate()
    {
        let mut entry = record(&format!("exp-{index}"), lifecycle, 1, 0, true);
        entry.updated_unix_ms = 1_700_000_000_000 + i64::try_from(index).unwrap_or(0);
        repository.put_experiment(&entry).expect("write");
    }
    let listed = repository.list_experiments().expect("list");
    assert_eq!(listed.len(), 3);
    assert_eq!(
        listed[0].lifecycle,
        ExperimentLifecycle::Interrupted,
        "newest first"
    );
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}

/// A record round-trips through its wire representation.
#[test]
fn a_record_round_trips_through_serialization() {
    let original = record("exp-1", ExperimentLifecycle::Interrupted, 2, 3, false);
    let json = serde_json::to_string(&original).expect("serialize");
    let restored: ExperimentRecord = serde_json::from_str(&json).expect("parse");
    assert_eq!(restored, original);
    assert!(json.contains("\"interrupted\""));
    assert!(json.contains("\"hasScore\":false"));
}

/// No credential can reach a persisted lifecycle record.
#[test]
fn a_lifecycle_record_never_carries_a_credential() {
    let path = temp_database("no-secret");
    let repository = SqliteResultRepository::new(&path).expect("repository");
    repository
        .put_experiment(&record("exp", ExperimentLifecycle::Running, 0, 0, false))
        .expect("write");
    let listed = repository.list_experiments().expect("list");
    let json = serde_json::to_string(&listed).expect("serialize");
    for marker in ["sk-", "sk-ant", "AIza", "api_key\":", "password"] {
        assert!(!json.contains(marker), "a lifecycle record leaked {marker}");
    }
    let _ = std::fs::remove_dir_all(path.parent().expect("parent"));
}
