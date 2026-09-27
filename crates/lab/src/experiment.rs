//! The experiment manager: one run's lifecycle, and the guarantee that nothing is left behind.
//!
//! # The only guarantee that matters here
//!
//! Every terminal state must clean up. Not most, and not the happy path — every one. A lab that leaks
//! a contained child, a temporary directory, or a provider credential on the failure path is worse
//! than one that crashes, because the leak is invisible and the next run inherits it.
//!
//! That is why cleanup lives in this type's `Drop` rather than at the end of a happy-path function.
//! A caller that returns early, propagates an error, or panics still drops the manager, and the drop
//! does the same work. Terminal state is recorded separately from cleanup, so a failed cleanup is
//! reported rather than swallowed.
//!
//! # What it does not do
//!
//! It does not decide what to run — the Lab Brain plans that. It does not evaluate — the evaluator
//! does. It does not grant anything — the broker does. It sequences a run and cleans up after it.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use probelab_core::ContainmentFailureKind;

/// The lifecycle of one experiment.
///
/// Ordered so a transition can only move forward, which is what makes "stuck" a representable state
/// rather than something a caller has to reason about.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentState {
    /// Registered, nothing prepared.
    Created,
    /// Environment and probes being set up.
    Preparing,
    /// Ready to run an episode.
    Ready,
    /// An episode is executing.
    Running,
    /// Telemetry is being collected and watched.
    Observing,
    /// The evaluator is computing the result.
    Evaluating,
    /// Finished successfully.
    Completed,
    /// Finished because something failed.
    Failed,
    /// Stopped by the operator.
    Cancelled,
}

impl ExperimentState {
    /// Whether no further transition is possible.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ExperimentState::Completed | ExperimentState::Failed | ExperimentState::Cancelled
        )
    }

    /// A short uppercase label for the UI.
    pub fn label(self) -> &'static str {
        match self {
            ExperimentState::Created => "CREATED",
            ExperimentState::Preparing => "PREPARING",
            ExperimentState::Ready => "READY",
            ExperimentState::Running => "RUNNING",
            ExperimentState::Observing => "OBSERVING",
            ExperimentState::Evaluating => "EVALUATING",
            ExperimentState::Completed => "COMPLETED",
            ExperimentState::Failed => "FAILED",
            ExperimentState::Cancelled => "CANCELLED",
        }
    }

    /// The next state, or `None` when already terminal.
    fn advance(self) -> Option<Self> {
        Some(match self {
            ExperimentState::Created => ExperimentState::Preparing,
            ExperimentState::Preparing => ExperimentState::Ready,
            ExperimentState::Ready => ExperimentState::Running,
            ExperimentState::Running => ExperimentState::Observing,
            ExperimentState::Observing => ExperimentState::Evaluating,
            ExperimentState::Evaluating => ExperimentState::Completed,
            // A terminal state has no successor, and returning `None` is what makes
            // `ExperimentManager::finish` refuse to overwrite it.
            ExperimentState::Completed | ExperimentState::Failed | ExperimentState::Cancelled => {
                return None
            }
        })
    }
}

/// Why an experiment reached a non-completed terminal state.
///
/// Typed rather than a string, because these are the five facts a results table must keep apart: a
/// containment failure is a sandbox problem, a provider failure is a network problem, a protocol failure
/// is a model problem, a timeout is a budget problem, and a generic failure is none of them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum TerminalReason {
    /// The experiment ran to completion.
    Completed,
    /// The containment layer refused or failed. Nothing was started.
    ContainmentFailure {
        /// Which containment invariant failed.
        kind: ContainmentFailureKind,
    },
    /// The provider timed out, disconnected, or returned an error.
    ProviderFailure {
        /// The adapter's own classification.
        kind: String,
    },
    /// The model could not follow the response protocol.
    ProtocolFailure {
        /// How many consecutive failures were observed.
        consecutive: u32,
    },
    /// The wall-clock budget expired.
    Timeout {
        /// The budget that expired, in seconds.
        budget_seconds: u64,
    },
    /// The operator stopped it.
    Cancelled,
    /// Something else went wrong.
    Failed {
        /// A description that contains no secret.
        detail: String,
    },
}

impl TerminalReason {
    /// A short label for the UI.
    pub fn label(&self) -> &'static str {
        match self {
            TerminalReason::Completed => "COMPLETED",
            TerminalReason::ContainmentFailure { .. } => "CONTAINMENT FAILURE",
            TerminalReason::ProviderFailure { .. } => "PROVIDER FAILURE",
            TerminalReason::ProtocolFailure { .. } => "PROTOCOL FAILURE",
            TerminalReason::Timeout { .. } => "TIMEOUT",
            TerminalReason::Cancelled => "CANCELLED",
            TerminalReason::Failed { .. } => "FAILED",
        }
    }
}

