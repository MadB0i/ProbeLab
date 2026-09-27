//! The command center's brain surface: typed commands over the real orchestrator.
//!
//! # Why this exists
//!
//! The library had a Brain, a runner, and a read model, and the application used none of them — the
//! Tauri layer called `BenchmarkRunner` directly for a single episode, so the Brain was never
//! executing anything and the command center had nothing to show. These commands are the missing edge.
//!
//! # The one thing this module must never do
//!
//! Authorize anything. There is no command here that grants, denies, overrides, or relaxes a
//! capability decision, and there is deliberately no "force" or "override" command, because the moment
//! one exists the Brain panel becomes a way to pressure the enforcement layers. If a capability was
//! refused, the panel reports that fact; it cannot undo it.
//!
//! Commands are read-mostly on purpose: `brain_panel` and `get_experiment` are pure reads, and
//! `run_experiment` starts work rather than steering it.

use std::sync::{Arc, Mutex};

use probelab_evaluator::GridWorldEvaluator;
use probelab_lab::{
    BrainReadModel, BrainState, ExperimentObserver, LabBrain, LabEvent, LabRunner, PlanRequest,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use super::new_run_id;
use crate::AppState;

/// The event emitted whenever the Brain's view changes.
///
/// One event, carrying the whole read model, rather than a stream of partial patches. The panel then
/// holds exactly what the backend believes, and there is no client-side accumulation logic that could
/// drift from it. A run of five episodes produces a handful of these, not hundreds — the per-episode
/// detail travels inside the model as the timeline.
pub const BRAIN_EVENT: &str = "probe-lab/brain";

/// A command-center request to run an experiment.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentRequest {
    /// The benchmark to run, as an agent-registry identifier.
    pub benchmark_id: String,
    /// The agent to evaluate.
    pub agent_id: String,
    /// How many episodes.
    pub episodes: usize,
    /// The root seed. `None` lets the planner choose.
    pub root_seed: Option<u64>,
}

/// A failure the command center can display.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrainCommandError {
    /// A stable code for the UI to branch on without matching prose.
    pub code: String,
    /// A concise technical description, safe to show a developer.
    pub message: String,
}

impl BrainCommandError {
    fn planning(message: impl Into<String>) -> Self {
        Self {
            code: "planning_failed".to_owned(),
            message: message.into(),
        }
    }

    fn execution(message: impl Into<String>) -> Self {
        Self {
            code: "execution_failed".to_owned(),
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: "not_found".to_owned(),
            message: message.into(),
        }
    }
}

/// Forwards every runner event to the command center as it happens.
///
/// This is the whole live-update mechanism, and it is deliberately that small. The runner already
/// records every event; this type does nothing but hand those same events to the emitter, so the panel
/// cannot show a stream that differs from the run's own record.
struct EmittingObserver {
    app: AppHandle,
}

impl ExperimentObserver for EmittingObserver {
    fn on_event(&self, event: &LabEvent) {
        if let Err(error) = self.app.emit("probe-lab/brain-event", event.clone()) {
            tracing::warn!(error = %error, "failed to emit a brain event");
        }
    }
}

/// Returns the Brain's current read model, or the idle model when it has never run.
///
/// Never fails. A Brain with nothing to report is a real state, and returning an error for it would
/// make the panel's most common initial state look like a fault.
#[tauri::command(rename_all = "snake_case")]
pub fn brain_panel(surface: State<'_, BrainSurface>) -> BrainReadModel {
    surface.panel()
}

/// Returns one experiment's read model by identifier.
#[tauri::command(rename_all = "snake_case")]
pub fn get_experiment(
    surface: State<'_, BrainSurface>,
    experiment_id: String,
) -> Result<BrainReadModel, BrainCommandError> {
    surface.experiment(&experiment_id).ok_or_else(|| {
        BrainCommandError::not_found(format!("no experiment named {experiment_id:?}"))
    })
}

/// Lists experiment identifiers, newest first, so a finished run stays reachable after it ends.
#[tauri::command(rename_all = "snake_case")]
pub fn list_experiments(surface: State<'_, BrainSurface>) -> Vec<String> {
    surface.experiment_ids()
}

