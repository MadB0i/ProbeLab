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

mod arena;
mod brain;
mod commands;
mod provider;
mod security;

pub use arena::{
    get_arena_match, latest_arena_match, list_arena_agents, list_arena_games, play_arena_match,
    replay_arena_match, ArenaAgentSummary, ArenaCommandError, ArenaGameSummary, ArenaMatchRequest,
    ArenaSurface,
};
pub use brain::{
    brain_panel, get_experiment, list_experiments, run_experiment, BrainCommandError, BrainSurface,
    ExperimentRequest,
};
pub use commands::{
    get_provider_status, get_result, get_run_metadata, list_agents, list_benchmark_details,
    list_benchmarks, list_providers, run_benchmark, subscribe_telemetry, AgentSummary,
    BenchmarkDetail, ProviderSummary,
};
pub use provider::{
    ModelRunConfig, ModelRunConfigError, ProviderConnectionStatus, ProviderStatus,
    CREDENTIAL_SOURCE_ENVIRONMENT, MOCK_PROVIDER, MODEL_RUN_CONFIG_SCHEMA_VERSION,
    OPENAI_COMPATIBLE_PROVIDER, PROVIDER_STATUS_SCHEMA_VERSION,
};
pub use security::{
    latest_security_report, list_security_probes, run_security_sweep, SecurityCommandError,
    SecurityProbeListing, SecuritySurface, SecuritySweepRequest,
};

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
    /// The command center's view of the Lab Brain and the experiments it has run.
    ///
    /// Managed state rather than something a command owns, because the panel asks "what is true right
    /// now" as well as "tell me when it changes" — a channel alone would show an empty panel after a
    /// reload even though a run had already finished.
    pub brain: BrainSurface,
    /// Completed arena matches, so a finished match stays inspectable.
    pub arena: ArenaSurface,
    /// The most recent security sweep, so a completed one stays inspectable.
    pub security: SecuritySurface,
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
            let brain = BrainSurface::default();
            // The Brain's runner records into the same durable sink as a single-episode run, so an
            // experiment's events and a run's events end up in one place rather than two.
            brain.set_recorder(recorder.clone());
            app.manage(AppState {
                catalog: BenchmarkCatalog::new()?,
                brain,
                arena: ArenaSurface::default(),
                security: SecuritySurface::default(),
                recorder,
                results,
                recent_events,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            brain_panel,
            get_experiment,
            list_experiments,
            run_experiment,
            list_agents,
            list_benchmark_details,
            list_benchmarks,
            list_providers,
            get_provider_status,
            run_benchmark,
            get_result,
            get_run_metadata,
            subscribe_telemetry
        ])
        .run(tauri::generate_context!())
        .expect("error while running ProbeLab desktop application");
}

#[cfg(test)]
mod tests {
    use super::{
        list_agents, list_benchmark_details, list_providers, new_run_id, AgentSummary,
        BenchmarkDetail, ProviderSummary,
    };
    use probelab_lab::{AgentCategory, AgentRegistry};

    #[test]
    fn run_ids_include_a_monotonic_counter() {
        let first = new_run_id().expect("first run ID");
        let second = new_run_id().expect("second run ID");
        assert_ne!(first, second);
        assert!(first.contains('-'));
        assert!(second.contains('-'));
    }

    /// The command center's agent list is the registry, not a copy of it.
    ///
    /// This is the test that would have caught the original bug. The UI used to hold a hand-written
    /// array of strategies that drifted from the mock enum and the IPC allowlist, so the dropdown could
    /// offer an agent the backend refused. Deriving the list makes drift impossible rather than
    /// merely unlikely, and this asserts the derivation reaches the IPC boundary intact.
    #[test]
    fn the_agent_list_the_ui_receives_is_the_registry() {
        let summaries = list_agents();
        let specs = AgentRegistry::built_in().specs();
        assert_eq!(
            summaries.len(),
            specs.len(),
            "the UI saw a different agent count"
        );
        for (summary, spec) in summaries.iter().zip(specs.iter()) {
            assert_eq!(summary.id, spec.id);
            assert_eq!(summary.name, spec.name);
        }
    }

    /// The label rule has to survive the IPC boundary, not just exist in Rust.
    ///
    /// A `measures_a_model` flag that were dropped or inverted in serialization would let the command
    /// center present a sandbox probe's integrity count as a model capability score — precisely the
    /// claim the category system exists to prevent.
    #[test]
    fn the_model_claim_flag_survives_serialization() {
        for summary in list_agents() {
            let category: AgentCategory =
                serde_json::from_value(serde_json::Value::String(summary.category.clone()))
                    .unwrap_or_else(|_| panic!("{} has an unparseable category", summary.id));
            assert_eq!(
                summary.measures_a_model,
                category == AgentCategory::TargetModel,
                "{} reports measures_a_model inconsistently with its category",
                summary.id
            );
            assert!(!summary.category_label.is_empty());
        }
    }

