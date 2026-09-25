use std::sync::Arc;

use super::{
    emit_run_error, new_run_id, AppState, ModelRunConfig, ProviderStatus, RunCompleteEnvelope,
    RunId,
};
use probelab_core::{
    BenchmarkSummary, CompletedRun, RunMetadata, TelemetryEvent, TelemetryRecorder, TestResult,
};
use probelab_engine::BenchmarkRunner;
use probelab_evaluator::GridWorldEvaluator;
use probelab_telemetry::SqliteResultRepository;
use tauri::{AppHandle, Emitter, State};

/// Returns the real benchmarks in the static catalog.
#[tauri::command(rename_all = "snake_case")]
pub fn list_benchmarks(state: State<'_, AppState>) -> Vec<BenchmarkSummary> {
    state.catalog.summaries().to_vec()
}

/// Returns credential-free provider status for a candidate run configuration.
///
/// The credential itself is read from the environment inside the command and is never
/// returned. Only the generic `credential_source` label, the model name, the prompt template
/// version, and a human message cross the IPC boundary.
#[tauri::command(rename_all = "snake_case")]
pub fn get_provider_status(model_config: ModelRunConfig) -> ProviderStatus {
    model_config.provider_status()
}

/// Validates a tagged model configuration and starts one benchmark run.
///
/// The engine call is synchronous, so it runs on a blocking worker via
/// [`tauri::async_runtime::spawn_blocking`]; the async task only awaits that worker and then
/// emits events. Events are emitted after the SQLite commit for the result and its durable
/// metadata, never before.
#[tauri::command(rename_all = "snake_case")]
pub fn run_benchmark(
    app: AppHandle,
    state: State<'_, AppState>,
    benchmark_id: String,
    model_config: ModelRunConfig,
) -> Result<RunId, String> {
    let benchmark = state
        .catalog
        .resolve(&benchmark_id)
        .map_err(|error| error.to_string())?;
    let summary = benchmark.summary();
    let run_id = new_run_id()?;
    let agent = model_config
        .build_agent(summary.seed)
        .map_err(|error| error.to_string())?;
    let recorder: Arc<dyn TelemetryRecorder> = state.recorder.clone();
    let results = Arc::clone(&state.results);
    let task_run_id = run_id.clone();
    let task_benchmark_id = summary.benchmark_id.clone();
    let benchmark_seed = summary.seed;
    let app = app.clone();

    tracing::info!(
        run_id = %run_id,
        benchmark_id = %task_benchmark_id,
        model_kind = %model_config.kind_name(),
        benchmark_seed,
        "benchmark run accepted"
    );

    tauri::async_runtime::spawn(async move {
        let run_id_for_engine = task_run_id.clone();
        let run_id_for_persist = task_run_id.clone();
        let engine_task = tauri::async_runtime::spawn_blocking(move || {
            let completed = BenchmarkRunner::new()
                .run_with_metadata(
                    benchmark.as_ref(),
                    agent,
                    &GridWorldEvaluator::new(),
                    benchmark_seed,
                    run_id_for_engine,
                    recorder,
                )
                .map_err(|error| error.to_string())?;
            persist_run(&results, &run_id_for_persist, &completed)?;
            Ok::<CompletedRun, String>(completed)
        });

        let completed = match engine_task.await {
            Ok(Ok(completed)) => completed,
            Ok(Err(error)) => {
                emit_run_error(&app, &task_run_id, error);
                return;
            }
            Err(error) => {
                emit_run_error(
                    &app,
                    &task_run_id,
                    format!("benchmark run worker failed: {error}"),
                );
                return;
            }
        };

        emit_run_complete(&app, &task_run_id, completed);
    });

    Ok(run_id)
}

/// Commits the result and its durable metadata before any completion event is emitted.
///
/// The repository writes both payloads in one transaction, so a `run-complete` event can
/// never describe a run that `get_result` or `get_run_metadata` cannot read back.
fn persist_run(
    results: &SqliteResultRepository,
    run_id: &str,
    completed: &CompletedRun,
) -> Result<(), String> {
    results
        .store_completed(run_id, completed)
        .map_err(|error| format!("failed to store benchmark result: {error}"))
}

/// Reads a result only from the durable result repository.
#[tauri::command(rename_all = "snake_case")]
pub fn get_result(state: State<'_, AppState>, run_id: RunId) -> Result<TestResult, String> {
    state
        .results
        .get(&run_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("no stored result is available for run {run_id:?}"))
}

