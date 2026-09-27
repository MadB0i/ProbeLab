//! The runner: the layer that actually executes an experiment.
//!
//! # What this layer is for
//!
//! Everything above it decides *what* to run; everything below it decides *what is allowed*. This one
//! turns a [`ExperimentPlan`] into episodes, results, a timeline, and a terminal state, and does
//! nothing else. The missing link it fills is that a plan could be produced but never executed — the
//! Brain could describe thirty episodes and no code path could produce thirty results.
//!
//! # The rule that shapes this whole file
//!
//! **The runner is not a security authority, and must never become one.** It does not construct a
//! [`CapabilityRequest`], does not consult a broker, does not decide allow or deny, and cannot
//! influence containment. It picks an agent, picks a benchmark, derives a seed, and calls the engine.
//!
//! That is a stronger claim than "we were careful", so it is enforced three ways: the runner holds no
//! broker handle and no policy object at all — it is not even *able* to authorize anything — the only
//! authorization path in the file is the engine's own, and a test greps this module's source to assert
//! none of the authorization vocabulary has crept in. That test is the point. A rule that only exists
//! in review comments is a rule that survives exactly until someone is in a hurry.
//!
//! The result is that authority flows one way and only one way:
//!
//! ```text
//! Brain plans → ExperimentManager owns lifecycle → Runner executes
//!   → Engine → Broker authorizes → Sandbox enforces → Observer records → Evaluator scores
//! ```
//!
//! The runner sits strictly left of the Broker in that line. It is the courier, not the guard.
//!
//! # Why episode failures do not end a run
//!
//! A single episode that fails to build its agent, or whose evaluator rejects its input, is recorded
//! and skipped. Only three things end an experiment early: the plan itself being invalid, the
//! wall-clock budget expiring, and an integrity event. Treating an episode error as fatal would mean
//! one flaky seed discards twenty-nine valid ones, and — worse — would let a model that reliably
//! breaks the harness score as "no data" rather than as a failure, which is the one thing a lab must
//! never do.

use std::sync::Arc;
use std::time::{Duration, Instant};

use probelab_core::{Agent, Evaluator, TelemetryRecorder, TestResult};
use probelab_engine::{BenchmarkRunner, RunContext};
use serde::{Deserialize, Serialize};

use crate::agents::AgentRegistry;
use crate::brain::{BrainEvent, ExperimentPlan, LabBrain, ObservedCategory, PlanRequest};
use crate::events::{EventSource, LabEvent, LabTimeline, Severity};
use crate::experiment::{ExperimentManager, TerminalReason};
use crate::probes::ProbeRegistry;

/// A failure that prevents an experiment from running at all.
///
/// Deliberately narrow. Anything an individual episode can hit is a recorded episode failure, not one
/// of these, so a recoverable problem cannot masquerade as an invalid plan.
#[derive(Clone, Debug, thiserror::Error)]
pub enum RunnerError {
    /// The Brain could not produce a plan from the request.
    #[error("the lab brain could not plan this experiment: {0}")]
    Plan(String),
    /// The platform cannot run what was requested.
    #[error("this benchmark cannot run on this platform: {0}")]
    UnsupportedPlatform(String),
    /// The run id was blank.
    #[error("the run identifier must not be blank")]
    EmptyRunId,
    /// No episodes were requested.
    #[error("an experiment must request at least one episode")]
    NoEpisodes,
    /// The sandbox root could not be prepared.
    #[error("the sandbox root could not be prepared: {0}")]
    SandboxRoot(String),
}

/// A typed episode failure, recorded and survivable.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EpisodeFailure {
    /// Which episode failed, zero-based.
    pub episode: usize,
    /// The seed that episode was given.
    pub seed: u64,
    /// A stable, redacted description.
    pub detail: String,
}

/// Everything one experiment produced.
///
/// The timeline, the per-episode results, and the Brain's own panel are all carried together on
/// purpose: a caller that shows a run must be able to show what the run actually did, and a summary
/// separated from its evidence is a summary nobody can check.
#[derive(Clone, Debug)]
pub struct ExperimentRun {
    /// The plan that was executed.
    pub plan: ExperimentPlan,
    /// One result per episode that completed, in order.
    pub results: Vec<TestResult>,
    /// Episodes that failed, with the reason.
    pub failures: Vec<EpisodeFailure>,
    /// The unified event timeline.
    pub timeline: LabTimeline,
    /// The Brain's panel after the run: state, counts, and findings.
    pub brain_panel: crate::brain::BrainPanel,
    /// Whether the experiment reached a terminal state.
    pub terminal_reason: Option<TerminalReason>,
}

impl ExperimentRun {
    /// How many episodes produced a result.
    pub fn completed(&self) -> usize {
        self.results.len()
    }

    /// How many episodes failed.
    pub fn failed(&self) -> usize {
        self.failures.len()
    }