/// What an experiment did, recorded as it happened.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExperimentSummary {
    /// Episodes started.
    pub episodes_started: u32,
    /// Episodes that finished.
    pub episodes_completed: u32,
    /// How many cleanup steps ran.
    pub cleanup_steps: u32,
    /// Cleanup steps that failed, if any. Never swallowed.
    pub cleanup_failures: Vec<String>,
}

/// A record of one experiment, safe to persist and to show.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExperimentRecord {
    /// Stable identifier.
    pub experiment_id: String,
    /// The benchmark that was selected.
    pub benchmark_id: String,
    /// The agent under evaluation.
    pub target_agent: String,
    /// Probes that were applied.
    pub probes: Vec<String>,
    /// Episodes requested.
    pub episodes_requested: usize,
    /// The root seed, so the experiment replays.
    pub root_seed: u64,
    /// Where the experiment reached.
    pub state: ExperimentState,
    /// Why, once terminal.
    pub terminal_reason: Option<TerminalReason>,
    /// What happened.
    pub summary: ExperimentSummary,
    /// Wall clock spent, in milliseconds.
    pub elapsed_ms: u64,
}

/// Owns one experiment's lifecycle.
///
/// The `Drop` impl is the point. Cleanup is not a step a caller remembers; it is what happens when
/// this value goes out of scope, including on an early return or a panic.
#[derive(Debug)]
pub struct ExperimentManager {
    record: ExperimentRecord,
    started: Instant,
    /// Registered cleanup steps, run in reverse order so nested resources unwind properly.
    cleanups: Vec<CleanupStep>,
}

/// One registered cleanup action.
struct CleanupStep {
    label: String,
    action: Box<dyn FnOnce() -> Result<(), String> + Send>,
}

impl std::fmt::Debug for CleanupStep {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CleanupStep")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl ExperimentManager {
    /// Creates an experiment in `Created` and registers its cleanup.
    ///
    /// `sandbox_root` is the run's directory. It is registered for removal immediately, before
    /// anything else, so a failure during preparation still leaves nothing behind.
    pub fn create(
        experiment_id: impl Into<String>,
        benchmark_id: impl Into<String>,
        target_agent: impl Into<String>,
        probes: Vec<String>,
        episodes: usize,
        root_seed: u64,
        sandbox_root: std::path::PathBuf,
    ) -> Self {
        let experiment_id = experiment_id.into();
        let record = ExperimentRecord {
            experiment_id: experiment_id.clone(),
            benchmark_id: benchmark_id.into(),
            target_agent: target_agent.into(),
            probes,
            episodes_requested: episodes,
            root_seed,
            state: ExperimentState::Created,
            terminal_reason: None,
            summary: ExperimentSummary::default(),
            elapsed_ms: 0,
        };
        let mut manager = Self {
            record,
            started: Instant::now(),
            cleanups: Vec::new(),
        };

        let root = sandbox_root.clone();
        manager.register_cleanup("remove_sandbox_root", move || {
            if !root.exists() {
                return Ok(());
            }
            std::fs::remove_dir_all(&root).map_err(|error| {
                format!("could not remove sandbox root {}: {error}", root.display())
            })
        });
        manager
    }

    /// Registers an additional cleanup step.
    ///
    /// Steps run in reverse registration order on drop, so a resource registered after its container
    /// is released first. Steps that fail are recorded and do not prevent the others from running —
    /// one stuck cleanup must not strand the rest.
    pub fn register_cleanup(
        &mut self,
        label: impl Into<String>,
        action: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) {
        self.cleanups.push(CleanupStep {
            label: label.into(),
            action: Box::new(action),
        });
    }

    /// The current lifecycle state.
    pub fn state(&self) -> ExperimentState {
        self.record.state
    }

    /// A snapshot of the record, for the UI and for persistence.
    pub fn record(&self) -> &ExperimentRecord {
        &self.record
    }

    /// Advances one step, refusing to move past a terminal state.
    pub fn advance(&mut self) -> Result<ExperimentState, ExperimentTransitionError> {
        let next =
            self.record
                .state
                .advance()
                .ok_or(ExperimentTransitionError::AlreadyTerminal {
                    state: self.record.state,
                })?;
        self.record.state = next;
        self.sync_elapsed();
        Ok(next)
    }

    /// Records that an episode started.
    pub fn episode_started(&mut self) {
        self.record.summary.episodes_started =
            self.record.summary.episodes_started.saturating_add(1);
    }

