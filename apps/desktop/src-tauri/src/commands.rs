use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use super::{
    emit_run_cancelled, emit_run_error, new_run_id, AppState, ModelRunConfig, ProviderStatus,
    RunCompleteEnvelope, RunId,
};
use probelab_core::{
    BenchmarkSummary, CompletedRun, RunMetadata, TelemetryEvent, TelemetryRecorder, TestResult,
};
use probelab_engine::{BenchmarkRunner, EngineError, RunContext};
use probelab_evaluator::GridWorldEvaluator;
use probelab_lab::AgentRegistry;
use probelab_model_adapters::ProviderKind;
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

/// Returns every agent the lab can evaluate, from the authoritative registry.
///
/// # Why the UI does not keep its own list
///
/// It used to. `App.tsx` held a hand-written `strategies` array that had to be edited in step with
/// the mock enum, the IPC allowlist, and the reference policies — four places to add an agent and
/// three chances to forget one, which produced a dropdown entry the backend then refused. This command
/// makes the registry the only list, so adding an agent is a one-line change that cannot half-land.
#[tauri::command(rename_all = "snake_case")]
pub fn list_agents() -> Vec<AgentSummary> {
    AgentRegistry::built_in()
        .specs()
        .into_iter()
        .map(AgentSummary::from)
        .collect()
}

/// Returns every benchmark with its full metadata rather than the short catalog summary.
#[tauri::command(rename_all = "snake_case")]
pub fn list_benchmark_details() -> Vec<BenchmarkDetail> {
    probelab_benchmarks::BenchmarkRegistry::built_in()
        .specs()
        .into_iter()
        .map(BenchmarkDetail::from)
        .collect()
}

/// Returns every provider vendor, with defaults and credential *names* but no credential values.
#[tauri::command(rename_all = "snake_case")]
pub fn list_providers() -> Vec<ProviderSummary> {
    ProviderKind::all()
        .into_iter()
        .map(ProviderSummary::from)
        .collect()
}

/// Renders an enum as its canonical wire string.
///
/// `format!("{:?}")` would be easier and is wrong. Debug gives `SecurityProbe` while the serde
/// encoding — the one result bundles, replay digests, and every other stored record use — gives
/// `security_probe`. A payload carrying the Debug form cannot be parsed back with the same decoder
/// that reads everything else, and a wire format that changes when someone reformats a struct is not a
/// wire format. Every enum that crosses into a summary goes through here.
fn wire<T: serde::Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| match value {
            serde_json::Value::String(text) => Some(text),
            _ => None,
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

/// One agent as the command center renders it.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSummary {
    /// Stable identifier, used to build the agent.
    pub id: String,
    /// Name shown in the picker.
    pub name: String,
    /// What the agent is, in one sentence.
    pub description: String,
    /// Which family it belongs to, which is what determines what its score means.
    pub category: String,
    /// The family's display label.
    pub category_label: String,
    /// Whether its score is a claim about a model.
    pub measures_a_model: bool,
    /// How it decides: `in_process_deterministic` or `remote_model`.
    pub execution: String,
    /// Whether deciding needs a network.
    pub requires_network: bool,
    /// Benchmarks this agent is scoped to, or empty when it applies to all.
    pub applicable_benchmarks: Vec<String>,
}

impl From<probelab_lab::AgentSpec> for AgentSummary {
    fn from(spec: probelab_lab::AgentSpec) -> Self {
        Self {
            id: spec.id,
            name: spec.name,
            description: spec.description,
            category: wire(spec.category),
            category_label: spec.category.label().to_owned(),
            measures_a_model: spec.category.measures_a_model(),
            execution: wire(spec.execution),
            requires_network: spec.execution.requires_network(),
            applicable_benchmarks: spec.applicable_benchmarks,
        }
    }
}

/// One benchmark with the metadata a command-center row needs.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkDetail {
    /// Stable identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// One-line description.
    pub description: String,
    /// What family it measures.
    pub category: String,
    /// The single capability dimension this benchmark isolates.
    pub capability_dimension: String,
    /// Whether the capabilities it declares are actually executed or only authorized.
    pub capability_execution: String,
    /// How an episode is scored, in words.
    pub scoring: String,
    /// The registered reference agent, when one exists.
    pub reference_agent: Option<String>,
    /// What replay means here, which is not always "re-run it".
    pub replay_support: String,
    /// The platforms it can run on.
    pub platform_requirements: String,
    /// Human-readable limits.
    pub limits: std::collections::BTreeMap<String, String>,
    /// The schema version.
    pub version: String,
}

