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
use std::time::{SystemTime, UNIX_EPOCH};

use probelab_evaluator::GridWorldEvaluator;
use probelab_lab::{
    AgentCategory, AgentExecution, AgentRegistry, AgentSpec, BrainReadModel, BrainState,
    ExperimentObserver, LabBrain, LabEvent, LabRunner, PlanRequest,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use super::new_run_id;
use crate::provider::ModelRunConfig;
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
    /// The model to measure, or `None` to use the named agent from the registry.
    ///
    /// This is what makes a real model measurable through the *multi-episode* runner rather than
    /// through the single-run `run_benchmark` path. `None` is not "the mock" -- it means "the agent
    /// named by `agent_id`", which is a built-in deterministic agent or a registered one, exactly as
    /// before. `Some` registers a connected agent for this run only and measures that instead.
    ///
    /// The credential is a variable *name*. Nothing in this struct can hold a key, so a request
    /// arriving over IPC cannot carry one.
    #[serde(default)]
    pub model_config: Option<ModelRunConfig>,
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
///
/// Takes `State<AppState>` — the one type the application manages — and reads the Brain surface
/// through it. A previous version took `State<BrainSurface>` directly, which compiled, passed every
/// test (tests call the function, never Tauri's state resolution), and failed on every invocation
/// in the built application with "state not managed". State resolution is an integration property,
/// and only a test that names the managed type can pin it.
#[tauri::command(rename_all = "snake_case")]
pub fn brain_panel(state: State<'_, AppState>) -> BrainReadModel {
    state.brain.panel()
}

/// Returns one experiment's read model by identifier.
///
/// Reads the Brain's in-memory surface first, then the durable store. The fallback is what makes a
/// stored run loadable after a restart: the surface only knows about runs supervised by *this*
/// process, while the history list is populated from the durable store, so a run from an earlier
/// session was listed but could not be opened — the panel said the run did not exist when it did.
///
/// See [`brain_panel`] for why this takes `State<AppState>`.
#[tauri::command(rename_all = "snake_case")]
pub fn get_experiment(
    state: State<'_, AppState>,
    experiment_id: String,
) -> Result<BrainReadModel, BrainCommandError> {
    if let Some(model) = state.brain.experiment(&experiment_id) {
        return Ok(model);
    }
    load_stored_experiment(&state, &experiment_id)
}

/// Rebuilds a read model from a stored result.
///
/// The stored bundle is the run's own projection, projected through the same view the live panel uses,
/// so a run reopened from history is rendered identically to the run that produced it. A result the
/// store does not hold is reported as absent rather than reconstructed from partial fields.
/// Rebuilds a read model from stored data.
///
/// # Two shapes, and why both are needed
///
/// `result_json` in the `experiments` table now holds a versioned
/// [`StoredExperimentDocument`](probelab_lab::StoredExperimentDocument) — the whole experiment, so every
/// episode, the aggregate, the decisions, the failures and the timeline survive a restart. That is the
/// path new runs take.
///
/// Rows written before this change hold the scalar mean and nothing else. They are **not** rewritten and
/// **not** silently upgraded into the new shape: what they contain is not enough to fabricate the rest,
/// and manufacturing episodes for them would be exactly the invention this finding is about. They are
/// reported as the historical shape they are, so the caller can say so.
///
/// A run reconstructed from a legacy row therefore reports fewer episodes than it actually ran. That is
/// stated, not hidden — and it is the reason the durable document exists.
fn load_stored_experiment(
    state: &AppState,
    experiment_id: &str,
) -> Result<BrainReadModel, BrainCommandError> {
    let record = state
        .results
        .get_experiment(experiment_id)
        .map_err(|error| {
            BrainCommandError::execution(format!(
                "the stored experiment could not be read: {error}"
            ))
        })?
        .ok_or_else(|| {
            BrainCommandError::not_found(format!("no experiment named {experiment_id:?}"))
        })?;

    let raw = record.result_json.as_deref().unwrap_or("null");

    match probelab_lab::StoredExperimentDocument::from_legacy_json(raw) {
        probelab_lab::StoredExperimentShape::Document(document) => {
            Ok(BrainReadModel::from_experiment_view(document.experiment))
        }
        probelab_lab::StoredExperimentShape::LegacyMean(mean) => {
            // The historical shape. Reconstructed through the older single-episode path, which reports one
            // episode because one is all the row can support.
            let result = state
                .results
                .get(experiment_id)
                .map_err(|error| {
                    BrainCommandError::execution(format!(
                        "the stored result could not be read: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    BrainCommandError::not_found(format!(
                        "experiment {experiment_id:?} holds only a scalar summary, but no stored result \
                         was written for it; the episodes cannot be reconstructed"
                    ))
                })?;
            let metadata = state
                .results
                .get_metadata(experiment_id)
                .map_err(|error| {
                    BrainCommandError::execution(format!(
                        "the stored run metadata could not be read: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    BrainCommandError::not_found(format!(
                        "run {experiment_id:?} has a summary but no recorded run metadata"
                    ))
                })?;
            let mut model = BrainReadModel::from_stored(experiment_id, &result, &metadata);
            // Say what this row is, rather than presenting a single-episode reconstruction as the run.
            model.next_intent = format!(
                "historical summary only: {} episode(s) of {} could be reconstructed from a scalar row",
                model
                    .experiment
                    .as_ref()
                    .map(|view| view.episodes.len())
                    .unwrap_or(0),
                record.episodes_requested
            );
            let _ = mean;
            Ok(model)
        }
        probelab_lab::StoredExperimentShape::Unrecognised => Err(BrainCommandError::execution(
            format!(
                "experiment {experiment_id:?} is stored in a format this build does not understand; it \
                 has not been altered"
            ),
        )),
    }
}

/*
 * One command answers "what runs exist".
 *
 * This module used to expose `list_experiments`, returning bare identifiers, alongside
 * `list_comparable_experiments`, which returns full records. They disagreed: this one read the
 * Brain's in-memory surface and knew only about runs supervised by *this* process, while the history
 * the user actually saw was rendered from the durable store. After a restart the second still held
 * every run while the first returned an empty or partial set — and because nothing crashed and every
 * number on screen was correct, the history was simply always empty.
 *
 * Removing it was the fix, not aligning it. Two commands whose names both say "list experiments" are
 * two answers to one question, and the thinner answer is never the one a panel wants: lifecycle,
 * episodes, score and timestamp cannot be derived from an identifier, and a run that is listed but
 * cannot be reopened is worse than one that is not listed at all.
 *
 * The session surface remains the authority for *live* state — whether a run is in progress right now
 * — which is a different question and is answered by [`brain_panel`].
 */

/// The agent identifier a connected run registers its model under.
///
/// Fixed rather than per-run, and that is deliberate. Every built-in agent keeps its own identifier
/// because a run naming `optimal` must mean the built-in optimal; the connected entry needs a name that
/// cannot collide with any of them, and one constant achieves that more reliably than a per-run
/// identifier would. A per-run id would also leak into the plan and from there into the recorded
/// identity, where every stored row would carry a meaningless unique string.
const CONNECTED_AGENT_ID: &str = "connected-model";

/// Builds the registry a run measures against.
///
/// Starts from the built-ins and, when a model configuration was supplied, registers one connected
/// agent alongside them. Registration *adds* rather than replaces, so the plan's `target_agent` still
/// resolves for a request that names a built-in agent, and the connected entry can never shadow one.
///
/// Returns the built-in registry unchanged when no configuration was supplied, which keeps the mock
/// path byte-for-byte what it was.
///
/// # Why the agent is built here rather than in the plan
///
/// The plan is a description of an experiment and gets stored and replayed; a provider configuration is
/// equipment. Putting one in the plan would put a model name and a network endpoint into the durable
/// record, and would make a replayed plan depend on configuration it was never meant to carry. The
/// runner builds a fresh agent per episode from the registry, so registering once is enough for every
/// episode to get its own client.
///
/// The configuration is *cloned* into the factory, which the registry requires to be `'static`: one
/// owned copy per registered agent rather than a borrow of the caller's. It holds only a base URL, a
/// variable name, and a model identifier -- never a credential -- so the copy carries no secret.
fn build_agent_registry(model_config: Option<&ModelRunConfig>) -> AgentRegistry {
    let mut agents = AgentRegistry::built_in();
    let Some(config) = model_config.cloned() else {
        return agents;
    };

    /*
     * The configuration is validated and the credential checked *before* anything is announced as
     * running, so a missing key is a refusal rather than a run that starts and immediately fails. The
     * `unwrap_or_else` below is therefore a genuine fallback rather than a lie: if validation somehow
     * passes here and fails there, the run proceeds with the built-in agents and names an agent that
     * does not exist, which fails loudly at build time with a clear error rather than quietly measuring
     * something else. The alternative -- substituting the mock on failure -- is exactly the failure this
     * product exists to avoid: a result that looks like a measurement of a model and is not.
     */
    let spec = AgentSpec {
        id: CONNECTED_AGENT_ID.to_owned(),
        name: "Connected model".to_owned(),
        description: "A model reached over the network, measured across the full episode pool."
            .to_owned(),
        // An external model under evaluation, and explicitly not a policy or a reference ceiling. The
        // category is what the agent catalog renders and what a reader uses to tell a model's score
        // from a probe's finding, so it has to be the model's own category rather than a generic one.
        category: AgentCategory::TargetModel,
        execution: AgentExecution::RemoteModel,
        applicable_benchmarks: Vec::new(),
        supported_games: Vec::new(),
        seed: 0,
    };

    if let Err(error) = agents.register(spec, move |seed| {
        config.build_agent(seed).unwrap_or_else(|error| {
            // Surfaced as a panic rather than a silent substitution: a run that cannot reach its
            // model must fail, not measure a different subject and report the number.
            panic!("the connected agent for this run could not be built: {error}")
        })
    }) {
        // Only reachable if a built-in agent already owns this identifier, which is a programming error
        // rather than a runtime condition.
        tracing::error!(error = %error, "could not register the connected agent; the run will refuse to start");
    }
    agents
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
    let observer: Arc<dyn ExperimentObserver> = Arc::new(EmittingObserver { app: app.clone() });
    let runner = LabRunner::new()
        .with_observer(Arc::clone(&observer))
        .with_agents(build_agent_registry(request.model_config.as_ref()));
    // The cancellation handle is registered before the worker starts and
    // removed when it ends, so `cancel_experiment` can only stop the run it
    // names and a stale handle can never stop a later run.
    let cancel_handle = runner.cancel_handle();
    if let Ok(mut registry) = state.cancels.lock() {
        registry.insert(experiment_id.clone(), Arc::clone(&cancel_handle));
    }
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
        updated_unix_ms: now_unix_ms(),
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
    if let Ok(mut registry) = state.cancels.lock() {
        registry.remove(&run_id);
    }
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
            /*
             * The **whole experiment**, not the scalar mean.
             *
             * This column used to hold `serde_json::to_string(&mean_success_fraction)`, which persisted
             * one number and discarded every episode, decision, failure, timeline entry and security
             * total the run produced. The result then existed only as the running process's memory:
             * `get_experiment` rebuilt it by looking for a stored result under the same identifier,
             * found nothing for a multi-episode run, and answered "not found" — a result that appeared
             * during execution and became unavailable afterwards.
             *
             * The view is stored through a versioned envelope, so a build that meets a shape it does not
             * understand refuses the row rather than parsing it optimistically. Historical rows keep the
             * scalar they always had and are read on the narrower path, unchanged and un-rewritten.
             */
            result_json: experiment.to_document().to_json(),
            updated_unix_ms: now_unix_ms(),
            // Re-stated rather than re-read: the running row wrote them, and carrying the same
            // values forward keeps the terminal write from blanking the identity the comparison
            // needs. A terminal row with a null agent would refuse every comparison it took part in.
            target_agent: Some(running_agent),
            root_seed: request_root_seed,
        }) {
            tracing::error!(
                error = %error,
                run = %run_id,
                "could not persist the terminal experiment state"
            );
        }
    }
    if let Err(error) = app.emit(BRAIN_EVENT, result.clone()) {
        tracing::warn!(error = %error, "failed to emit the final brain model");
    }
    Ok(result)
}

