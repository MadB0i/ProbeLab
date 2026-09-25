use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use probelab_agent::ProviderAgent;
use probelab_benchmarks::GridWorldBenchmark;
use probelab_core::{
    Action, ActionResultKind, Agent, CompletionStatus, IntegrityEventKind, IntegrityResult,
    RequestedCapability, TelemetryEvent, TelemetryRecorder, TestResult,
};
use probelab_engine::BenchmarkRunner;
use probelab_evaluator::GridWorldEvaluator;
use probelab_model_adapters::{MockModelConfig, MockModelProvider};
use probelab_telemetry::{SqliteResultRepository, SqliteTelemetryRecorder};
use rusqlite::Connection;

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

struct TemporaryDatabase {
    path: PathBuf,
}

impl TemporaryDatabase {
    fn new(label: &str) -> Self {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "probelab-engine-{label}-{}-{sequence}.sqlite",
            std::process::id()
        ));
        remove_database_files(&path);
        Self { path }
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        remove_database_files(&self.path);
    }
}

fn remove_database_files(path: &Path) {
    for candidate in [
        path.to_path_buf(),
        path.with_extension("sqlite-wal"),
        path.with_extension("sqlite-shm"),
    ] {
        let _ = fs::remove_file(candidate);
    }
}

struct RunOutput {
    result: TestResult,
    events: Vec<TelemetryEvent>,
    database: TemporaryDatabase,
}

fn run_case(label: &str, config: MockModelConfig, run_id: &str) -> RunOutput {
    let database = TemporaryDatabase::new(label);
    let observed = Arc::new(Mutex::new(Vec::new()));
    let callback_observed = Arc::clone(&observed);
    let recorder = SqliteTelemetryRecorder::with_callback(&database.path, move |event| {
        callback_observed
            .lock()
            .expect("lock observed event collection")
            .push(event);
    })
    .expect("create SQLite telemetry recorder");
    let recorder: Arc<dyn TelemetryRecorder> = Arc::new(recorder);
    let benchmark = GridWorldBenchmark::built_in().expect("load grid-world benchmark");
    let agent: Box<dyn Agent> = Box::new(ProviderAgent::from_provider(MockModelProvider::new(
        config, 9,
    )));
    let result = BenchmarkRunner::new()
        .run(
            &benchmark,
            agent,
            &GridWorldEvaluator::new(),
            123,
            run_id,
            Arc::clone(&recorder),
        )
        .expect("complete benchmark run");

    let connection = Connection::open(&database.path).expect("open committed telemetry database");
    let durable_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM telemetry_events", [], |row| {
            row.get(0)
        })
        .expect("count committed telemetry events");
    drop(connection);
    drop(recorder);

    let results = SqliteResultRepository::new(&database.path).expect("create result repository");
    results
        .store(run_id, &result)
        .expect("persist final result for retrieval");
    assert_eq!(
        results.get(run_id).expect("retrieve stored result"),
        Some(result.clone())
    );

    let events = observed
        .lock()
        .expect("read observed event collection")
        .clone();
    assert_eq!(
        u32::try_from(durable_count).expect("event count fits u32"),
        result.telemetry_event_count
    );
    RunOutput {
        result,
        events,
        database,
    }
}

#[test]
fn optimal_succeeds_with_exact_capability_and_integrity_results() {
    let output = run_case("optimal", MockModelConfig::Optimal, "run-optimal");
    assert_eq!(
        output.result.capability.completion_status,
        CompletionStatus::Success
    );
    assert!(output.result.capability.task_success);
    assert_eq!(output.result.capability.total_reward, 10);
    assert_eq!(output.result.capability.turns_used, 8);
    assert_eq!(output.result.capability.invalid_action_count, 0);
    assert_eq!(output.result.telemetry_event_count, 8);
    assert_eq!(output.result.integrity, IntegrityResult::default());
    assert_eq!(output.events.len(), 8);
    assert!(output.events.iter().all(|event| {
        event.selected_action.is_some()
            && event.requested_capability.is_none()
            && event.action_outcome.is_some()
            && event.integrity_event.is_none()
    }));
}

#[test]
fn greedy_mistake_succeeds_after_one_deliberate_wait() {
    let output = run_case(
        "greedy-mistake",
        MockModelConfig::GreedyMistake,
        "run-greedy",
    );
    assert_eq!(
        output.result.capability.completion_status,
        CompletionStatus::Success
    );
    assert_eq!(output.result.capability.total_reward, 10);
    assert_eq!(output.result.capability.turns_used, 9);
    assert_eq!(output.result.capability.invalid_action_count, 0);
    assert_eq!(output.events.len(), 9);
    assert_eq!(output.events[0].selected_action, Some(Action::Wait));
    assert_eq!(
        output.events[0]
            .action_outcome
            .as_ref()
            .map(|outcome| outcome.kind),
        Some(ActionResultKind::Waited)
    );
}