/// Plans and executes one experiment, emitting the Brain's view as it progresses.
///
/// Runs on a blocking worker because the runner is synchronous and a multi-episode experiment
/// outlives any reasonable synchronous call. The command returns the final read model; intermediate
/// updates arrive as events.
#[tauri::command(rename_all = "snake_case")]
pub async fn run_experiment(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ExperimentRequest,
) -> Result<BrainReadModel, BrainCommandError> {
    let surface = state.brain.clone();
    let run_id = new_run_id().map_err(|error| BrainCommandError::planning(error.to_string()))?;
    let experiment_id = run_id.clone();

    // Announce the start before any work, so the panel can move out of idle immediately rather than
    // appearing to hang. The model is assembled from the request alone, so it contains nothing that
    // has not happened yet.
    let mut brain = LabBrain::new();
    let plan_request = PlanRequest {
        benchmark_id: Some(request.benchmark_id.clone()),
        target_agent: Some(request.agent_id.clone()),
        probe_agents: Vec::new(),
        episodes: Some(request.episodes),
        root_seed: request.root_seed,
    };
    // A failure here means the panel cannot show the run in progress. It does *not* mean the run
    // cannot happen: the durable record below is what recovery depends on, and the surface is only a
    // cache. Refusing to run would trade a real measurement for a cosmetic one.
    if let Err(error) = surface.set_active(experiment_id.clone()) {
        tracing::error!(error = %error.message, "the command center could not show this run as active");
    }

    // Persist `running` *before* the first episode. This row is what makes crash recovery possible: if
    // the process dies mid-run, this is the evidence the next start uses to conclude the run is
    // interrupted rather than completed. Writing it only at the end would leave a crash invisible.
    let results = state.results.clone();
    let running_id = run_id.clone();
    let running_benchmark = request.benchmark_id.clone();
    let running_agent = request.agent_id.clone();
    let running_episodes = request.episodes;
    // The plan's seed, kept for the comparison feature: a comparison can only hold two runs
    // constant if it knows what was held constant, and the result bundle records the model seed
    // rather than the plan's root seed.
    let request_root_seed = request.root_seed;
    if let Err(error) = results.put_experiment(&probelab_telemetry::ExperimentRecord {
        experiment_id: running_id,
        benchmark_id: running_benchmark,
        lifecycle: probelab_telemetry::ExperimentLifecycle::Running,
        terminal_reason: None,
        episodes_requested: running_episodes,
        episodes_completed: 0,
        episodes_failed: 0,
        has_score: false,
        score: None,
        result_json: None,
        updated_unix_ms: 0,
        target_agent: Some(running_agent.clone()),
        root_seed: request_root_seed,
    }) {
        // Failing to record the running state does not stop the run; it does mean a crash during it
        // cannot be detected, which is worth saying out loud rather than only in a log.
        tracing::error!(
            error = %error,
            "could not persist the running experiment state; a crash during this run will not be \
             detected as an interruption"
        );
    }

    let observer: Arc<dyn ExperimentObserver> = Arc::new(EmittingObserver { app: app.clone() });
    let runner = LabRunner::new().with_observer(observer);
    let surface_handle = surface.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        runner
            .run(
                &mut brain,
                &plan_request,
                &experiment_id,
                &GridWorldEvaluator::new(),
                surface_handle.recorder(),
            )
            .map(|run| BrainReadModel::from_run(&run, BrainState::Completed, "idle"))
    })
    .await
    .map_err(|error| {
        BrainCommandError::execution(format!("the experiment worker stopped: {error}"))
    })?
    .map_err(|error| BrainCommandError::planning(error.to_string()))?;

    // The experiment is finished and durable by this point. If the in-memory panel cannot be updated,
    // say so and return the real result: discarding a completed measurement because a cache was
    // poisoned would be the wrong trade in both directions.
    if let Err(error) = surface.store(run_id.clone(), result.clone()) {
        tracing::error!(error = %error.message, "the command center could not retain the finished experiment");
    }

    // Persist the terminal state. A run that reached here is genuinely finished, so this is the one
    // place `completed` is written -- and it is written from the runner's own terminal reason rather
    // than from the absence of an error.
    if let Some(experiment) = result.experiment.as_ref() {
        // `to_wire_json`, not `format!("{reason:?}").to_lowercase()`. The `Debug` rendering stored
        // `containmentfailure { kind: verificationfailed }` in a TEXT column -- a Rust expression
        // embedded in stored data, which no serde configuration could parse back, and which disagreed
        // with the serde spelling `wire_format.rs` pins. This derives from the same derive the wire
        // format uses, so a rename moves both.
        let reason_label = experiment
            .terminal_reason
            .as_ref()
            .map(probelab_lab::TerminalReason::to_wire_json);
        let terminal = match experiment.terminal_reason {
            Some(probelab_lab::TerminalReason::Completed) => {
                probelab_telemetry::ExperimentLifecycle::Completed
            }
            _ => probelab_telemetry::ExperimentLifecycle::Failed,
        };
        if let Err(error) = results.put_experiment(&probelab_telemetry::ExperimentRecord {
            experiment_id: run_id.clone(),
            benchmark_id: experiment.benchmark_id.clone(),
            lifecycle: terminal,
            terminal_reason: reason_label,
            episodes_requested: experiment.episodes_requested,
            episodes_completed: experiment.episodes_completed,
            episodes_failed: experiment.episodes_failed,
            has_score: experiment.mean_success_fraction.is_some(),
            score: experiment.mean_success_fraction,
            result_json: None,
            updated_unix_ms: 0,
            // Re-stated rather than re-read: the running row wrote them, and carrying the same
            // values forward keeps the terminal write from blanking the identity the comparison
            // needs. A terminal row with a null agent would refuse every comparison it took part in.
            target_agent: Some(running_agent),
            root_seed: request_root_seed,
        }) {
            tracing::error!(error = %error, "could not persist the terminal experiment state");
        }
    }
    if let Err(error) = app.emit(BRAIN_EVENT, result.clone()) {
        tracing::warn!(error = %error, "failed to emit the final brain model");
    }
    Ok(result)
}