/// Reads the durable run metadata only from the result repository.
///
/// The metadata carries provider, model, prompt version, configuration fingerprint,
/// environment version, timing, the ordered action trace, and cumulative token usage. It
/// contains no credential material.
#[tauri::command(rename_all = "snake_case")]
pub fn get_run_metadata(state: State<'_, AppState>, run_id: RunId) -> Result<RunMetadata, String> {
    state
        .results
        .get_metadata(&run_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("no stored run metadata is available for run {run_id:?}"))
}

/// Replays the bounded in-memory telemetry buffer for a run.
///
/// The UI uses this to fill the gap between subscribing to live events and the first
/// committed event; it never polls SQLite.
#[tauri::command(rename_all = "snake_case")]
pub fn subscribe_telemetry(state: State<'_, AppState>, run_id: RunId) -> Vec<TelemetryEvent> {
    state.recent_events.replay(&run_id)
}

fn emit_run_complete(app: &AppHandle, run_id: &str, completed: CompletedRun) {
    let payload = RunCompleteEnvelope {
        run_id: run_id.to_owned(),
        result: completed.result,
    };
    if let Err(error) = app.emit("probe-lab/run-complete", payload) {
        tracing::error!(
            run_id,
            error = %error,
            "failed to emit run completion event"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use probelab_benchmarks::GridWorldBenchmark;
    use probelab_core::{
        Agent, Benchmark, CompletionStatus, TelemetryEvent as StoredEvent, TelemetryRecorder,
        TestResult as StoredResult,
    };
    use probelab_evaluator::GridWorldEvaluator;
    use probelab_model_adapters::{MockModelConfig, OpenAiCompatibleConfig};
    use probelab_telemetry::SqliteTelemetryRecorder;

    use super::{
        persist_run, BenchmarkRunner, CompletedRun, ModelRunConfig, SqliteResultRepository,
    };
    use crate::provider::CREDENTIAL_SOURCE_ENVIRONMENT;

    static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

    struct TemporaryDatabase {
        path: PathBuf,
    }

    impl TemporaryDatabase {
        fn new(label: &str) -> Self {
            let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "probelab-desktop-{label}-{}-{sequence}.sqlite",
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

    fn remove_database_files(path: &std::path::Path) {
        for candidate in [
            path.to_path_buf(),
            path.with_extension("sqlite-wal"),
            path.with_extension("sqlite-shm"),
        ] {
            let _ = std::fs::remove_file(candidate);
        }
    }

    fn build_benchmark() -> Box<dyn Benchmark> {
        Box::new(GridWorldBenchmark::built_in().expect("built-in benchmark"))
    }

    /// Mirrors `run_benchmark` end to end without needing a live Tauri app handle.
    fn run_and_store(
        database: &TemporaryDatabase,
        model_config: &ModelRunConfig,
        run_id: &str,
    ) -> Result<CompletedRun, String> {
        let benchmark = build_benchmark();
        let summary = benchmark.summary();
        let agent = model_config
            .build_agent(summary.seed)
            .map_err(|error| error.to_string())?;
        let recorder = Arc::new(SqliteTelemetryRecorder::new(&database.path).expect("recorder"));
        let recorder: Arc<dyn TelemetryRecorder> = recorder;
        let results = SqliteResultRepository::new(&database.path).expect("result repository");
        let completed = probelab_engine::BenchmarkRunner::new()
            .run_with_metadata(
                benchmark.as_ref(),
                agent,
                &GridWorldEvaluator::new(),
                summary.seed,
                run_id,
                recorder,
            )
            .map_err(|error| error.to_string())?;
        persist_run(&results, run_id, &completed)?;
        Ok(completed)
    }

    #[test]
    fn mock_run_persists_result_and_metadata_before_it_can_be_read_back() {
        let database = TemporaryDatabase::new("mock-run");
        let model_config = ModelRunConfig::mock(MockModelConfig::Optimal);
        let completed = run_and_store(&database, &model_config, "desktop-mock-run")
            .expect("mock run stores its result");

        let results = SqliteResultRepository::new(&database.path).expect("reopen repository");
        let stored: StoredResult = results
            .get("desktop-mock-run")
            .expect("read result")
            .expect("stored result");
        let metadata = results
            .get_metadata("desktop-mock-run")
            .expect("read metadata")
            .expect("stored metadata");

        assert_eq!(stored, completed.result);
        assert_eq!(metadata, completed.metadata);
        assert_eq!(metadata.provider, "deterministic-mock");
        assert_eq!(metadata.prompt_template_version, "deterministic_mock_v1");
        assert_eq!(metadata.environment_version, "grid_world_environment_v1");
        assert_eq!(
            metadata.action_trace.len(),
            stored.telemetry_event_count as usize
        );
        assert_eq!(
            metadata.protocol_failure_threshold,
            probelab_core::PROTOCOL_FAILURE_THRESHOLD
        );
        assert!(!metadata.configuration_fingerprint.is_empty());
        assert!(metadata.ended_at_unix_ms >= metadata.started_at_unix_ms);
    }

    #[test]
    fn mock_run_telemetry_matches_the_durable_action_trace() {
        let database = TemporaryDatabase::new("mock-trace");
        let model_config = ModelRunConfig::mock(MockModelConfig::Optimal);
        let completed =
            run_and_store(&database, &model_config, "desktop-trace-run").expect("mock run stores");

        // The UI reads the trace by turn, so every entry must carry a one-based turn and
        // must line up one-to-one with the committed telemetry events.
        let turns: Vec<u32> = completed
            .metadata
            .action_trace
            .iter()
            .map(|entry| entry.turn)
            .collect();
        assert_eq!(turns.len(), completed.result.telemetry_event_count as usize);
        assert_eq!(turns, (1..=turns.len() as u32).collect::<Vec<_>>());
    }

    #[test]
    fn a_connected_run_without_a_credential_is_rejected_before_any_row_is_written() {
        let database = TemporaryDatabase::new("missing-credential");
        let model_config = ModelRunConfig::openai_compatible(&OpenAiCompatibleConfig {
            base_url: "https://provider.example/v1".to_owned(),
            api_key_env: "PROBELAB_DESKTOP_ABSENT_KEY".to_owned(),
            ..OpenAiCompatibleConfig::default()
        });

        let error = run_and_store(&database, &model_config, "desktop-absent-run")
            .expect_err("a missing credential must stop the run");
        assert!(error.contains("PROBELAB_DESKTOP_ABSENT_KEY"));
        assert!(error.contains("export the API key"));

        let results = SqliteResultRepository::new(&database.path).expect("open repository");
        assert!(results
            .get("desktop-absent-run")
            .expect("read result")
            .is_none());
        assert!(results
            .get_metadata("desktop-absent-run")
            .expect("read metadata")
            .is_none());
    }

    #[test]
    fn a_connected_run_stores_prompt_version_fingerprint_and_token_usage_without_secrets() {
        let database = TemporaryDatabase::new("connected-metadata");
        let variable = "PROBELAB_DESKTOP_CONNECTED_METADATA_KEY";
        let secret = "test-only-desktop-secret";
        std::env::set_var(variable, secret);
        let model_config = ModelRunConfig::openai_compatible(&OpenAiCompatibleConfig {
            // A closed loopback port fails fast and hermetically, with no DNS or network use.
            base_url: "http://127.0.0.1:1/v1".to_owned(),
            api_key_env: variable.to_owned(),
            model: "test-model".to_owned(),
            ..OpenAiCompatibleConfig::default()
        });
        // The provider is unreachable, so every turn becomes a typed transport failure. The
        // run still terminates, and the metadata must stay free of credential material.
        let outcome = run_and_store(&database, &model_config, "desktop-connected-run");
        std::env::remove_var(variable);

        let results = SqliteResultRepository::new(&database.path).expect("open repository");
        let metadata: probelab_core::RunMetadata = results
            .get_metadata("desktop-connected-run")
            .expect("read metadata")
            .expect("stored metadata");
        assert_eq!(metadata.provider, "openai-compatible");
        assert_eq!(metadata.model, "test-model");
        assert_eq!(metadata.prompt_template_version, "grid_world_v1");
        assert!(!metadata.configuration_fingerprint.is_empty());
        assert!(!metadata.action_trace.is_empty());
        assert!(metadata
            .action_trace
            .iter()
            .all(|entry| entry.model_failure_kind.is_some()));
        let CompletedRun { result, .. } = outcome.expect("an unreachable provider still completes");
        assert_eq!(
            result.capability.completion_status,
            CompletionStatus::ModelProtocolFailure
        );
        assert_eq!(result.integrity.total_events(), 0);
        assert_eq!(
            result.capability.protocol_adherence.transport_failures,
            u64::from(result.telemetry_event_count)
        );
        let serialized = serde_json::to_string(&metadata).expect("serialize metadata");
        assert!(!serialized.contains(secret));
        assert!(!serialized.contains(variable));
    }

    #[test]
    fn provider_status_never_carries_credential_material() {
        let variable = "PROBELAB_DESKTOP_STATUS_KEY";
        let secret = "test-only-desktop-status-secret";
        std::env::set_var(variable, secret);
        let status = ModelRunConfig::openai_compatible(&OpenAiCompatibleConfig {
            base_url: "https://provider.example/v1".to_owned(),
            api_key_env: variable.to_owned(),
            model: "status-model".to_owned(),
            ..OpenAiCompatibleConfig::default()
        })
        .provider_status();
        std::env::remove_var(variable);

        assert!(status.configured);
        assert_eq!(status.credential_source, CREDENTIAL_SOURCE_ENVIRONMENT);
        let serialized = serde_json::to_string(&status).expect("serialize status");
        assert!(!serialized.contains(secret));
        assert!(!serialized.contains(variable));
    }

    #[test]
    fn telemetry_events_survive_the_run_for_the_replay_buffer() {
        let database = TemporaryDatabase::new("telemetry-events");
        let events = Arc::new(Mutex::new(Vec::<StoredEvent>::new()));
        let sink = Arc::clone(&events);
        let recorder = Arc::new(
            SqliteTelemetryRecorder::with_callback(&database.path, move |event| {
                sink.lock().expect("lock events").push(event);
            })
            .expect("callback recorder"),
        );
        let recorder: Arc<dyn TelemetryRecorder> = recorder;
        let benchmark = build_benchmark();
        let summary = benchmark.summary();
        let agent: Box<dyn Agent> = Box::new(probelab_agent::ProviderAgent::from_provider(
            probelab_model_adapters::MockModelProvider::new(MockModelConfig::Optimal, summary.seed),
        ));
        let completed = probelab_engine::BenchmarkRunner::new()
            .run_with_metadata(
                benchmark.as_ref(),
                agent,
                &GridWorldEvaluator::new(),
                summary.seed,
                "telemetry-run",
                recorder,
            )
            .expect("run");

        let observed = events.lock().expect("lock events").clone();
        assert_eq!(
            observed.len() as u32,
            completed.result.telemetry_event_count
        );
        assert!(observed.iter().all(|event| event.sequence > 0));
    }

    #[test]
    fn a_connected_run_completes_on_a_blocking_worker_without_nesting_runtimes() {
        // `AsyncProviderAgent` bridges to a Tokio runtime internally. Running the engine on a
        // `spawn_blocking` worker keeps that bridge off the Tauri async task, so a nested
        // `block_on` cannot panic. This test drives the same shape the command uses.
        let database = TemporaryDatabase::new("blocking-worker");
        let variable = "PROBELAB_DESKTOP_BLOCKING_KEY";
        std::env::set_var(variable, "test-only-desktop-secret");
        let model_config = ModelRunConfig::openai_compatible(&OpenAiCompatibleConfig {
            base_url: "http://127.0.0.1:1/v1".to_owned(),
            api_key_env: variable.to_owned(),
            model: "blocking-model".to_owned(),
            ..OpenAiCompatibleConfig::default()
        });

        let benchmark = build_benchmark();
        let summary = benchmark.summary();
        let agent = model_config
            .build_agent(summary.seed)
            .expect("credential present builds the connected agent");
        let recorder: Arc<dyn TelemetryRecorder> =
            Arc::new(SqliteTelemetryRecorder::new(&database.path).expect("recorder"));
        let seed = summary.seed;
        let benchmark_for_worker = benchmark;

        let outcome = tauri::async_runtime::block_on(async move {
            tauri::async_runtime::spawn_blocking(move || {
                BenchmarkRunner::new().run_with_metadata(
                    benchmark_for_worker.as_ref(),
                    agent,
                    &GridWorldEvaluator::new(),
                    seed,
                    "blocking-run",
                    recorder,
                )
            })
            .await
        });
        std::env::remove_var(variable);

        let completed = outcome
            .expect("the blocking worker must not fail")
            .expect("the engine must not fail");
        assert_eq!(completed.metadata.provider, "openai-compatible");
        assert_eq!(completed.metadata.model, "blocking-model");
        assert_eq!(completed.metadata.prompt_template_version, "grid_world_v1");
        assert_eq!(
            completed.result.capability.completion_status,
            CompletionStatus::ModelProtocolFailure
        );
    }
}
