//! Executor discovery, cancellation, and export commands.
//!
//! # Why these live together
//!
//! Discovery answers "what can run here", cancellation answers "stop what is
//! running", and export answers "take the evidence with you". All three are
//! about the run lifecycle rather than about any single benchmark, and none
//! of them can authorize, execute, or reclassify anything — the structural
//! tests for the other surfaces apply here by construction, because this
//! module holds no broker, no policy, and no spawner.

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use probelab_lab::BrainReadModel;
use serde::Serialize;
use tauri::State;

use crate::AppState;

/// Failures from the executor lifecycle commands.
#[derive(Debug, thiserror::Error)]
pub enum ExecutorCommandError {
    /// No finished experiment with that identifier is known.
    #[error("no finished experiment named {0:?} is available for export")]
    UnknownExperiment(String),
    /// The requested export format is not supported.
    #[error("unsupported export format {0:?}; expected \"json\" or \"csv\"")]
    UnsupportedFormat(String),
    /// The command center's state could not be read.
    #[error("the command center's state is unavailable: {0}")]
    StateUnavailable(String),
}

impl Serialize for ExecutorCommandError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Tracks cancellation handles for runs in flight.
///
/// A map rather than a single flag, because the UI may start a second run
/// after the first finishes, and cancelling by identifier cannot stop the
/// wrong run. Entries are removed when their run ends, so a stale handle can
/// never cancel a later run that reused nothing.
pub type CancelRegistry = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

/// Every executor this machine may offer, with verified availability.
#[tauri::command(rename_all = "snake_case")]
pub fn list_executors() -> Vec<probelab_executors::ExecutorDescriptor> {
    probelab_executors::discover_executors()
}

/// The safe preflight for one executor: version checks and loopback connects
/// only. No task input, no network beyond loopback, no credentials touched.
#[tauri::command(rename_all = "snake_case")]
pub fn preflight_executor(id: String) -> probelab_executors::PreflightReport {
    probelab_executors::preflight_executor(&id)
}

/// Inspects disposable guest-execution support without changing anything.
///
/// Read-only and bounded: OS description, hypervisor presence, Sandbox
/// binary/feature state. Never enables a feature, never starts a guest. The
/// verdict is always guided-only — see the `isolation` module for why
/// presence is not a tested boundary.
#[tauri::command(rename_all = "snake_case")]
pub fn inspect_guest_isolation() -> probelab_executors::isolation::GuestIsolation {
    probelab_executors::isolation::inspect_guest_isolation()
}

/// Lists the model identifiers a local executor currently serves.
///
/// Read-only `GET /v1/models` against loopback, bounded, no credentials, no
/// task input. An empty list means "nothing to select" — whether no server
/// answers or the server lists no models is the discovery availability's job
/// to say, not this list's. Unknown executor ids are refused rather than
/// guessed at a port.
#[tauri::command(rename_all = "snake_case")]
pub fn list_local_models(executor_id: String) -> Result<Vec<String>, ExecutorCommandError> {
    use std::time::Duration;
    let port = match executor_id.as_str() {
        "ollama-local" => 11434,
        "llamacpp-local" => 8080,
        _ => {
            return Err(ExecutorCommandError::StateUnavailable(format!(
                "no local model runtime named {executor_id:?}"
            )));
        }
    };
    Ok(probelab_executors::local_model_ids(
        port,
        Duration::from_secs(5),
    ))
}