impl From<probelab_benchmarks::BenchmarkSpec> for BenchmarkDetail {
    fn from(spec: probelab_benchmarks::BenchmarkSpec) -> Self {
        Self {
            id: spec.id,
            name: spec.name,
            description: spec.description,
            category: wire(spec.category),
            capability_dimension: spec.capability_dimension,
            // The whole point of this field reaching the UI: a user must be able to see that a
            // benchmark declaring `filesystem.write` authorizes it without executing it, rather than
            // discovering that from a score.
            capability_execution: wire(spec.capability_execution),
            scoring: spec.scoring,
            reference_agent: spec.reference_agent.clone(),
            replay_support: wire(spec.replay_support),
            platform_requirements: wire(spec.platform_requirements),
            limits: spec.limits,
            version: spec.version,
        }
    }
}

/// One provider vendor, with no credential material.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSummary {
    /// Stable identifier.
    pub id: String,
    /// Name shown in the picker.
    pub display_name: String,
    /// The endpoint a new configuration starts from.
    pub default_base_url: String,
    /// The environment variable to read the key from. A name, never a value.
    pub api_key_env: String,
    /// Whether a call needs a credential.
    pub requires_credential: bool,
}

impl From<ProviderKind> for ProviderSummary {
    fn from(kind: ProviderKind) -> Self {
        Self {
            id: kind.id().to_owned(),
            display_name: kind.display_name().to_owned(),
            default_base_url: kind.default_base_url().to_owned(),
            api_key_env: kind.default_api_key_env().to_owned(),
            requires_credential: kind.requires_credential(),
        }
    }
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

    /*
     * The `running` row is written before the engine starts.
     *
     * This is what makes a crash during the run detectable: start-up marks any row still saying
     * `running` as `interrupted`, because a row that says running and a process that has just started
     * cannot both be true. Writing only at the end would leave a crash invisible, and the history would
     * silently omit a run that had genuinely been started.
     */
    if let Err(error) = state
        .results
        .put_experiment(&probelab_telemetry::ExperimentRecord {
            experiment_id: task_run_id.clone(),
            benchmark_id: task_benchmark_id.clone(),
            lifecycle: probelab_telemetry::ExperimentLifecycle::Running,
            terminal_reason: None,
            episodes_requested: 1,
            episodes_completed: 0,
            episodes_failed: 0,
            has_score: false,
            score: None,
            result_json: None,
            updated_unix_ms: unix_milliseconds(),
            target_agent: Some(model_config.kind_name().to_owned()),
            root_seed: None,
        })
    {
        // Not fatal. The run is real either way; what is lost is the ability to tell a crash from a
        // run that never started, and that is worth saying out loud rather than only in a log.
        tracing::error!(
            error = %error,
            "could not record the running experiment; a crash during this run will be indistinguishable \
             from a run that never started"
        );
    }