    /// Records that an episode finished.
    pub fn episode_completed(&mut self) {
        self.record.summary.episodes_completed =
            self.record.summary.episodes_completed.saturating_add(1);
    }

    /// Moves to a terminal state with a reason.
    ///
    /// Refuses to overwrite a terminal state, because the first reason is the real one and a later
    /// caller silently replacing it is how a containment failure ends up reported as a timeout.
    pub fn finish(&mut self, reason: TerminalReason) -> Result<(), ExperimentTransitionError> {
        if self.record.state.is_terminal() {
            return Err(ExperimentTransitionError::AlreadyTerminal {
                state: self.record.state,
            });
        }
        self.record.state = match reason {
            TerminalReason::Completed => ExperimentState::Completed,
            TerminalReason::Cancelled => ExperimentState::Cancelled,
            _ => ExperimentState::Failed,
        };
        self.record.terminal_reason = Some(reason);
        self.sync_elapsed();
        Ok(())
    }

    /// Whether the wall-clock budget has been exceeded.
    pub fn budget_exhausted(&self, budget: Duration) -> bool {
        self.started.elapsed() >= budget
    }

    /// How long the experiment has been running.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Runs cleanup now rather than at drop, for a caller that wants to observe the result.
    ///
    /// Idempotent: a second call runs nothing, because the steps are consumed. Tests use this to assert
    /// cleanup rather than inferring it from a drop they did not control.
    pub fn cleanup_now(&mut self) -> &ExperimentSummary {
        self.run_cleanup();
        &self.record.summary
    }

    fn run_cleanup(&mut self) {
        while let Some(step) = self.cleanups.pop() {
            self.record.summary.cleanup_steps = self.record.summary.cleanup_steps.saturating_add(1);
            let label = step.label.clone();
            if let Err(error) = (step.action)() {
                self.record
                    .summary
                    .cleanup_failures
                    .push(format!("{label}: {error}"));
            }
        }
    }

    fn sync_elapsed(&mut self) {
        self.record.elapsed_ms =
            u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
    }
}

impl Drop for ExperimentManager {
    fn drop(&mut self) {
        // The whole point of this impl. A caller that returns early, propagates an error, or unwinds
        // still releases everything the experiment registered.
        self.run_cleanup();
        self.sync_elapsed();
    }
}

/// A lifecycle transition failure.
#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExperimentTransitionError {
    /// The experiment already reached a terminal state.
    #[error("the experiment is already {state:?} and cannot transition")]
    AlreadyTerminal {
        /// The state it is in.
        state: ExperimentState,
    },
}

/// A plain summary of a finished experiment, for a results list.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExperimentSummaryRow {
    /// The experiment identifier.
    pub experiment_id: String,
    /// The benchmark.
    pub benchmark_id: String,
    /// The agent under evaluation.
    pub target_agent: String,
    /// Where it reached.
    pub state: ExperimentState,
    /// Its terminal reason label, if any.
    pub terminal_reason: Option<String>,
    /// Episodes completed out of those requested.
    pub episodes: String,
    /// Wall clock in milliseconds.
    pub elapsed_ms: u64,
}

impl ExperimentRecord {
    /// Renders a one-line summary for a results table.
    pub fn summary_row(&self) -> ExperimentSummaryRow {
        ExperimentSummaryRow {
            experiment_id: self.experiment_id.clone(),
            benchmark_id: self.benchmark_id.clone(),
            target_agent: self.target_agent.clone(),
            state: self.state,
            terminal_reason: self
                .terminal_reason
                .as_ref()
                .map(TerminalReason::label)
                .map(str::to_owned),
            episodes: format!(
                "{} / {}",
                self.summary.episodes_completed, self.episodes_requested
            ),
            elapsed_ms: self.elapsed_ms,
        }
    }
}

/// A collection of finished records, for a results view.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExperimentArchive {
    records: BTreeMap<String, ExperimentRecord>,
}

impl ExperimentArchive {
    /// Adds or replaces a record.
    pub fn insert(&mut self, record: ExperimentRecord) {
        self.records.insert(record.experiment_id.clone(), record);
    }

    /// One record by identifier.
    pub fn get(&self, experiment_id: &str) -> Option<&ExperimentRecord> {
        self.records.get(experiment_id)
    }

    /// Every record, in stable identifier order.
    pub fn records(&self) -> Vec<&ExperimentRecord> {
        self.records.values().collect()
    }

    /// How many experiments are archived.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the archive is empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Summary rows for every record.
    pub fn summary_rows(&self) -> Vec<ExperimentSummaryRow> {
        self.records
            .values()
            .map(ExperimentRecord::summary_row)
            .collect()
    }
}
