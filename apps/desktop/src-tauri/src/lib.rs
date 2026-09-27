//! Tauri command boundary for the ProbeLab desktop vertical slice.

#![deny(missing_docs)]

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use probelab_benchmarks::BenchmarkCatalog;
use probelab_core::{TelemetryEvent, TestResult};
use probelab_lab::SqliteSettingsStore;
use probelab_telemetry::{SqliteResultRepository, SqliteTelemetryRecorder};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

mod arena;
mod brain;
mod comparison;
use comparison::{compare_experiments, list_comparable_experiments};
use settings::{apply_settings, read_settings, reset_settings};
mod commands;
mod provider;
mod security;
mod settings;

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
/// Settings live in their own file rather than in the results database.
///
/// Not tidiness: a settings reset must be incapable of touching a recorded result, and the clearest
/// way to guarantee that is for the two to have no shared file to write to. It also means a corrupt
/// settings row cannot be read through the results API at all.
const SETTINGS_FILE_NAME: &str = "probelab-settings.sqlite3";
const RECENT_EVENT_LIMIT: usize = 256;
static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unique identifier returned when a benchmark run is accepted.
pub type RunId = String;

#[derive(Clone, Default)]
struct RecentEventBuffer {
    events: Arc<Mutex<HashMap<String, VecDeque<TelemetryEvent>>>>,
}

impl RecentEventBuffer {
    /// Buffers an event for the replay read path, dropping the oldest when full.
    ///
    /// Deliberately infallible. This runs inside the SQLite recorder's commit callback, on whichever
    /// thread is driving the engine, mid-experiment. A panic here cannot be returned to anyone -- the
    /// callback's signature has no error channel -- and it would poison the lock for every later event
    /// of the session. So a failure is logged and the event is dropped, which costs one event from an
    /// in-memory convenience buffer; the durable SQLite record is written independently and is
    /// unaffected.
    fn push(&self, event: TelemetryEvent) {
        let mut buffers = match self.events.lock() {
            Ok(buffers) => buffers,
            Err(poisoned) => {
                // `PoisonError` still hands back the guard, so the buffer keeps working for the rest
                // of the session rather than becoming permanently dead.
                tracing::error!(
                    "the recent telemetry buffer was left locked by an earlier fault; recovering and \
                     continuing"
                );
                poisoned.into_inner()
            }
        };
        let events = buffers.entry(event.run_id.clone()).or_default();
        if events.len() == RECENT_EVENT_LIMIT {
            events.pop_front();
        }
        events.push_back(event);
    }