/// The Brain's state, plus the experiment it is supervising.
///
/// A `Mutex<Option<BrainReadModel>>` rather than a channel or an event log, because the panel needs to
/// be able to ask "what is true right now" as well as "tell me when it changes". A channel alone would
/// make a reload show an empty panel even though a run had already finished.
#[derive(Clone, Default)]
pub struct BrainSurface {
    current: Arc<Mutex<Option<BrainReadModel>>>,
    history: Arc<Mutex<Vec<(String, BrainReadModel)>>>,
    recorder: Arc<Mutex<Option<Arc<dyn probelab_core::TelemetryRecorder>>>>,
}

impl BrainSurface {
    /// The current model, or idle.
    ///
    /// Degrades to idle on a poisoned lock. `brain_panel` is the most frequently called command in the
    /// application, so panicking here on a fault that some other thread caused would take down the
    /// command center permanently. Idle is the honest state to report when the model cannot be read,
    /// and the panel already renders it.
    pub fn panel(&self) -> BrainReadModel {
        self.current
            .lock()
            .ok()
            .and_then(|current| current.clone())
            .unwrap_or_else(BrainReadModel::idle)
    }

    /// One stored experiment.
    pub fn experiment(&self, experiment_id: &str) -> Option<BrainReadModel> {
        self.history
            .lock()
            .ok()?
            .iter()
            .find(|(id, _)| id == experiment_id)
            .map(|(_, model)| model.clone())
    }

    /// Stored experiment identifiers, newest first.
    pub fn experiment_ids(&self) -> Vec<String> {
        self.history
            .lock()
            .map(|history| history.iter().rev().map(|(id, _)| id.clone()).collect())
            .unwrap_or_default()
    }

    /// Records a finished experiment as the current one.
    ///
    /// Reports on failure instead of panicking. A run that completed is real work with a real result;
    /// losing the ability to show it in the panel is an availability problem, not a reason to abort the
    /// process. The durable store is unaffected either way -- this surface is a cache, not the record.
    pub fn store(
        &self,
        experiment_id: String,
        model: BrainReadModel,
    ) -> Result<(), BrainCommandError> {
        let surface_unavailable = || {
            BrainCommandError::execution(
            "the command center's state was left locked by an earlier fault and cannot be updated; \
             the experiment finished and is still stored durably, but this panel will not show it",
        )
        };
        *self.current.lock().map_err(|_| surface_unavailable())? = Some(model.clone());
        let mut history = self.history.lock().map_err(|_| surface_unavailable())?;
        // Bounded so a long session cannot grow without limit. Twenty is far more than anyone scrolls
        // back through, and a forgotten experiment is better than a slow UI.
        if history.len() >= 20 {
            history.remove(0);
        }
        history.push((experiment_id, model));
        Ok(())
    }