    // The cancel handle is registered *before* the worker starts and removed when it finishes,
    // whichever way it finishes. Registering after would leave a window in which the run is executing
    // but the stop button reports "nothing is running"; never removing would let a stale handle stop
    // whatever started next under the same identifier.
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut registry = state
            .cancels
            .lock()
            .map_err(|_| "the cancel registry was left locked by an earlier fault".to_owned())?;
        registry.insert(task_run_id.clone(), Arc::clone(&cancel));
    }
    let registry = Arc::clone(&state.cancels);
    let id_for_cleanup = task_run_id.clone();
    // Every exit path from the worker removes the handle. A `Drop` guard would be tidier, but this
    // function already has four exits and an explicit removal at each is easier to verify than a
    // guard's drop order.
    let release = move || {
        if let Ok(mut registry) = registry.lock() {
            registry.remove(&id_for_cleanup);
        }
    };

    tauri::async_runtime::spawn(async move {
        let run_id_for_engine = task_run_id.clone();
        let run_id_for_persist = task_run_id.clone();
        let results_for_terminal = Arc::clone(&results);
        let engine_task = tauri::async_runtime::spawn_blocking(move || {
            let context = RunContext::ephemeral("grid")
                .map_err(|error| error.to_string())?
                .with_cancel(Arc::clone(&cancel));
            let completed: Result<CompletedRun, EngineError> = BenchmarkRunner::new()
                .run_with_context(
                    benchmark.as_ref(),
                    agent,
                    &GridWorldEvaluator::new(),
                    benchmark_seed,
                    run_id_for_engine,
                    recorder,
                    context,
                );
            // A cancelled run is recorded as `interrupted` — a correct terminal state, not a failure
            // and not a success. Writing it here rather than skipping the write is the point: an
            // experiment row that says `running` after the process has moved on is the one state this
            // persistence layer is designed never to leave behind.
            //
            // The outcome is typed rather than an `Err(String)`. Cancellation used to come back as
            // `Err("the run was cancelled")`, which forced the caller to recover the fact that it was a
            // cancellation by matching on English prose — and which meant the event the UI received could
            // not distinguish "the operator stopped this" from "something broke".
            match completed {
                Ok(completed) => {
                    persist_run(&results, &run_id_for_persist, &completed)?;
                    Ok::<RunOutcome, String>(RunOutcome::Completed(Box::new(completed)))
                }
                Err(EngineError::Cancelled { turns_used }) => {
                    record_cancelled_experiment(
                        &results,
                        &run_id_for_persist,
                        &task_benchmark_id,
                        turns_used,
                    );
                    Ok(RunOutcome::Cancelled { turns_used })
                }
                Err(error) => Ok(RunOutcome::Failed(error.to_string())),
            }
        });

        let outcome = match engine_task.await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => {
                release();
                emit_run_error(&app, &task_run_id, error);
                return;
            }
            Err(error) => {
                release();
                emit_run_error(
                    &app,
                    &task_run_id,
                    format!("benchmark run worker failed: {error}"),
                );
                return;
            }
        };

        release();
        drop(results_for_terminal);
        match outcome {
            RunOutcome::Completed(completed) => emit_run_complete(&app, &task_run_id, *completed),
            RunOutcome::Cancelled { turns_used } => {
                emit_run_cancelled(&app, &task_run_id, turns_used)
            }
            RunOutcome::Failed(error) => emit_run_error(&app, &task_run_id, error),
        }
    });

    Ok(run_id)
}

/// How a benchmark run ended, as three outcomes rather than a result-or-error pair.
///
/// A stop is not a failure and is not a success, and folding it into `Err` is what made it look like
/// one. Every consumer of a run's outcome now has to say which of the three it is handling, so the
/// default case is no longer "assume it worked".
///
/// The completed run is boxed because `CompletedRun` carries a whole `TestResult` — some nine hundred
/// bytes of benchmark metadata — and this value crosses a thread boundary and an async task to reach
/// the emitter. Unboxed, every cancel and every failure moved that much memory to carry a `u32`.
#[derive(Debug)]
enum RunOutcome {
    Completed(Box<CompletedRun>),
    Cancelled { turns_used: u32 },
    Failed(String),
}

/// Writes the lifecycle row for a run the operator stopped.
///
/// `Interrupted` rather than `Failed`, and with `has_score = false` and a null score. A cancelled run
/// produced no measurement, and recording a partial one as a score would put a number in the history
/// that no benchmark ever produced.
fn record_cancelled_experiment(
    results: &SqliteResultRepository,
    run_id: &str,
    benchmark_id: &str,
    turns_used: u32,
) {
    let outcome = results.put_experiment(&probelab_telemetry::ExperimentRecord {
        experiment_id: run_id.to_owned(),
        benchmark_id: benchmark_id.to_owned(),
        lifecycle: probelab_telemetry::ExperimentLifecycle::Interrupted,
        terminal_reason: Some(probelab_lab::TerminalReason::Cancelled.to_wire_json()),
        episodes_requested: 1,
        episodes_completed: 0,
        episodes_failed: 0,
        has_score: false,
        score: None,
        result_json: None,
        updated_unix_ms: unix_milliseconds(),
        target_agent: None,
        root_seed: None,
    });
    if let Err(error) = outcome {
        tracing::error!(
            error = %error,
            turns_used,
            "a cancelled benchmark run could not be recorded as interrupted; the history will show \
             it as still running until the next launch corrects it"
        );
    }
}