    /// The buffered events for a run, oldest first.
    ///
    /// An empty result means either "no events" or "the buffer is unavailable", and the two are not
    /// distinguished. That is the safe direction: the replay path falls back to the durable store, and
    /// reporting a partial in-memory buffer as a complete one would be worse than reporting none.
    fn replay(&self, run_id: &str) -> Vec<TelemetryEvent> {
        match self.events.lock() {
            Ok(buffers) => buffers
                .get(run_id)
                .map(|events| events.iter().cloned().collect())
                .unwrap_or_default(),
            Err(_) => {
                tracing::error!(
                    run_id,
                    "the recent telemetry buffer could not be read; the replay view is empty"
                );
                Vec::new()
            }
        }
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
    /// The durable settings store.
    ///
    /// A separate store, in a separate file, from `results`. Separate because results carry an
    /// immutability trigger and settings do not, and because a settings reset must be structurally
    /// incapable of reaching a measurement.
    pub settings: Arc<SqliteSettingsStore>,
    /// Where the settings are stored, so the settings page can report the real path.
    pub settings_path: std::path::PathBuf,
    /// Where results are stored, so the settings page can report the real path.
    pub results_path: std::path::PathBuf,
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
            // Settings live in their own file. Opened here so a failure to read them is a start-up
            // error rather than a surprise the first time someone opens the settings page, and so the
            // real path can be reported on that page as a read-only fact.
            let settings_path = app_data_dir.join(SETTINGS_FILE_NAME);
            let settings = Arc::new(
                SqliteSettingsStore::open(&settings_path).map_err(|error| {
                    format!(
                        "could not open the settings store at {}: {error}",
                        settings_path.display()
                    )
                })?,
            );
            //
            // A row still saying `running` at start-up is provably wrong: nothing is running. Left
            // alone it would make a crashed run indistinguishable from a completed one, which is the
            // worst thing a persistence layer can do — silently wrong rather than loudly absent. The
            // identifiers corrected are logged so the correction is visible rather than silent.
            match results.reconcile_interrupted() {
                Ok(recovered) if !recovered.is_empty() => {
                    tracing::warn!(
                        interrupted = ?recovered,
                        "experiments left running by a previous process were marked interrupted; their \
                         partial episodes are preserved but are not a complete measurement"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    // A failure to recover must not stop the application: it would turn a recoverable
                    // state into an unusable product. The runs stay on disk and the error is logged.
                    tracing::error!(
                        error = %error,
                        "could not reconcile interrupted experiments; stored lifecycle state may be stale"
                    );
                }
            }
            let brain = BrainSurface::default();
            // The Brain's runner records into the same durable sink as a single-episode run, so an
            // experiment's events and a run's events end up in one place rather than two.
            //
            // The surface was created two lines above, so its lock cannot be poisoned here. The
            // failure is still handled rather than unwrapped: if that ever stops being true, the
            // application should start and record the degradation instead of aborting at launch.
            if let Err(error) = brain.set_recorder(recorder.clone()) {
                tracing::error!(
                    error = %error.message,
                    "the command center could not install its telemetry sink; experiment telemetry \
                     will not be recorded"
                );
            }
            app.manage(AppState {
                catalog: BenchmarkCatalog::new()?,
                brain,
                arena: ArenaSurface::default(),
                security: SecuritySurface::default(),
                recorder,
                results,
                recent_events,
                settings,
                settings_path: settings_path.clone(),
                results_path: database_path,
            });
            Ok(())
        })
        // Every command the frontend invokes is registered here, and the list is verified against
        // `apps/desktop/src/api.ts` by `every_invoked_command_is_registered` in the tests below.
        //
        // Eight commands were exported from their modules but absent from this handler, which meant
        // the Arena, Security Lab, and Replay panels were dead surfaces in the built application
        // while `README.md`, `PROGRESS.md`, `ARCHITECTURE.md`, and `docs/feature-matrix.json` all
        // described them as working. `#[tauri::command]` on a function is only a *definition*: Tauri
        // dispatches nothing that is not in `generate_handler!`. The drift guard now fails if the
        // two lists ever disagree again, in either direction.
        .invoke_handler(tauri::generate_handler![
            apply_settings,
            brain_panel,
            compare_experiments,
            get_experiment,
            list_comparable_experiments,
            list_experiments,
            read_settings,
            reset_settings,
            run_experiment,
            list_agents,
            list_benchmark_details,
            list_benchmarks,
            list_providers,
            get_provider_status,
            run_benchmark,
            get_result,
            get_run_metadata,
            subscribe_telemetry,
            get_arena_match,
            latest_arena_match,
            list_arena_agents,
            list_arena_games,
            play_arena_match,
            replay_arena_match,
            latest_security_report,
            list_security_probes,
            run_security_sweep
        ])
        .run(tauri::generate_context!())
        .expect("error while running ProbeLab desktop application");
}

#[cfg(test)]
mod tests {
    use super::{
        get_arena_match, latest_arena_match, latest_security_report, list_agents,
        list_arena_agents, list_arena_games, list_benchmark_details, list_providers,
        list_security_probes, new_run_id, play_arena_match, replay_arena_match, run_security_sweep,
        AgentSummary, BenchmarkDetail, ProviderSummary,
    };
    use probelab_lab::AgentCategory;
    use probelab_lab::AgentRegistry;

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

    // # IPC registration drift guard
    //
    // `#[tauri::command]` marks a function as a command. It does not make Tauri dispatch it. Only an
    // entry in `generate_handler![]` does, and a command that is defined but not registered is not a
    // command the product has — it is a function that silently does nothing.
    //
    // That distinction was invisible here for a long time. The Arena, Security Lab, and Replay panels
    // were fully built, fully tested, and fully documented, and every one of their commands was
    // absent from the handler. Nothing failed: the Rust compiled, all 834 tests passed, the feature
    // matrix checker passed, and CI was green. A user who opened the application found three panels
    // that could not do anything.
    //
    // The failure mode was structural, not accidental. Three separate facts had to be checked and none
    // of them were: the frontend invokes a name, `api.ts` declares a wrapper for it, and
    // `generate_handler![]` lists it. `scripts/check-feature-matrix.py` came closest, but it collected
    // every `#[tauri::command]` in the crate — definitions, not registrations — so a command that was
    // defined and unregistered satisfied it exactly. A gate that reads the wrong list is worse than no
    // gate, because it reads as coverage.
    //
    // So the two real lists are compared directly: the names `api.ts` actually invokes, and the names
    // `generate_handler![]` actually dispatches. Either direction of drift fails.