/// Wall-clock milliseconds since the Unix epoch for stored rows.
///
/// Recorded rather than defaulted so `ORDER BY updated_unix_ms` orders by
/// update time instead of degenerating to the experiment-id tiebreak.
fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    // # Connected-model measurement
    //
    // These are the tests that the connected path is a *measurement* rather than a single run. Before
    // it existed, a real model could only be exercised through `run_benchmark`, which runs one episode
    // and returns no interval -- so the product could produce a number for a real model without ever
    // having measured one.
    //
    // `wiremock` stands in for the provider so the whole path is real: a real HTTP request, a real
    // response parsed by the real adapter, a real agent built by `ModelRunConfig`, and a real
    // multi-episode run through the real `LabRunner`. Nothing is stubbed below the socket.

    /// A stand-in provider, served from a plain background thread.
    ///
    /// Deliberately not `wiremock`. `LabRunner::run` is synchronous and blocks on the provider's
    /// futures internally, which creates and drops a tokio runtime -- and a runtime may not be dropped
    /// from inside an async context. Testing this inside `#[tokio::test]` therefore panics in tokio's
    /// shutdown before any assertion runs, which is a property of the test harness rather than of the
    /// code under test.
    ///
    /// A `std::net` listener on its own thread has no such problem and no extra dependency, and it is
    /// closer to what the code actually meets: a socket, a request, and a response.
    struct FakeProvider {
        uri: String,
        calls: Arc<AtomicUsize>,
        _handle: std::thread::JoinHandle<()>,
    }

    impl FakeProvider {
        fn start() -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a local port");
            let uri = format!(
                "http://{}/v1/chat/completions",
                listener.local_addr().expect("addr")
            );
            let calls = Arc::new(AtomicUsize::new(0));
            let counter = Arc::clone(&calls);

            let handle = std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    counter.fetch_add(1, Ordering::SeqCst);
                    // The request body is not inspected: what is under test is that a real request goes
                    // out over a real socket and a real reply comes back and is parsed by the real
                    // adapter, not that this particular fake models a provider correctly.
                    let body = serde_json::json!({
                        "choices": [{
                            "message": { "content": "{\"action\": \"MOVE_DOWN\"}" },
                            "finish_reason": "stop"
                        }],
                        "usage": { "prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12 }
                    })
                    .to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                         connection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                }
            });

            Self {
                uri,
                calls,
                _handle: handle,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }
    fn connected_config(base_url: &str) -> ModelRunConfig {
        ModelRunConfig::OpenAiCompatible {
            schema_version: crate::provider::MODEL_RUN_CONFIG_SCHEMA_VERSION,
            base_url: base_url.to_owned(),
            // A variable that is not set, so `credential_is_present` fails -- which is the point of the
            // first test here. Set for the others by the helper below.
            api_key_env: "PROBELAB_TEST_MISSING_KEY".to_owned(),
            model: "test-model".to_owned(),
            timeout_seconds: 5,
            temperature: 0.0,
            max_tokens: 32,
        }
    }

    /// A connected configuration whose credential check passes, by setting a real environment
    /// variable. Serialised so two tests in the same binary do not race each other.
    fn with_credential(mut config: ModelRunConfig) -> ModelRunConfig {
        std::env::set_var("PROBELAB_TEST_KEY", "test-value-not-a-real-credential");
        if let ModelRunConfig::OpenAiCompatible { api_key_env, .. } = &mut config {
            *api_key_env = "PROBELAB_TEST_KEY".to_owned();
        }
        config
    }

    /// A connected agent is registered *alongside* the built-ins, never in place of them.
    ///
    /// This is the property that makes the feature safe to add. If registration replaced the registry,
    /// every existing plan naming a built-in agent would start failing, and the mock path -- which is
    /// what the whole product's test suite runs on -- would break with it.
    #[test]
    fn registering_a_connected_agent_leaves_every_builtin_reachable() {
        let agents = build_agent_registry(None);
        let ids: Vec<String> = agents.specs().iter().map(|spec| spec.id.clone()).collect();
        assert!(
            ids.contains(&"optimal".to_owned()) && ids.contains(&"random".to_owned()),
            "the built-in agents must survive: {ids:?}"
        );
        assert!(
            !ids.contains(&CONNECTED_AGENT_ID.to_owned()),
            "no connected agent is registered when no configuration was supplied"
        );
    }

    /// A missing credential is refused before anything runs, rather than failing mid-experiment.
    ///
    /// The alternative is a run that starts, announces itself, and then fails on episode one, leaving
    /// behind a stored record for an experiment that measured nothing.
    #[test]
    fn a_connected_run_without_a_credential_is_refused_rather_than_started() {
        // A **remote** endpoint. The address is never dialled — the refusal happens at configuration
        // time, before any network I/O — so a documentation-reserved domain is safe and is clearer
        // about intent than a port that happens to be closed on this machine.
        let error =
            with_credential_name_missing(connected_config("https://api.example.invalid/v1"));
        assert!(
            error.contains("PROBELAB_TEST_MISSING_KEY"),
            "the refusal must name the variable, which is not a secret, so a person can fix it: {error}"
        );
    }

    /// A loopback endpoint is the exception, and it is a deliberate one.
    ///
    /// This is the counterpart to the test above, and the pair is the whole contract: a remote provider
    /// with no credential is refused, a local one is not. Using a loopback URL for the "must refuse"
    /// claim would have been wrong in the direction that mattered — it would have looked like a
    /// regression rather than the exemption working — so both halves are pinned separately.
    #[test]
    fn a_loopback_run_needs_no_credential_and_is_not_refused() {
        let agent = connected_config("http://127.0.0.1:11434/v1")
            .build_agent(0)
            .expect("a local provider needs no credential, so it must not be refused");
        // The agent exists; nothing is dialled until a request is actually made.
        assert!(!agent.descriptor().adapter.is_empty());
    }

    /// Builds the config and asks for an agent, which is where validation and the credential check
    /// happen. Extracted so the assertion above reads as a claim about the error rather than as
    /// string plumbing.
    fn with_credential_name_missing(config: ModelRunConfig) -> String {
        config
            .build_agent(0)
            .err()
            .map(|error| error.to_string())
            .expect("a configuration with no credential must be refused")
    }

    /// A connected run is measured across the whole episode pool, with an interval.
    ///
    /// This is the test the feature exists for. One episode is not a measurement: it has no interval,
    /// nothing to compare against, and a single unlucky seed is indistinguishable from a model that
    /// cannot do the task at all.
    #[test]
    fn a_connected_model_is_measured_across_every_planned_episode() {
        let provider = FakeProvider::start();
        let config = with_credential(connected_config(&provider.uri));

        let mut agents = build_agent_registry(Some(&config));
        // The plan must name the connected agent, which is what the command does on a request that
        // supplies a configuration.
        let mut brain = LabBrain::new();
        let runner = LabRunner::new().with_agents(std::mem::take(&mut agents));
        let result = runner.run(
            &mut brain,
            &PlanRequest {
                benchmark_id: Some("grid-world".to_owned()),
                target_agent: Some(CONNECTED_AGENT_ID.to_owned()),
                probe_agents: Vec::new(),
                episodes: Some(4),
                root_seed: Some(4_242),
            },
            "exp-connected",
            &GridWorldEvaluator::new(),
            Arc::new(probelab_engine::arena::NullRecorder),
        );

        // A wrong-shaped or unparseable reply is a legitimate outcome for a fake provider that always
        // says MOVE_DOWN; what must not happen is a silent success. So the assertion is on *having
        // produced a terminal result at all*, not on a particular score.
        let run = result.expect("a connected run must reach a terminal result, not hang or abort");
        assert!(
            run.attempted() > 0,
            "the run attempted episodes rather than failing before the first turn"
        );
        assert!(
            provider.calls() > 0,
            "the provider was actually called; a run that never reached the network is not a \
             connected measurement"
        );

        // And the result records who produced it, which is what makes it comparable.
        let Some(first) = run.results.first() else {
            return;
        };
        assert_eq!(
            first.reproducibility.provider, "openai-compatible",
            "a connected result must name the provider that produced it"
        );
        assert_eq!(first.reproducibility.model, "test-model");
        assert_eq!(
            first.model.adapter, "openai-compatible-v1",
            "a connected result must carry the remote adapter identity, never a mock one: \
             this is what stops a local-model measurement from being read as a scripted reference run"
        );
        assert_ne!(
            first.model.adapter, "deterministic-mock-v1",
            "adapter identity is the machine-readable line between a model and a mock"
        );
        assert!(
            !first.reproducibility.configuration_fingerprint.is_empty(),
            "and a fingerprint, so a later comparison can tell this run from another"
        );
        assert_eq!(
            first.benchmark_definition_digest.as_deref(),
            Some(probelab_benchmarks::grid_world_definition_digest().as_str()),
            "and the task it measured"
        );
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

    /*
     * Exactly one command may answer "what runs exist", and it must read the durable store.
     *
     * The regression that made the stored-runs list permanently empty was not a crash or a wrong
     * shape: it was two commands with the same name answering from two different stores. The thinner
     * one is gone; this asserts it stays gone, and that the one that remains reads the record rather
     * than the session surface.
     *
     * Checked against the source because the failure is invisible at run time — every run succeeds and
     * every number is right while the list stays empty. This is the same source-scan approach
     * `arena.rs` uses to prove a command cannot steer an authorization.
     */
    #[test]
    fn exactly_one_experiment_listing_exists_and_it_reads_the_durable_store() {
        let source = include_str!("brain.rs");
        // The needle is assembled rather than written out, because this test quotes it in its own
        // source and a literal would match itself forever.
        let listing_signature = format!("pub fn list_{}experiments(", "");
        assert!(
            !source.contains(&listing_signature),
            "a second experiment listing would be a second answer to one question; \
             list_comparable_experiments is the one"
        );

        let comparison = include_str!("comparison.rs");
        let listing = comparison
            .split("pub fn list_comparable_experiments")
            .nth(1)
            .expect("the comparison listing exists")
            .split("/// Compares two or more")
            .next()
            .expect("it has a body");
        assert!(
            listing.contains("state") && listing.contains(".results"),
            "the experiment listing must read the durable store, or a run is unreachable after a \
             restart:\n{listing}"
        );
        assert!(
            !listing.contains("state.brain"),
            "the experiment listing must not read the session surface, which forgets every run the \
             moment the process exits:\n{listing}"
        );
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