/// Commits the result and its durable metadata before any completion event is emitted.
///
/// The repository writes both payloads in one transaction, so a `run-complete` event can
/// never describe a run that `get_result` or `get_run_metadata` cannot read back.
/// Writes a finished benchmark run: its immutable result **and** the lifecycle row the history reads.
///
/// # Why both
///
/// A run used to persist only through [`SqliteResultRepository::store_completed`], which writes the
/// `results` row. The Command Center's stored-runs list reads the `experiments` table, so a run
/// executed, produced a complete result, rendered every number correctly — and the history stayed
/// permanently empty, on every launch. Nothing looked broken, because nothing *was* broken except one
/// row that was never written.
///
/// The two writes are deliberately separate: `results` is immutable evidence guarded by a trigger, and
/// `experiments` is a lifecycle record that is updated as a run progresses. Collapsing them would mean
/// either relaxing the immutability trigger or losing crash recovery, so they stay two rows written
/// together.
fn persist_run(
    results: &SqliteResultRepository,
    run_id: &str,
    completed: &CompletedRun,
) -> Result<(), String> {
    results
        .store_completed(run_id, completed)
        .map_err(|error| format!("failed to store benchmark result: {error}"))?;
    record_completed_experiment(results, run_id, completed)
}

/// Writes the lifecycle row for a run that reached a terminal state.
///
/// Every field is taken from what the run recorded. A run whose result carries no score is stored with
/// `has_score = false` and a null score, which the history renders as an explicit absence — never as a
/// zero, because a zero is a measurement.
fn record_completed_experiment(
    results: &SqliteResultRepository,
    run_id: &str,
    completed: &CompletedRun,
) -> Result<(), String> {
    let outcome = completed.result.capability.task_success;
    let episodes = 1usize;
    results
        .put_experiment(&probelab_telemetry::ExperimentRecord {
            experiment_id: run_id.to_owned(),
            benchmark_id: completed.metadata.benchmark_id.clone(),
            lifecycle: probelab_telemetry::ExperimentLifecycle::Completed,
            terminal_reason: Some(probelab_lab::TerminalReason::Completed.to_wire_json()),
            episodes_requested: episodes,
            episodes_completed: episodes,
            episodes_failed: 0,
            has_score: true,
            // The score of a single-episode benchmark run is its outcome: this benchmark scores success
            // as one and failure as zero, so `f64::from(outcome)` is the measurement rather than a
            // restatement of it.
            score: Some(f64::from(u8::from(outcome))),
            result_json: None,
            updated_unix_ms: completed.metadata.ended_at_unix_ms,
            // Recorded so the comparison picker can offer this run. A row with no agent is one the
            // picker refuses, and a run the lab cannot compare is a run it should still be able to list.
            target_agent: Some(completed.metadata.model.clone()),
            root_seed: None,
        })
        .map_err(|error| format!("failed to store the experiment history row: {error}"))
}

/// Now, in Unix milliseconds, for the lifecycle row's ordering column.
///
/// `0` rather than a panic: a clock before the epoch is absurd, and this column only orders history.
fn unix_milliseconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
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

/// Reads a run's committed telemetry back out of the append-only store.
///
/// # Why this is not `subscribe_telemetry`
///
/// `subscribe_telemetry` reads a bounded in-memory buffer of the *current session*, and says so. It
/// exists to close the gap between subscribing to live events and the first one arriving, so it
/// deliberately holds a bounded number of recent events for one run. Two things follow, and both
/// would have produced a convincing, wrong playback:
///
/// * A run from a previous session is not in the buffer at all, so after a restart its recording was
///   on disk and unreachable.
/// * Even in the same session, a long run would have its oldest events evicted, so the episode would
///   replay from the middle.
///
/// So this reads the durable table instead, which is append-only and holds every event the run
/// committed. It performs no execution and holds no model, provider, or engine: every field returned
/// is a field already on disk, which is what makes a replayed episode the same episode rather than a
/// second experiment.
///
/// An unknown run identifier is an empty vector, not an error, because "nothing was recorded under
/// that name" is the truthful answer and every caller would otherwise have to invent a distinction
/// between a run that never happened and a recording that is missing.
#[tauri::command(rename_all = "snake_case")]
pub fn get_run_telemetry(
    state: State<'_, AppState>,
    run_id: RunId,
) -> Result<Vec<TelemetryEvent>, String> {
    state
        .results
        .telemetry_events(&run_id)
        .map_err(|error| error.to_string())
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
        assert_eq!(metadata.environment_version, "grid_world_environment_v2");
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
        assert_eq!(metadata.prompt_template_version, "grid_world_v2");
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
        assert_eq!(completed.metadata.prompt_template_version, "grid_world_v2");
        assert_eq!(
            completed.result.capability.completion_status,
            CompletionStatus::ModelProtocolFailure
        );
    }
}