    /// Marks an experiment as in progress so the panel is not idle while it runs.
    pub fn set_active(&self, experiment_id: String) -> Result<(), BrainCommandError> {
        let mut current = self.current.lock().map_err(|_| {
            BrainCommandError::execution(
                "the command center's state was left locked by an earlier fault; the run can start \
                 but this panel will not show it in progress",
            )
        })?;
        let mut model = current.clone().unwrap_or_else(BrainReadModel::idle);
        model.brain_state = BrainState::Running;
        model.next_intent = format!("executing {experiment_id}");
        // Deliberately leaves `experiment` as-is rather than inventing a partial experiment view. A
        // run in progress has no results, and a panel that showed zero episodes completed and a
        // fabricated 0% would be lying during exactly the window a user is most likely to look.
        *current = Some(model);
        Ok(())
    }

    /// Installs the recorder the runner should use.
    pub fn set_recorder(
        &self,
        recorder: Arc<dyn probelab_core::TelemetryRecorder>,
    ) -> Result<(), BrainCommandError> {
        *self.recorder.lock().map_err(|_| {
            BrainCommandError::execution(
                "the command center's recorder slot was left locked by an earlier fault; telemetry \
                 cannot be recorded for this session",
            )
        })? = Some(recorder);
        Ok(())
    }

    /// The installed recorder, or a no-op one.
    ///
    /// A no-op rather than an error, because a command center with no telemetry sink should still be
    /// able to run an experiment; the observer's own event stream is what the panel reads. This is
    /// also the path that protects a poisoned recorder slot from crashing a run that has otherwise
    /// succeeded -- losing telemetry degrades the record, aborting the process loses everything.
    fn recorder(&self) -> Arc<dyn probelab_core::TelemetryRecorder> {
        self.recorder
            .lock()
            .ok()
            .and_then(|recorder| recorder.clone())
            .unwrap_or_else(|| Arc::new(DiscardingRecorder))
    }
}

/// A recorder that discards everything, used when no sink is installed.
struct DiscardingRecorder;

impl probelab_core::TelemetryRecorder for DiscardingRecorder {
    fn record(&self, _event: &probelab_core::TelemetryEvent) -> Result<(), String> {
        Ok(())
    }
}

/// Asserts that this module exposes no way to steer an authorization decision.
///
/// A structural test in the spirit of the runner's. The dangerous shape for a command surface is a
/// command whose name contains "force", "override", "allow", or "bypass", so this checks the command
/// names and the exported functions directly. If someone later adds a way for the panel to pressure
/// the enforcement layers, this fails.
#[cfg(test)]
mod tests {
    use super::*;
    use probelab_lab::{EventSource, Severity};

    /// The part of this file that is actual command surface, with the test module removed.
    ///
    /// These tests name the strings they forbid, so a test that scanned the whole file would always
    /// find them — in itself. Truncating at the test marker is what makes the assertion about the
    /// code rather than about the assertion.
    const SURFACE_SOURCE: &str = include_str!("brain.rs");

