//! Tauri command boundary for the ProbeLab desktop vertical slice.

#![deny(missing_docs)]

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use probelab_benchmarks::BenchmarkCatalog;
use probelab_core::{TelemetryEvent, TestResult};
use probelab_telemetry::{SqliteResultRepository, SqliteTelemetryRecorder};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

mod commands;
pub use commands::{get_result, list_benchmarks, run_benchmark, subscribe_telemetry};

const DATABASE_FILE_NAME: &str = "probelab.sqlite3";
const RECENT_EVENT_LIMIT: usize = 256;
static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique identifier returned when a benchmark run is accepted.
pub type RunId = String;

#[derive(Clone, Default)]
struct RecentEventBuffer {
    events: Arc<Mutex<HashMap<String, VecDeque<TelemetryEvent>>>>,
}

impl RecentEventBuffer {
    fn push(&self, event: TelemetryEvent) {
        let mut buffers = self.events.lock().expect("recent telemetry buffer lock");
        let events = buffers.entry(event.run_id.clone()).or_default();
        if events.len() == RECENT_EVENT_LIMIT {
            events.pop_front();
        }
        if events.len() < RECENT_EVENT_LIMIT {
            events.push_back(event);
        }
    }

    fn replay(&self, run_id: &str) -> Vec<TelemetryEvent> {
        let buffers = self.events.lock().expect("recent telemetry buffer lock");
        buffers
            .get(run_id)
            .map(|events| events.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// Thread-safe application state shared by the Tauri command boundary.
pub struct AppState {
    catalog: BenchmarkCatalog,
    recorder: Arc<SqliteTelemetryRecorder>,
    results: Arc<SqliteResultRepository>,
    recent_events: RecentEventBuffer,
}

#[derive(Clone, Serialize)]
struct TelemetryEnvelope {
    run_id: String,
    event: TelemetryEvent,
}

#[derive(Clone, Serialize)]
struct RunCompleteEnvelope {
    run_id: String,
    result: TestResult,
}

#[derive(Clone, Serialize)]
struct RunErrorEnvelope {
    run_id: String,
    error: String,
}

fn new_run_id() -> Result<RunId, String> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before the Unix epoch: {error}"))?;
    let timestamp = u64::try_from(elapsed.as_millis())
        .map_err(|_| "system timestamp is outside the supported run ID range".to_owned())?;
    let sequence = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(format!("{timestamp}-{sequence:016x}"))
}

fn emit_run_error(app: &AppHandle, run_id: &str, error: impl Into<String>) {
    let payload = RunErrorEnvelope {
        run_id: run_id.to_owned(),
        error: error.into(),
    };
    if let Err(emit_error) = app.emit("probe-lab/run-error", payload) {
        tracing::error!(run_id, error = %emit_error, "failed to emit run error event");
    }
}

/// Starts the ProbeLab Tauri desktop application.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let filter = tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("probelab=info"));
            let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

            let app_data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data_dir)?;
            let database_path = app_data_dir.join(DATABASE_FILE_NAME);
            let recent_events = RecentEventBuffer::default();
            let callback_events = recent_events.clone();
            let callback_app = app.handle().clone();
            let recorder = Arc::new(SqliteTelemetryRecorder::with_callback(
                &database_path,
                move |event| {
                    let event_run_id = event.run_id.clone();
                    let envelope = TelemetryEnvelope {
                        run_id: event.run_id.clone(),
                        event: event.clone(),
                    };
                    callback_events.push(event);
                    if let Err(error) = callback_app.emit("probe-lab/telemetry", envelope) {
                        tracing::error!(
                            run_id = %event_run_id,
                            error = %error,
                            "failed to emit telemetry event"
                        );
                    }
                },
            )?);
            let results = Arc::new(SqliteResultRepository::new(&database_path)?);
            app.manage(AppState {
                catalog: BenchmarkCatalog::new()?,
                recorder,
                results,
                recent_events,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_benchmarks,
            run_benchmark,
            get_result,
            subscribe_telemetry
        ])
        .run(tauri::generate_context!())
        .expect("error while running ProbeLab desktop application");
}

#[cfg(test)]
mod tests {
    use super::new_run_id;

    #[test]
    fn run_ids_include_a_monotonic_counter() {
        let first = new_run_id().expect("first run ID");
        let second = new_run_id().expect("second run ID");
        assert_ne!(first, second);
        assert!(first.contains('-'));
        assert!(second.contains('-'));
    }
}