    /// Agent summaries round-trip, because the UI parses them back.
    #[test]
    fn an_agent_summary_round_trips_through_the_ipc_encoding() {
        for summary in list_agents() {
            let json = serde_json::to_string(&summary).expect("serialize");
            let restored: AgentSummary = serde_json::from_str(&json).expect("parse");
            assert_eq!(restored.id, summary.id);
            assert_eq!(restored.measures_a_model, summary.measures_a_model);
            assert_eq!(restored.category_label, summary.category_label);
        }
    }

    /// No agent summary may carry anything that looks like a credential.
    #[test]
    fn an_agent_summary_never_carries_credential_shaped_text() {
        for summary in list_agents() {
            let json = serde_json::to_string(&summary).expect("serialize");
            for marker in ["sk-", "sk-ant", "AIza", "api_key\":", "token\":"] {
                assert!(
                    !json.contains(marker),
                    "{} leaked something credential-shaped into the IPC payload",
                    summary.id
                );
            }
        }
    }

    /// A benchmark's declared capability execution reaches the user verbatim.
    ///
    /// The reason this matters: a benchmark that authorizes `filesystem.write` without executing it
    /// must not be renderable in a way that reads like a working sandbox feature. The field exists so
    /// the honesty is in the row the user is looking at, not only in a document they may not read.
    #[test]
    fn capability_execution_reaches_the_ui_verbatim() {
        for detail in list_benchmark_details() {
            assert!(
                ["executed", "authorized_deferred", "not_applicable"]
                    .contains(&detail.capability_execution.as_str()),
                "{} reports an unknown execution mode {}",
                detail.id,
                detail.capability_execution
            );
            assert!(
                !detail.scoring.is_empty(),
                "{} has no scoring text",
                detail.id
            );
        }
    }

    /// Benchmark details and the catalog must not disagree about what exists.
    #[test]
    fn benchmark_details_and_the_catalog_agree() {
        let details: Vec<BenchmarkDetail> = list_benchmark_details();
        let catalog = probelab_benchmarks::BenchmarkCatalog::new().expect("built-in catalog");
        let catalog_ids: Vec<String> = catalog
            .summaries()
            .iter()
            .map(|summary| summary.benchmark_id.clone())
            .collect();
        let detail_ids: Vec<String> = details.iter().map(|d| d.id.clone()).collect();
        assert_eq!(
            catalog_ids, detail_ids,
            "the catalog and the registry disagree about which benchmarks exist"
        );
    }

    /// Every benchmark's reference agent is named, and the UI is told when there is none.
    ///
    /// A missing ceiling is not an error, but it must be visible: a benchmark with no reference cannot
    /// be compared against anything, and a user should see that before running thirty episodes.
    #[test]
    fn a_benchmark_without_a_ceiling_says_so() {
        for detail in list_benchmark_details() {
            match &detail.reference_agent {
                Some(reference) => assert!(!reference.is_empty()),
                None => assert!(
                    detail.description.len() > 20,
                    "{} has neither a reference agent nor a description",
                    detail.id
                ),
            }
        }
    }

    /// Provider listings carry variable names, never values.
    #[test]
    fn provider_summaries_carry_names_not_credentials() {
        let providers = list_providers();
        assert!(providers.iter().any(|p| p.id == "anthropic"));
        assert!(providers.iter().any(|p| p.id == "gemini"));
        assert!(providers.iter().any(|p| p.id == "openai-compatible"));
        for ProviderSummary {
            id,
            api_key_env,
            default_base_url,
            requires_credential,
            ..
        } in providers
        {
            assert!(!api_key_env.is_empty(), "{id} names no credential variable");
            assert!(
                api_key_env
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "{id} has a variable name that is not a plain identifier: {api_key_env}"
            );
            assert!(
                !default_base_url.contains('?'),
                "{id} puts a query in its base URL"
            );
            assert!(requires_credential);
        }
    }

    /// Round-trip check for the provider payload the settings screen parses.
    #[test]
    fn a_provider_summary_round_trips() {
        for provider in list_providers() {
            let json = serde_json::to_string(&provider).expect("serialize");
            let restored: ProviderSummary = serde_json::from_str(&json).expect("parse");
            assert_eq!(restored.id, provider.id);
            assert_eq!(restored.api_key_env, provider.api_key_env);
        }
    }
}
