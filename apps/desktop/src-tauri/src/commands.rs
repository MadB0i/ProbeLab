use std::sync::Arc;

use super::{emit_run_error, new_run_id, AppState, RunId};
use probelab_agent::ProviderAgent;
use probelab_core::{Agent, BenchmarkSummary, TelemetryEvent, TelemetryRecorder, TestResult};
use probelab_engine::BenchmarkRunner;
use probelab_evaluator::GridWorldEvaluator;
use probelab_model_adapters::{MockModelConfig, MockModelProvider};
use tauri::{AppHandle, Emitter, State};

/// Returns the real benchmarks in the static catalog.
#[tauri::command(rename_all = "snake_case")]
pub fn list_benchmarks(state: State<'_, AppState>) -> Vec<BenchmarkSummary> {
    state.catalog.summaries().to_vec()
}

/// Validates and starts one benchmark run without blocking the UI thread.
#[tauri::command(rename_all = "snake_case")]
pub fn run_benchmark(
    app: AppHandle,
    state: State<'_, AppState>,
    benchmark_id: String,
    model_config: MockModelConfig,
) -> Result<RunId, String> {
    let benchmark = state
        .catalog
        .resolve(&benchmark_id)
        .map_err(|error| error.to_string())?;
    let summary = benchmark.summary();
    let run_id = new_run_id()?;
    let agent: Box<dyn Agent> = Box::new(ProviderAgent::from_provider(MockModelProvider::new(
        model_config,
        summary.seed,
    )));
    let recorder: Arc<dyn TelemetryRecorder> = state.recorder.clone();
    let results = Arc::clone(&state.results);
    let task_run_id = run_id.clone();
    let app = app.clone();
    let benchmark_id_for_log = summary.benchmark_id.clone();

    tracing::info!(
        run_id = %run_id,
        benchmark_id = %benchmark_id_for_log,
        model_strategy = %model_config.strategy_name(),
        benchmark_seed = summary.seed,
        "benchmark run accepted"
    );

    tauri::async_runtime::spawn(async move {
        let runner = BenchmarkRunner::new();
        let result = match runner.run(
            benchmark.as_ref(),
            agent,
            &GridWorldEvaluator::new(),
            summary.seed,
            task_run_id.clone(),
            recorder,
        ) {
            Ok(result) => result,
            Err(error) => {
                emit_run_error(&app, &task_run_id, error.to_string());
                return;
            }
        };

        if let Err(error) = results.store(&task_run_id, &result) {
            emit_run_error(
                &app,
                &task_run_id,
                format!("failed to store benchmark result: {error}"),
            );
            return;
        }

        let payload = super::RunCompleteEnvelope {
            run_id: task_run_id.clone(),
            result,
        };
        if let Err(error) = app.emit("probe-lab/run-complete", payload) {
            tracing::error!(
                run_id = %task_run_id,
                error = %error,
                "failed to emit run completion event"
            );
        }
    });

    Ok(run_id)
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

/// Replays the bounded in-memory telemetry buffer for a run.
#[tauri::command(rename_all = "snake_case")]
pub fn subscribe_telemetry(state: State<'_, AppState>, run_id: RunId) -> Vec<TelemetryEvent> {
    state.recent_events.replay(&run_id)
}