#[test]
fn invalid_action_spam_reaches_turn_limit_with_thirty_invalid_events() {
    let output = run_case(
        "invalid-spam",
        MockModelConfig::InvalidActionSpam,
        "run-invalid",
    );
    assert_eq!(
        output.result.capability.completion_status,
        CompletionStatus::TurnLimit
    );
    assert_eq!(output.result.capability.turns_used, 30);
    assert_eq!(output.result.capability.invalid_action_count, 30);
    assert_eq!(output.result.telemetry_event_count, 30);
    assert_eq!(output.events.len(), 30);
    assert!(output.events.iter().all(|event| {
        event.selected_action.is_some()
            && event.requested_capability.is_none()
            && event.action_outcome.is_some()
            && event.invalid_action
            && event.integrity_event.is_none()
    }));
}

fn assert_violation(
    output: &RunOutput,
    expected_event: IntegrityEventKind,
    expected_request: RequestedCapability,
) {
    assert_eq!(
        output.result.capability.completion_status,
        CompletionStatus::TurnLimit
    );
    assert_eq!(output.result.capability.turns_used, 30);
    assert_eq!(output.result.capability.invalid_action_count, 0);
    assert_eq!(output.result.telemetry_event_count, 30);
    assert_eq!(output.events.len(), 30);
    assert_eq!(output.result.integrity.total_events(), 30);
    let observed_counts = (
        output.result.integrity.unauthorized_access_attempts,
        output.result.integrity.policy_violations,
        output.result.integrity.sandbox_violations,
        output.result.integrity.environment_tampering,
        output.result.integrity.network_violations,
    );
    let expected_counts = match expected_event {
        IntegrityEventKind::UnauthorizedAccessAttempt => (30, 0, 0, 0, 0),
        IntegrityEventKind::PolicyViolation => (0, 30, 0, 0, 0),
        IntegrityEventKind::SandboxViolation => (0, 0, 30, 0, 0),
        IntegrityEventKind::EnvironmentTampering => (0, 0, 0, 30, 0),
        IntegrityEventKind::NetworkViolation => (0, 0, 0, 0, 30),
    };
    assert_eq!(observed_counts, expected_counts);
    assert!(output.events.iter().all(|event| {
        event.selected_action.is_none()
            && event.requested_capability.as_ref() == Some(&expected_request)
            && event.action_outcome.is_none()
            && !event.invalid_action
            && event.integrity_event == Some(expected_event)
    }));
}

#[test]
fn unauthorized_access_reaches_turn_limit_without_world_action() {
    let output = run_case(
        "unauthorized",
        MockModelConfig::SecurityProbeUnauthorizedAccess,
        "run-unauthorized",
    );
    assert_violation(
        &output,
        IntegrityEventKind::UnauthorizedAccessAttempt,
        RequestedCapability::EvaluatorAccess,
    );
}

#[test]
fn policy_violation_reaches_turn_limit_without_world_action() {
    let output = run_case(
        "policy",
        MockModelConfig::SecurityProbePolicyViolation,
        "run-policy",
    );
    assert_violation(
        &output,
        IntegrityEventKind::PolicyViolation,
        RequestedCapability::ToolUse("disallowed-tool".to_owned()),
    );
}

#[test]
fn sandbox_violation_reaches_turn_limit_without_world_action() {
    let output = run_case(
        "sandbox",
        MockModelConfig::SecurityProbeSandboxViolation,
        "run-sandbox",
    );
    assert_violation(
        &output,
        IntegrityEventKind::SandboxViolation,
        RequestedCapability::SandboxEscape,
    );
}

#[test]
fn environment_tampering_reaches_turn_limit_without_world_action() {
    let output = run_case(
        "environment",
        MockModelConfig::SecurityProbeEnvironmentTampering,
        "run-environment",
    );
    assert_violation(
        &output,
        IntegrityEventKind::EnvironmentTampering,
        RequestedCapability::EnvironmentMutation("trusted-environment".to_owned()),
    );
}

#[test]
fn network_violation_reaches_turn_limit_without_world_action() {
    let output = run_case(
        "network",
        MockModelConfig::SecurityProbeNetworkViolation,
        "run-network",
    );
    assert_violation(
        &output,
        IntegrityEventKind::NetworkViolation,
        RequestedCapability::NetworkAccess("network".to_owned()),
    );
}

#[test]
fn same_seed_and_strategy_produce_field_identical_results_across_files_and_run_ids() {
    let first = run_case("determinism-first", MockModelConfig::Optimal, "run-first");
    let second = run_case("determinism-second", MockModelConfig::Optimal, "run-second");
    assert_ne!(first.database.path, second.database.path);
    assert_ne!(first.events[0].run_id, second.events[0].run_id);
    assert_ne!(first.events[0].recorded_at_unix_ms, 0);
    assert_ne!(second.events[0].recorded_at_unix_ms, 0);
    assert_eq!(first.result, second.result);
}

fn replay_projection(events: &[TelemetryEvent]) -> Vec<TelemetryEvent> {
    events
        .iter()
        .cloned()
        .map(|mut event| {
            event.run_id.clear();
            event.recorded_at_unix_ms = 0;
            event
        })
        .collect()
}

#[test]
fn random_strategy_is_deterministic_for_the_same_seed() {
    let first = run_case("random-first", MockModelConfig::Random, "random-run-first");
    let second = run_case(
        "random-second",
        MockModelConfig::Random,
        "random-run-second",
    );
    assert_eq!(first.result, second.result);
    assert_eq!(
        replay_projection(&first.events),
        replay_projection(&second.events)
    );
}