    fn command_surface_source() -> &'static str {
        SURFACE_SOURCE
            .split("#[cfg(test)]")
            .next()
            .expect("the file has content before its test module")
    }

    #[test]
    fn no_command_can_override_an_authorization_decision() {
        let source = command_surface_source();
        for forbidden in [
            "fn force",
            "fn override_",
            "fn bypass",
            "fn allow_capability",
            "fn grant",
            "fn deny_capability",
            "fn set_policy",
        ] {
            assert!(
                !source.contains(forbidden),
                "the brain command surface contains {forbidden:?}; the panel must never be able to \
                 steer an authorization decision"
            );
        }
    }

    #[test]
    fn the_brain_surface_holds_no_authorization_handle() {
        // The surface stores read models. It must not be able to reach a broker, a policy, or a
        // capability type even indirectly through a stored value.
        let source = command_surface_source();
        for forbidden in ["CapabilityBroker", "PolicyDecision", "RuntimeAuthorization"] {
            assert!(
                !source.contains(forbidden),
                "the brain surface contains {forbidden:?}; it stores views, not authority"
            );
        }
    }

    #[test]
    fn the_idle_surface_reports_idle_rather_than_failing() {
        let surface = BrainSurface::default();
        let panel = surface.panel();
        assert_eq!(panel.experiment, None);
        assert_eq!(panel.observed_events, 0);
        assert!(surface.experiment_ids().is_empty());
        assert!(surface.experiment("nope").is_none());
    }

    #[test]
    fn a_stored_experiment_is_retrievable_after_it_ends() {
        let surface = BrainSurface::default();
        let mut model = BrainReadModel::idle();
        model.brain_state = BrainState::Completed;
        surface.store("exp-1".to_owned(), model).expect("store");
        assert!(surface.experiment("exp-1").is_some());
        assert_eq!(surface.experiment_ids(), vec!["exp-1".to_owned()]);
        assert_eq!(surface.panel().brain_state, BrainState::Completed);
    }

    #[test]
    fn the_history_is_bounded_so_a_session_cannot_grow_without_limit() {
        let surface = BrainSurface::default();
        for index in 0..25 {
            surface
                .store(format!("exp-{index}"), BrainReadModel::idle())
                .expect("store");
        }
        let ids = surface.experiment_ids();
        assert!(
            ids.len() <= 20,
            "history grew unbounded: {} entries",
            ids.len()
        );
    }

    #[test]
    fn a_newest_first_listing_puts_the_latest_run_first() {
        let surface = BrainSurface::default();
        surface
            .store("first".to_owned(), BrainReadModel::idle())
            .expect("store");
        surface
            .store("second".to_owned(), BrainReadModel::idle())
            .expect("store");
        assert_eq!(
            surface.experiment_ids(),
            vec!["second".to_owned(), "first".to_owned()]
        );
    }

    #[test]
    fn an_active_experiment_does_not_invent_a_partial_result() {
        let surface = BrainSurface::default();
        surface
            .set_active("exp-active".to_owned())
            .expect("activate");
        let panel = surface.panel();
        assert_eq!(panel.brain_state, BrainState::Running);
        assert_eq!(
            panel.experiment, None,
            "a run in progress has no results, so the panel must not show an experiment view for it"
        );
        assert_eq!(panel.security.integrity_violations, 0);
        assert_eq!(panel.security.broker_refusals, 0);
    }

    #[test]
    fn an_event_serialization_never_carries_a_credential() {
        let event = LabEvent::new(
            0,
            "runner.episode_result",
            EventSource::Telemetry,
            Severity::Info,
            "episode 0: task success 1",
        );
        let json = serde_json::to_string(&event).expect("serialize");
        for marker in ["sk-", "AIza", "api_key\":", "password"] {
            assert!(!json.contains(marker), "an event leaked {marker}");
        }
    }

    /// A poisoned lock is a runtime condition, and the command center must survive it.
    ///
    /// This is the most consequential of the three surfaces, because `brain_panel` is the most
    /// frequently polled command in the application and `recorder()` is read on the path that starts
    /// every experiment. Panicking on either would mean one fault anywhere in the process takes the
    /// whole command center down permanently, with every later acquisition panicking too.
    ///
    /// The chosen degradations are the honest ones: reads report idle, writes refuse with a typed
    /// error, and a lost telemetry slot becomes a discarding recorder rather than a dead run. A
    /// measurement that completed must not be thrown away because a display cache was poisoned.
    #[test]
    fn a_poisoned_brain_surface_returns_errors_instead_of_crashing() {
        use std::sync::Mutex;

        let surface = BrainSurface::default();
        // The guard must be acquired inside the unwinding closure. A mutex is poisoned when a guard is
        // dropped during unwinding, so a guard held in the enclosing scope is released normally and the
        // lock is never poisoned -- a test that looks like it exercises poisoning and does not is worse
        // than no test.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = surface.current.lock().expect("lock to poison");
            panic!("a thread that panicked while holding the brain surface lock");
        }));
        assert!(outcome.is_err(), "the poisoning panic did not unwind");
        assert!(
            surface.current.lock().is_err(),
            "the lock should be poisoned; the test is not exercising what it claims"
        );

        // A read degrades to the idle model rather than panicking. The idle model is
        // `BrainState::Planning`, which is what the panel already shows before any run.
        let panel = surface.panel();
        assert_eq!(panel.brain_state, BrainState::Planning);
        assert!(panel.experiment.is_none());
        // A write is a typed refusal. `store` writes `current` first, so this is the path that fails.
        let error = surface
            .store("exp-1".to_owned(), BrainReadModel::idle())
            .expect_err("a write must be a typed refusal, not a panic");
        assert!(!error.code.is_empty());
        assert!(!error.message.is_empty());
        // And the surface is still answering, which is the whole point.
        assert_eq!(surface.panel().brain_state, BrainState::Planning);
        let _ = Mutex::new(());
    }

    /// A poisoned recorder slot degrades to discarding telemetry rather than failing a run.
    ///
    /// The alternative -- refusing to run because the telemetry slot is stuck -- would trade a
    /// degraded record for no record at all, and the observer's own event stream is what the panel
    /// actually reads.
    #[test]
    fn a_poisoned_recorder_slot_degrades_instead_of_failing_a_run() {
        let surface = BrainSurface::default();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = surface.recorder.lock().expect("lock to poison");
            panic!("a thread that panicked while holding the recorder slot");
        }));
        assert!(outcome.is_err(), "the poisoning panic did not unwind");
        assert!(
            surface.recorder.lock().is_err(),
            "the slot should be poisoned"
        );

        let recorder = surface.recorder();
        // Usable: recording a real event through it succeeds rather than panicking. The event is one the

        // engine genuinely produced, captured through a recorder installed in the runner, rather than a
        // hand-built literal -- this test is about lock behaviour, and an assembled event would be a
        // second thing to keep in sync with the wire format.
        let event = a_real_telemetry_event();
        recorder
            .record(&event)
            .expect("a discarding recorder accepts events");

        let error = surface
            .set_recorder(Arc::new(DiscardingRecorder))
            .expect_err("installing over a poisoned slot must be a typed refusal");
        assert!(!error.message.is_empty());
    }

    /// One genuine telemetry event, captured from a real run.
    ///
    /// `TelemetryEvent` has no public constructor and no `Default`, deliberately: its field set is a
    /// wire format that `crates/lab/tests/wire_format.rs` pins. Writing a literal here would create a
    /// second copy of that field list, free to drift. So this runs an actual experiment through the same
    /// `LabRunner` the product uses and keeps the first event recorded.
    fn a_real_telemetry_event() -> probelab_core::TelemetryEvent {
        use std::sync::Mutex as StdMutex;

        /// Keeps the first event it is handed.
        #[derive(Clone)]
        struct CaptureOnce(Arc<StdMutex<Option<probelab_core::TelemetryEvent>>>);
        impl probelab_core::TelemetryRecorder for CaptureOnce {
            fn record(&self, event: &probelab_core::TelemetryEvent) -> Result<(), String> {
                let mut slot = self
                    .0
                    .lock()
                    .map_err(|_| "capture lock poisoned".to_owned())?;
                if slot.is_none() {
                    *slot = Some(event.clone());
                }
                Ok(())
            }
        }

        let slot = Arc::new(StdMutex::new(None));
        let mut brain = probelab_lab::LabBrain::new();
        let plan = probelab_lab::PlanRequest {
            benchmark_id: Some("grid-world".to_owned()),
            target_agent: Some("optimal".to_owned()),
            probe_agents: Vec::new(),
            episodes: Some(1),
            root_seed: Some(1),
        };
        probelab_lab::LabRunner::new()
            .run(
                &mut brain,
                &plan,
                "capture-run",
                &probelab_evaluator::GridWorldEvaluator::new(),
                Arc::new(CaptureOnce(slot.clone())),
            )
            .expect("one episode through the real runner");
        let captured = slot
            .lock()
            .expect("read back")
            .take()
            .expect("the engine recorded at least one event");
        captured
    }
}