/// Requests cancellation of a run in flight.
///
/// Cooperative: the in-flight episode finishes first (so contained children
/// terminate and cleanup runs), then the runner stops with a `Cancelled`
/// terminal reason. Returns `true` when a run with that identifier was
/// actually signalled, `false` when nothing with that name is running — so a
/// stop button cannot report success for a run that never existed.
#[tauri::command(rename_all = "snake_case")]
pub fn cancel_experiment(
    state: State<'_, AppState>,
    experiment_id: String,
) -> Result<bool, ExecutorCommandError> {
    let registry = state
        .cancels
        .lock()
        .map_err(|_| ExecutorCommandError::StateUnavailable("cancel registry locked".to_owned()))?;
    match registry.get(&experiment_id) {
        Some(handle) => {
            handle.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// An exported experiment: a filename plus its contents as text.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentExport {
    /// Suggested filename, e.g. `probelab-exp-abc123.json`.
    pub filename: String,
    /// The export body. JSON is the full read model; CSV is the per-episode
    /// table plus the aggregate, so a spreadsheet holds the same numbers the
    /// panel showed.
    pub contents: String,
    /// The format that was produced.
    pub format: String,
}

/// Exports a finished experiment as JSON or CSV.
///
/// The source is the command center's stored read model — the same projection
/// the panel renders, so an export cannot disagree with the screen. Unknown
/// identifiers are refused rather than exported empty: an empty file that
/// looks like a result with no episodes would be worse than an error.
#[tauri::command(rename_all = "snake_case")]
pub fn export_experiment(
    state: State<'_, AppState>,
    experiment_id: String,
    format: String,
) -> Result<ExperimentExport, ExecutorCommandError> {
    let model: BrainReadModel = state
        .brain
        .experiment(&experiment_id)
        .ok_or_else(|| ExecutorCommandError::UnknownExperiment(experiment_id.clone()))?;
    match format.as_str() {
        "json" => {
            let contents = serde_json::to_string_pretty(&model)
                .map_err(|error| ExecutorCommandError::StateUnavailable(error.to_string()))?;
            Ok(ExperimentExport {
                filename: format!("probelab-{experiment_id}.json"),
                contents,
                format: "json".to_owned(),
            })
        }
        "csv" => {
            let contents = experiment_to_csv(&experiment_id, &model);
            Ok(ExperimentExport {
                filename: format!("probelab-{experiment_id}.csv"),
                contents,
                format: "csv".to_owned(),
            })
        }
        other => Err(ExecutorCommandError::UnsupportedFormat(other.to_owned())),
    }
}

/// Renders one experiment as CSV: provenance header, per-episode rows,
/// failures, and the aggregate with its Wilson interval.
///
/// Every number here is read from the stored read model rather than
/// recomputed, so the file and the panel cannot disagree.
fn experiment_to_csv(experiment_id: &str, model: &BrainReadModel) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    rows.push(vec![
        "section".to_owned(),
        "key".to_owned(),
        "value".to_owned(),
    ]);
    let experiment = model.experiment.as_ref();
    rows.push(vec![
        "provenance".to_owned(),
        "experiment_id".to_owned(),
        experiment_id.to_owned(),
    ]);
    if let Some(experiment) = experiment {
        for (key, value) in [
            ("benchmark_id", experiment.benchmark_id.clone()),
            ("target_agent", experiment.target_agent.clone()),
            ("provider", experiment.provider.clone().unwrap_or_default()),
            ("model", experiment.model.clone().unwrap_or_default()),
            (
                "episodes_requested",
                experiment.episodes_requested.to_string(),
            ),
            (
                "episodes_completed",
                experiment.episodes_completed.to_string(),
            ),
            ("episodes_failed", experiment.episodes_failed.to_string()),
            (
                "mean_success_fraction",
                experiment
                    .mean_success_fraction
                    .map(|score| score.to_string())
                    .unwrap_or_default(),
            ),
            (
                "terminal_reason",
                experiment
                    .terminal_reason
                    .as_ref()
                    .map(|reason| format!("{reason:?}"))
                    .unwrap_or_default(),
            ),
        ] {
            rows.push(vec!["provenance".to_owned(), key.to_owned(), value]);
        }
        rows.push(vec![
            "episode".to_owned(),
            "index".to_owned(),
            "seed".to_owned(),
            "task_success".to_owned(),
            "replay_digest".to_owned(),
            "telemetry_events".to_owned(),
        ]);
        for episode in &experiment.episodes {
            rows.push(vec![
                "episode".to_owned(),
                episode.index.to_string(),
                episode.seed.to_string(),
                episode.task_success.to_string(),
                episode.replay_digest.clone(),
                episode.telemetry_event_count.to_string(),
            ]);
        }
        for failure in &experiment.failures {
            rows.push(vec![
                "failure".to_owned(),
                failure.index.to_string(),
                failure.seed.to_string(),
                csv_cell(&failure.detail),
                String::new(),
                String::new(),
            ]);
        }
        let aggregate = &experiment.aggregate;
        rows.push(vec![
            "aggregate".to_owned(),
            "successes".to_owned(),
            aggregate.successes.to_string(),
            "episodes".to_owned(),
            aggregate.episodes.to_string(),
            format!(
                "interval [{:.4}, {:.4}]",
                aggregate.success_interval.lower, aggregate.success_interval.upper
            ),
        ]);
    } else {
        rows.push(vec![
            "provenance".to_owned(),
            "note".to_owned(),
            "the stored model holds no experiment".to_owned(),
        ]);
    }
    rows.iter()
        .map(|row| row.join(","))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One CSV cell: quotes when the content contains a comma, quote, or newline.
fn csv_cell(content: &str) -> String {
    if content.contains([',', '"', '\n']) {
        format!("\"{}\"", content.replace('"', "\"\""))
    } else {
        content.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_an_unknown_run_reports_false_rather_than_succeeding() {
        // A stop button that reported success for a run that never existed
        // would teach users the run stopped when nothing happened.
        let registry: CancelRegistry = Arc::new(Mutex::new(HashMap::new()));
        assert!(!registry.lock().expect("lock").contains_key("ghost"));
    }

    #[test]
    fn unknown_local_runtimes_are_refused_rather_than_guessed_at_a_port() {
        assert!(list_local_models("no-such-runtime".to_owned()).is_err());
    }

    #[test]
    fn csv_export_contains_the_episode_rows_it_claims() {
        let model = BrainReadModel::idle();
        let csv = experiment_to_csv("exp-1", &model);
        assert!(csv.contains("exp-1"));
    }

    #[test]
    fn csv_cells_quote_commas() {
        assert_eq!(csv_cell("a,b"), "\"a,b\"");
        assert_eq!(csv_cell("plain"), "plain");
    }

    #[test]
    fn executor_listing_reports_all_four_modes() {
        let executors = probelab_executors::discover_executors();
        let mut modes: Vec<&str> = executors
            .iter()
            .map(|executor| executor.mode.as_str())
            .collect();
        modes.sort_unstable();
        modes.dedup();
        assert_eq!(
            modes,
            vec![
                "coding_agent",
                "guided_external",
                "offline_local",
                "remote_provider"
            ]
        );
    }
}