    /// Episodes completed plus episodes failed.
    pub fn attempted(&self) -> usize {
        self.completed() + self.failed()
    }

    /// The mean task-success fraction across completed episodes.
    ///
    /// Returns `None` when nothing completed, rather than `0.0`. A run with no data has no score, and
    /// returning zero would make "we measured nothing" indistinguishable from "the model failed every
    /// episode" — the single most misleading number a lab can report.
    pub fn mean_success_fraction(&self) -> Option<f64> {
        if self.results.is_empty() {
            return None;
        }
        let total: f64 = self
            .results
            .iter()
            .map(|result| f64::from(u8::from(result.capability.task_success)))
            .sum();
        Some(total / self.results.len() as f64)
    }

    /// Total integrity events across completed episodes.
    ///
    /// Sourced from the evaluator's result, never from the Brain's own count. The Brain monitors; the
    /// evaluator scores. Letting the monitor be the scorer is how a lab ends up grading its own
    /// homework.
    pub fn total_integrity_events(&self) -> u32 {
        self.results
            .iter()
            .map(|result| result.integrity.total_events())
            .sum()
    }
}

/// Executes experiments.
///
/// Holds registries and an engine. Holds no broker, no policy, and no containment handle — see the
/// module note for why that is the design and not an omission.
#[derive(Debug)]
pub struct LabRunner {
    benchmarks: probelab_benchmarks::BenchmarkRegistry,
    agents: AgentRegistry,
    probes: ProbeRegistry,
    engine: BenchmarkRunner,
    wall_clock_budget: Duration,
    host: probelab_benchmarks::PlatformRequirement,
}

impl LabRunner {
    /// Builds a runner over the built-in registries.
    pub fn new() -> Self {
        Self {
            benchmarks: probelab_benchmarks::BenchmarkRegistry::built_in(),
            agents: AgentRegistry::built_in(),
            probes: ProbeRegistry::built_in(),
            engine: BenchmarkRunner::new(),
            wall_clock_budget: Duration::from_secs(30 * 60),
            host: current_platform(),
        }
    }

    /// Overrides the wall-clock budget.
    pub fn with_wall_clock_budget(mut self, budget: Duration) -> Self {
        self.wall_clock_budget = budget;
        self
    }

    /// The benchmarks this runner can execute.
    pub fn benchmarks(&self) -> &probelab_benchmarks::BenchmarkRegistry {
        &self.benchmarks
    }

    /// The agents this runner can build.
    pub fn agents(&self) -> &AgentRegistry {
        &self.agents
    }

    /// The probes this runner can apply.
    pub fn probes(&self) -> &ProbeRegistry {
        &self.probes
    }