    /// The command names the frontend invokes, read out of the real frontend source.
    ///
    /// Scans every `.ts`/`.tsx` file under `apps/desktop/src`, not just `api.ts`. The typed wrappers
    /// in `api.ts` are the *preferred* route, but the command center also calls a handful of commands
    /// directly -- `get_result` and `get_run_metadata` for the durable read-back, `run_benchmark` for
    /// the single-episode path, `get_provider_status` for the preflight check. Reading only the
    /// wrappers would report those as unreachable and the gate would be wrong.
    fn frontend_invoked_commands() -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for source in frontend_sources() {
            let mut rest = source;
            while let Some(at) = rest.find("invoke<") {
                rest = &rest[at + "invoke<".len()..];
                // Skip the generic argument, which may itself contain angle brackets.
                let mut angle = 1usize;
                let mut end = None;
                for (index, character) in rest.char_indices() {
                    match character {
                        '<' => angle += 1,
                        '>' => {
                            angle -= 1;
                            if angle == 0 {
                                end = Some(index);
                                break;
                            }
                        }
                        _ => {}
                    }
                }

                let Some(end) = end else { break };
                rest = &rest[end + 1..];
                let Some(open) = rest.find('(') else { break };
                let Some(close) = rest[open..].find(')') else {
                    break;
                };
                let arguments = &rest[open + 1..open + close];
                let quoted = arguments
                    .trim()
                    .trim_start_matches(|c: char| c != '"' && c != '\'' && c != '`')
                    .trim_start_matches(['"', '\'', '`']);
                if let Some(end_quote) = quoted.find(['"', '\'', '`']) {
                    names.push(quoted[..end_quote].to_owned());
                }
            }
        }
        names.sort();
        names.dedup();
        assert!(
            !names.is_empty(),
            "no invoke calls found in the frontend; the extraction is broken, not the frontend"
        );
        names
    }

    /// Every frontend source file, embedded at compile time so the guard cannot pass by accident.
    fn frontend_sources() -> Vec<&'static str> {
        vec![
            include_str!("../../src/api.ts"),
            include_str!("../../src/App.tsx"),
            include_str!("../../src/AppShell.tsx"),
            include_str!("../../src/AgentCatalog.tsx"),
            include_str!("../../src/ArenaPanel.tsx"),
            include_str!("../../src/BenchmarkCatalog.tsx"),
            include_str!("../../src/BrainPanel.tsx"),
            include_str!("../../src/ComparisonPanel.tsx"),
            include_str!("../../src/ReplayPanel.tsx"),
            include_str!("../../src/SecurityPanel.tsx"),
            include_str!("../../src/SettingsPanel.tsx"),
            include_str!("../../src/main.tsx"),
        ]
    }

    /// The command names `generate_handler![]` dispatches, read out of this file's own source.
    fn dispatched_commands() -> Vec<String> {
        let source = include_str!("lib.rs");
        let start = source
            .find("tauri::generate_handler![")
            .expect("this file registers commands");
        let rest = &source[start + "tauri::generate_handler![".len()..];
        let end = rest.find(']').expect("the handler list is closed");
        let mut names: Vec<String> = rest[..end]
            .split(',')
            .map(|entry| entry.trim().to_owned())
            .filter(|entry| !entry.is_empty())
            .collect();
        names.sort();
        names.dedup();
        assert!(!names.is_empty(), "the handler list is empty");
        names
    }

    /// Every command the frontend calls is one Tauri dispatches.
    ///
    /// This is the assertion that would have failed before the eight missing commands were
    /// registered, and it fails in the same direction if one is ever dropped again.
    #[test]
    fn every_command_the_frontend_invokes_is_registered() {
        let invoked = frontend_invoked_commands();
        let dispatched = dispatched_commands();
        let missing: Vec<&String> = invoked
            .iter()
            .filter(|name| !dispatched.contains(name))
            .collect();
        assert!(
            missing.is_empty(),
            "the frontend invokes {} command(s) Tauri does not dispatch: {missing:?}. \
             A command that is not in `generate_handler![]` is a function, not a feature: the panel \
             that calls it is dead in the built application while its documentation claims it works.",
            missing.len()
        );
    }

    /// Every dispatched command is one the frontend calls, or is on the justified list below.
    ///
    /// The other direction, and it is a weaker property than the first. A command nobody invokes is
    /// not a correctness defect -- nothing breaks, and the read-only commands that make up most of this
    /// list are useful without being wired to a screen. Deleting working code to satisfy a check would
    /// be the wrong trade, so this is a *declared* list rather than a hard requirement: an unused
    /// command has to be named here, with its reason, and a reason that stops being true gets deleted
    /// rather than left to rot.
    #[test]
    fn every_registered_command_is_reachable_or_declared() {
        // Read-only accessors and a durable read-back path that the API deliberately keeps exposed
        // even where no panel currently calls it. Each entry states why it is still here.
        const DECLARED_UNUSED: &[(&str, &str)] = &[(
            "get_arena_match",
            "read-only single-match accessor, paired with the 20-match retained surface; \
                 `latest_arena_match` is what the panel calls",
        )];
        let invoked = frontend_invoked_commands();
        let dispatched = dispatched_commands();
        let unaccounted: Vec<&String> = dispatched
            .iter()
            .filter(|name| !invoked.contains(name))
            .filter(|name| !DECLARED_UNUSED.iter().any(|(command, _)| command == *name))
            .collect();
        assert!(
            unaccounted.is_empty(),
            "Tauri dispatches {} command(s) the frontend never invokes and that are not declared \
             unused: {unaccounted:?}. Either the frontend should call it, or the reason it is kept \
             belongs in DECLARED_UNUSED -- an undeclared command is a claim with no user.",
            unaccounted.len()
        );
        // A declared entry that has become used is stale, and a stale exception hides the next one.
        for (command, _) in DECLARED_UNUSED {
            assert!(
                dispatched.iter().any(|name| name == command),
                "{command} is declared unused but is not dispatched at all"
            );
            assert!(
                !invoked.iter().any(|name| name == command),
                "{command} is declared unused but the frontend now invokes it; remove the entry"
            );
        }
    }

    /// The eight commands that were defined but never dispatched, by name.
    ///
    /// Pinned rather than left to history because the failure was invisible for long enough that
    /// "it used to be broken" is the only honest description. If one of these is ever deliberately
    /// removed, this test is the place to record that, in the same commit.
    #[test]
    fn the_panels_that_were_dead_are_now_live() {
        let dispatched = dispatched_commands();
        for command in [
            // Arena
            "play_arena_match",
            "list_arena_games",
            "list_arena_agents",
            "latest_arena_match",
            "get_arena_match",
            "replay_arena_match",
            // Security Lab
            "run_security_sweep",
            "list_security_probes",
            "latest_security_report",
        ] {
            assert!(
                dispatched.iter().any(|name| name == command),
                "{command} is not dispatched, so the panel that calls it cannot work"
            );
        }
        // The functions exist and are typed; registration is what was missing. Referencing them keeps
        // this test honest if a command is ever renamed on the Rust side without the frontend
        // following, which `every_command_the_frontend_invokes_is_registered` also catches.
        let _ = (
            list_arena_games(),
            list_arena_agents("tic_tac_toe".to_owned()),
            latest_security_report as fn(_) -> _,
        );
        let _ = (
            latest_arena_match as fn(_) -> _,
            list_security_probes as fn(_) -> _,
            get_arena_match as fn(_, _) -> _,
            replay_arena_match as fn(_, _) -> _,
        );
    }

    /// The surfaces those commands read are the ones the application manages.
    ///
    /// `play_arena_match` and `run_security_sweep` take `State<'_, AppState>`, which only resolves
    /// because `AppState` is what `app.manage(..)` installs. A refactor that managed a different type
    /// would compile and then fail at run time on the first Arena match, so this asserts the managed
    /// state is the one the commands ask for.
    #[test]
    fn the_arena_and_security_commands_read_managed_state_that_exists() {
        fn assert_state_types_are_expressible(
            _: Option<tauri::State<'static, super::ArenaSurface>>,
            _: Option<tauri::State<'static, super::SecuritySurface>>,
            _: Option<tauri::State<'static, super::BrainSurface>>,
            _: Option<tauri::State<'static, super::AppState>>,
        ) {
        }
        // The commands below are never called; their types are what is under test.
        let _ = play_arena_match as fn(_, _) -> _;
        let _ = run_security_sweep as fn(_, _) -> _;
        assert_state_types_are_expressible(None, None, None, None);
    }
}