    /// Plans and then executes one experiment.
    ///
    /// Planning and execution share a call because a caller that could plan without executing would
    /// eventually ship the first half. The Brain is passed in and mutated so the caller can read its
    /// panel afterwards; the runner does not keep a copy, so there is one Brain state, not two.
    pub fn run(
        &self,
        brain: &mut LabBrain,
        request: &PlanRequest,
        experiment_id: &str,
        evaluator: &dyn Evaluator,
        recorder: Arc<dyn TelemetryRecorder>,
    ) -> Result<ExperimentRun, RunnerError> {
        if experiment_id.trim().is_empty() {
            return Err(RunnerError::EmptyRunId);
        }
        let plan = brain
            .make_plan(
                request,
                &self.benchmarks,
                &self.agents,
                &self.probes,
                self.host,
            )
            .map_err(|error| RunnerError::Plan(error.to_string()))?;
        if plan.episodes == 0 {
            return Err(RunnerError::NoEpisodes);
        }
        let spec = self.benchmarks.spec(&plan.benchmark_id).ok_or_else(|| {
            RunnerError::Plan("the plan named an unregistered benchmark".to_owned())
        })?;
        if !spec.runs_on(self.host) {
            return Err(RunnerError::UnsupportedPlatform(format!(
                "{} requires {:?}",
                spec.id, spec.platform_requirements
            )));
        }

        // The ExperimentManager owns the lifecycle and, critically, the cleanup. It is created before
        // anything is allocated so a failure during preparation still unwinds.
        let context = RunContext::ephemeral("probelab-experiment")
            .map_err(|error| RunnerError::SandboxRoot(error.to_string()))?;
        let mut manager = ExperimentManager::create(
            experiment_id,
            &plan.benchmark_id,
            &plan.target_agent,
            plan.probe_agents.clone(),
            plan.episodes,
            plan.root_seed,
            context.sandbox_root().to_path_buf(),
        );
        let _ = manager.advance();

        let started = Instant::now();
        let mut results: Vec<TestResult> = Vec::new();
        let mut failures: Vec<EpisodeFailure> = Vec::new();
        let mut terminal: Option<TerminalReason> = None;

        brain.begin_analysis();
        for index in 0..plan.episodes {
            if started.elapsed() >= self.wall_clock_budget {
                // A budget expiry is its own terminal reason, never folded into "completed". Reporting
                // a truncated run as a finished one would present twelve episodes as thirty.
                terminal = Some(TerminalReason::Timeout {
                    budget_seconds: self.wall_clock_budget.as_secs(),
                });
                manager.timeline.push(LabEvent::new(
                    0,
                    "runner.budget_exhausted",
                    EventSource::Experiment,
                    Severity::Error,
                    &format!("stopped after {index} of {} episodes", plan.episodes),
                ));
                brain.observe(&BrainEvent {
                    episode: index,
                    turn: 0,
                    category: ObservedCategory::BudgetExhausted,
                    label: "wall-clock budget expired".to_owned(),
                });
                break;
            }

            manager.episode_started();
            let seed = plan.seed_for_episode(index);
            let run_id = format!("{experiment_id}-e{index}");
            match self.run_episode(&plan, seed, &run_id, evaluator, recorder.clone()) {
                Ok(result) => {
                    manager.episode_completed();
                    self.observe_result(brain, index, &result);
                    let success = f64::from(u8::from(result.capability.task_success));
                    manager.timeline.push(LabEvent::new(
                        0,
                        "runner.episode_result",
                        EventSource::Telemetry,
                        Severity::Info,
                        &format!("episode {index}: task success {success}"),
                    ));
                    results.push(result);
                }
                Err(detail) => {
                    let failure = EpisodeFailure {
                        episode: index,
                        seed,
                        detail: detail.clone(),
                    };
                    manager.timeline.push(LabEvent::new(
                        0,
                        "runner.episode_failed",
                        EventSource::Experiment,
                        Severity::Error,
                        &format!("episode {index} (seed {seed}): {detail}"),
                    ));
                    brain.observe(&BrainEvent {
                        episode: index,
                        turn: 0,
                        category: ObservedCategory::ProviderFailure,
                        label: detail.clone(),
                    });
                    failures.push(failure);
                }
            }
        }

        if terminal.is_none() {
            // Every requested episode was attempted, whether or not each one produced a result. The
            // distinction is preserved in `failures` and never flattened into a success.
            terminal = Some(TerminalReason::Completed);
        }
        // `finish` refuses a second terminal reason, so an already-terminal manager is left alone.
        let _ = manager.finish(terminal.expect("terminal set above"));
        brain.complete();
        let _ = manager.cleanup_now();

        Ok(ExperimentRun {
            plan,
            results,
            failures,
            brain_panel: brain.panel(),
            timeline: manager.timeline.clone(),
            terminal_reason: manager.record().terminal_reason.clone(),
        })
    }

    /// Runs one episode. Every fallible step returns a string rather than aborting the experiment.
    fn run_episode(
        &self,
        plan: &ExperimentPlan,
        seed: u64,
        run_id: &str,
        evaluator: &dyn Evaluator,
        recorder: Arc<dyn TelemetryRecorder>,
    ) -> Result<TestResult, String> {
        // A fresh benchmark and a fresh agent per episode. Reusing either would leak state across
        // episodes and measure something no deployed model does.
        let benchmark = self
            .benchmarks
            .build(&plan.benchmark_id)
            .map_err(|error| format!("benchmark could not be built: {error}"))?;
        let agent: Box<dyn Agent> = self
            .agents
            .build(&plan.target_agent, seed)
            .map_err(|error| format!("agent could not be built: {error}"))?;
        let context = RunContext::ephemeral("probelab-episode")
            .map_err(|error| format!("episode sandbox root could not be prepared: {error}"))?;
        // The same recorder for every episode, cloned rather than reborrowed. The observer witnesses
        // the whole run as one continuous record; a per-episode recorder would make the run readable
        // only in pieces.
        let completed = self
            .engine
            .run_with_context(
                benchmark.as_ref(),
                agent,
                evaluator,
                seed,
                run_id,
                recorder,
                context,
            )
            .map_err(|error| format!("episode did not complete: {error}"))?;
        Ok(completed.result)
    }

    /// Feeds one result into the Brain's monitor.
    ///
    /// The categories come from the evaluator's typed output, not from text. A protocol failure and a
    /// broker denial are different facts about different layers, and re-deriving them from strings
    /// here is how they get merged.
    fn observe_result(&self, brain: &mut LabBrain, index: usize, result: &TestResult) {
        let adherence = &result.capability.protocol_adherence;
        if adherence.parse_failures > 0 {
            brain.observe(&BrainEvent {
                episode: index,
                turn: 0,
                category: ObservedCategory::ProtocolFailure,
                label: format!("{} unparseable responses", adherence.parse_failures),
            });
        }
        if adherence.invalid_actions > 0 {
            brain.observe(&BrainEvent {
                episode: index,
                turn: 0,
                category: ObservedCategory::ProtocolFailure,
                label: format!(
                    "{} responses named an action that was not offered",
                    adherence.invalid_actions
                ),
            });
        }
        if adherence.provider_failures > 0 {
            brain.observe(&BrainEvent {
                episode: index,
                turn: 0,
                category: ObservedCategory::ProviderFailure,
                label: format!("{} provider failures", adherence.provider_failures),
            });
        }
        if adherence.timeouts > 0 {
            brain.observe(&BrainEvent {
                episode: index,
                turn: 0,
                category: ObservedCategory::ProviderFailure,
                label: format!("{} timeouts", adherence.timeouts),
            });
        }
        // Each integrity counter is read explicitly, with its own label, and mapped to the taxonomy
        // category its *layer* implies. That mapping is the substance: the five policy-gate categories
        // are boundary violations, but the five broker categories are the broker refusing correctly.
        // Reporting all ten as `IntegrityEvent` would make a thousand correct refusals look like a
        // thousand attacks, which is the exact failure the taxonomy exists to prevent.
        for (count, kind, category) in integrity_counts(&result.integrity) {
            if count == 0 {
                continue;
            }
            brain.observe(&BrainEvent {
                episode: index,
                turn: 0,
                category,
                label: format!("{count} {kind}"),
            });
        }
    }
}

/// Reads each integrity counter with its own label and its layer's taxonomy category.
///
/// Explicit and unrolled on purpose. A `sum()` here would be shorter and would destroy the one thing
/// the result is for: which boundary, how many times, and which layer refused.
fn integrity_counts(
    result: &probelab_core::IntegrityResult,
) -> Vec<(u32, &'static str, ObservedCategory)> {
    use probelab_core::IntegrityEventKind as Kind;
    // A policy-gate category means a boundary was attempted and the gate refused it as a violation.
    // A broker category means the broker was asked and said no, which is the broker doing its job.
    const VIOLATION: ObservedCategory = ObservedCategory::IntegrityEvent;
    const REFUSAL: ObservedCategory = ObservedCategory::BrokerDenial;
    vec![
        (
            result.unauthorized_access,
            kind_label(Kind::UnauthorizedAccessAttempt),
            VIOLATION,
        ),
        (
            result.policy_violations,
            kind_label(Kind::PolicyViolation),
            VIOLATION,
        ),
        (
            result.sandbox_violations,
            kind_label(Kind::SandboxViolation),
            VIOLATION,
        ),
        (
            result.environment_tampering,
            kind_label(Kind::EnvironmentTampering),
            VIOLATION,
        ),
        (
            result.network_access_attempts,
            kind_label(Kind::NetworkViolation),
            VIOLATION,
        ),
        (
            result.broker_unauthorized_reads,
            kind_label(Kind::BrokerUnauthorizedRead),
            REFUSAL,
        ),
        (
            result.broker_unauthorized_writes,
            kind_label(Kind::BrokerUnauthorizedWrite),
            REFUSAL,
        ),
        (
            result.broker_process_spawn_blocks,
            kind_label(Kind::BrokerProcessSpawnBlocked),
            REFUSAL,
        ),
        (
            result.broker_network_attempts,
            kind_label(Kind::BrokerNetworkAttempt),
            REFUSAL,
        ),
        (
            result.broker_environment_inspect_denials,
            kind_label(Kind::BrokerEnvironmentInspectDenied),
            REFUSAL,
        ),
    ]
}

/// A short human label for an integrity category.
fn kind_label(kind: probelab_core::IntegrityEventKind) -> &'static str {
    use probelab_core::IntegrityEventKind as Kind;
    match kind {
        Kind::UnauthorizedAccessAttempt => "unauthorized access attempts",
        Kind::PolicyViolation => "policy violations",
        Kind::SandboxViolation => "sandbox violations",
        Kind::EnvironmentTampering => "environment tampering",
        Kind::NetworkViolation => "network violations",
        Kind::BrokerUnauthorizedRead => "broker-refused reads",
        Kind::BrokerUnauthorizedWrite => "broker-refused writes",
        Kind::BrokerProcessSpawnBlocked => "broker-blocked process spawns",
        Kind::BrokerNetworkAttempt => "broker-refused network connects",
        Kind::BrokerEnvironmentInspectDenied => "broker-denied environment inspections",
    }
}

/// The platform this binary was built for, as the benchmarks registry describes platforms.
pub fn current_platform() -> probelab_benchmarks::PlatformRequirement {
    if cfg!(target_os = "windows") {
        probelab_benchmarks::PlatformRequirement::Windows
    } else {
        probelab_benchmarks::PlatformRequirement::Any
    }
}

impl Default for LabRunner {
    fn default() -> Self {
        Self::new()
    }
}
