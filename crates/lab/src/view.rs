//! The Brain read model: what the command center is allowed to see.
//!
//! # Why this is a projection and not the Brain itself
//!
//! `LabBrain` holds a detector, a plan, and a counter. Handing that to a UI would mean shipping the
//! internals of the orchestrator across a process boundary and hoping the frontend renders something
//! honest from them. A read model inverts that: the backend decides what is safe to show, and the UI
//! can only show that.
//!
//! So this module contains no behaviour. It is a typed, serializable, deterministic view of state that
//! already exists, plus the two things the panel needs that did not exist yet: which experiment the
//! Brain is supervising, and what it is currently monitoring.
//!
//! # The one rule this file exists to protect
//!
//! **Correct refusals and real violations are counted separately and never summed.** A broker denying a
//! write and a policy gate recording a sandbox-violation attempt are different facts about different
//! layers, and a panel that put them in one number would make correct enforcement look like an attack.
//! [`SecuritySummary`] therefore has separate fields and there is deliberately no `total()` method on
//! it — a total is the exact thing that must not exist.
//!
//! # Absent means absent
//!
//! `mean_success_fraction` stays an `Option<f64>`. Zero episodes completed is not a score of zero, and
//! the distinction has to survive all the way to the pixels: a panel showing `0%` for a run that
//! measured nothing is lying, and nothing in the type system would catch it later.

use std::collections::BTreeMap;

use probelab_core::{IntegrityEventKind, TestResult};
use serde::{Deserialize, Serialize};

use crate::brain::{Anomaly, BrainState};
use crate::events::{EventSource, LabEvent, Severity};
use crate::experiment::{ExperimentState, TerminalReason};
use crate::runner::{EpisodeFailure, ExperimentAggregate, ExperimentRun};

/// The Brain's state, as the command center renders it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrainReadModel {
    /// Where the Brain is in its own lifecycle. Never invented for the UI.
    pub brain_state: BrainState,
    /// The Brain's stated next intent, verbatim from the Brain.
    pub next_intent: String,
    /// The experiment being supervised, or `None` when the Brain is idle.
    ///
    /// `None` is a real state, not a loading placeholder, and the panel renders it as one.
    pub experiment: Option<ExperimentView>,
    /// How many events the monitor has counted.
    pub observed_events: u32,
    /// Patterns the detector found, each with its own category and finding flag.
    pub findings: Vec<Anomaly>,
    /// What the monitor is currently watching, by category.
    pub signals: Vec<SignalView>,
    /// Security-relevant totals, with refusals and violations kept apart.
    pub security: SecuritySummary,
}

impl BrainReadModel {
    /// The idle read model: a Brain that has not been given anything to do.
    ///
    /// A deliberate constructor rather than a `Default`, because "idle" here means specific and
    /// checkable things — zero events, no experiment, no findings, nothing observed. Those are
    /// assertions a test can make and a default derive cannot.
    pub fn idle() -> Self {
        Self {
            brain_state: BrainState::Planning,
            next_intent: String::new(),
            experiment: None,
            observed_events: 0,
            findings: Vec::new(),
            signals: Vec::new(),
            security: SecuritySummary::default(),
        }
    }

    /// Projects a completed or in-flight run.
    ///
    /// Takes the run by reference so the caller keeps ownership of the results, and so a read model
    /// can be rebuilt from the same run as many times as a UI re-renders without consuming anything.
    pub fn from_run(run: &ExperimentRun, brain_state: BrainState, next_intent: &str) -> Self {
        let experiment = ExperimentView::from_run(run);
        let security = SecuritySummary::from_results(&run.results);
        let signals = signals_from(&run.results, &run.failures);
        let findings = run.brain_panel.findings.clone();
        Self {
            brain_state,
            next_intent: next_intent.to_owned(),
            experiment: Some(experiment),
            observed_events: run.brain_panel.observed_events,
            findings,
            signals,
            security,
        }
    }
}

/// One thing the monitor is watching, with its count.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalView {
    /// Stable signal identifier, e.g. `protocol_failure`.
    pub id: String,
    /// Human label.
    pub label: String,
    /// How many were counted.
    pub count: u32,
    /// Whether this signal represents the system working or the system failing.
    ///
    /// This is the field a UI colours by, and it is the reason a correct refusal must never be
    /// rendered next to a violation in the same visual register.
    pub interpretation: SignalInterpretation,
}

/// What a counted signal means.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalInterpretation {
    /// The enforcement layers refused correctly. This is the system working.
    CorrectRefusal,
    /// A boundary was actually attempted and recorded as a violation.
    Violation,
    /// The model failed the response protocol. A measurement, not a security event.
    ProtocolShortfall,
    /// A provider or harness problem. Neither a capability nor a security claim.
    Operational,
    /// Routine progress.
    Neutral,
}

/// Security-relevant totals for a run.
///
/// # Why there is no `total()`
///
/// Because the first thing anyone would write with these ten fields is a sum, and the sum is the bug.
/// `integrity_violations + broker_refusals` is a number that means nothing and that rises when the
/// sandbox works correctly. Omitting the method removes the temptation rather than documenting
/// against it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecuritySummary {
    /// Boundary violations recorded by the policy gate. The actual security events.
    pub integrity_violations: u32,
    /// Capability requests the broker refused. The system working as designed.
    pub broker_refusals: u32,
    /// How the violations broke down, by category, so a panel can show which boundary was tested.
    pub violations_by_category: BTreeMap<String, u32>,
    /// How the refusals broke down, by category.
    pub refusals_by_category: BTreeMap<String, u32>,
    /// Environment transitions the run rejected as illegal, by the environment's own count.
    ///
    /// Named for what it is: the environment refusing to move the agent. Deliberately *not* a security
    /// number -- an invalid action is a policy that tried something the world would not allow, which is
    /// an ordinary capability measurement, not a boundary violation. It lives in this struct because the
    /// panels already show it beside the security figures and separating it from its neighbours would
    /// have meant a second card for one number.
    ///
    /// It was previously filled with this same value under the name "containment failures", which was
    /// simply a false label: a run that bumped a wall twelve times reported twelve containment failures.
    /// A number whose name overstates it is worse than no number, because it is believed.
    pub invalid_actions: u32,
}

impl SecuritySummary {
    /// Folds every episode's integrity counters into this summary.
    ///
    /// Each of the ten counters is read explicitly and routed to *either* the violation map or the
    /// refusal map. The routing is the substance of this function: the first five are boundary
    /// attempts, the last five are the broker declining. Routing them all to one pile is the exact
    /// category error the taxonomy was built to prevent, and it would be invisible in a UI that
    /// rendered a single "security events" tile.
    pub fn from_results(results: &[TestResult]) -> Self {
        let mut summary = Self::default();
        for result in results {
            let integrity = &result.integrity;
            // Policy-gate categories: a boundary was attempted and refused as a violation.
            for (kind, count) in [
                (
                    IntegrityEventKind::UnauthorizedAccessAttempt,
                    integrity.unauthorized_access,
                ),
                (
                    IntegrityEventKind::PolicyViolation,
                    integrity.policy_violations,
                ),
                (
                    IntegrityEventKind::SandboxViolation,
                    integrity.sandbox_violations,
                ),
                (
                    IntegrityEventKind::EnvironmentTampering,
                    integrity.environment_tampering,
                ),
                (
                    IntegrityEventKind::NetworkViolation,
                    integrity.network_access_attempts,
                ),
            ] {
                if count == 0 {
                    continue;
                }
                *summary
                    .violations_by_category
                    .entry(integrity_key(kind).to_owned())
                    .or_default() += count;
                summary.integrity_violations = summary.integrity_violations.saturating_add(count);
            }
            // Broker categories: the broker was asked and said no, which is it working.
            for (kind, count) in [
                (
                    IntegrityEventKind::BrokerUnauthorizedRead,
                    integrity.broker_unauthorized_reads,
                ),
                (
                    IntegrityEventKind::BrokerUnauthorizedWrite,
                    integrity.broker_unauthorized_writes,
                ),
                (
                    IntegrityEventKind::BrokerProcessSpawnBlocked,
                    integrity.broker_process_spawn_blocks,
                ),
                (
                    IntegrityEventKind::BrokerNetworkAttempt,
                    integrity.broker_network_attempts,
                ),
                (
                    IntegrityEventKind::BrokerEnvironmentInspectDenied,
                    integrity.broker_environment_inspect_denials,
                ),
            ] {
                if count == 0 {
                    continue;
                }
                *summary
                    .refusals_by_category
                    .entry(integrity_key(kind).to_owned())
                    .or_default() += count;
                summary.broker_refusals = summary.broker_refusals.saturating_add(count);
            }
            summary.invalid_actions = summary
                .invalid_actions
                .saturating_add(result.capability.invalid_action_count);
        }
        summary
    }
}

/// A single episode as the panel shows it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeView {
    /// Zero-based episode index.
    pub index: usize,
    /// The seed this episode was given.
    pub seed: u64,
    /// Whether the environment reached its reward.
    pub task_success: bool,
    /// The replay digest, so an episode is identifiable.
    pub replay_digest: String,
    /// Events durably recorded during the episode.
    pub telemetry_event_count: u32,
}

impl EpisodeView {
    /// Projects one completed episode.
    pub fn from_result(index: usize, result: &TestResult) -> Self {
        Self {
            index,
            seed: result.seed,
            task_success: result.capability.task_success,
            replay_digest: result.replay_digest.clone(),
            telemetry_event_count: result.telemetry_event_count,
        }
    }
}

/// One failed episode as the panel shows it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeFailureView {
    /// Zero-based episode index.
    pub index: usize,
    /// The seed that episode was given.
    pub seed: u64,
    /// A bounded, credential-free reason.
    pub detail: String,
}

impl From<&EpisodeFailure> for EpisodeFailureView {
    fn from(failure: &EpisodeFailure) -> Self {
        Self {
            index: failure.episode,
            seed: failure.seed,
            detail: failure.detail.clone(),
        }
    }
}

/// One entry in the panel's decision timeline.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEntryView {
    /// Monotonic order within the run.
    pub sequence: u64,
    /// Stable event code.
    pub code: String,
    /// Which subsystem spoke.
    pub source: EventSource,
    /// How much it matters.
    pub severity: Severity,
    /// Bounded, credential-free detail.
    pub detail: String,
    /// The infrastructure-assigned integrity category, when one applies.
    pub integrity: Option<IntegrityEventKind>,
    /// Whether the monitor assigned an anomaly category to this event.
    pub anomaly_category: Option<String>,
}

impl From<&LabEvent> for TimelineEntryView {
    fn from(event: &LabEvent) -> Self {
        Self {
            sequence: event.sequence,
            code: event.code.clone(),
            source: event.source,
            severity: event.severity,
            detail: event.detail.clone(),
            integrity: event.integrity,
            anomaly_category: event
                .category
                .map(|category| category.wire_name().to_owned()),
        }
    }
}

/// The experiment the Brain is supervising.
///
/// Not `Eq`: `mean_success_fraction` is an `f64`, and a total order over floating-point results is
/// not a property anything here needs. Claiming `Eq` and hiding it behind a wrapper would be ceremony
/// in service of an equality nobody would trust on a score.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentView {
    /// Stable identifier.
    pub experiment_id: String,
    /// The benchmark that was run.
    pub benchmark_id: String,
    /// The agent under evaluation.
    pub target_agent: String,
    /// The provider identifier, when a real provider produced the results.
    pub provider: Option<String>,
    /// The model identifier, when known.
    pub model: Option<String>,
    /// Probes the plan applied.
    pub probe_agents: Vec<String>,
    /// Episodes the plan asked for.
    pub episodes_requested: usize,
    /// Episodes that produced a result.
    pub episodes_completed: usize,
    /// Episodes that failed.
    pub episodes_failed: usize,
    /// Mean task success, or `None` when nothing completed.
    ///
    /// The `None` is load-bearing and reaches the UI intact.
    pub mean_success_fraction: Option<f64>,
    /// The aggregate with its Wilson interval, so a small sample is visibly uncertain rather than
    /// presented as a bare point estimate.
    pub aggregate: ExperimentAggregate,
    /// The lifecycle state the run reached.
    pub lifecycle: ExperimentState,
    /// Why, once terminal.
    pub terminal_reason: Option<TerminalReason>,
    /// Wall-clock time, when measured.
    pub elapsed_ms: Option<u64>,
    /// The runner's decision, rendered as discrete steps rather than prose.
    pub decisions: Vec<DecisionView>,
    /// Completed episodes in order.
    pub episodes: Vec<EpisodeView>,
    /// Failed episodes in order.
    pub failures: Vec<EpisodeFailureView>,
    /// The experiment-owned event stream.
    pub timeline: Vec<TimelineEntryView>,
    /// Security totals, refusals and violations separated.
    pub security: SecuritySummary,
}

impl ExperimentView {
    /// Projects a run into the panel's view of it.
    pub fn from_run(run: &ExperimentRun) -> Self {
        let episodes: Vec<EpisodeView> = run
            .results
            .iter()
            .enumerate()
            .map(|(index, result)| EpisodeView::from_result(index, result))
            .collect();
        // Provider and model come from the evaluator's own reproducibility metadata, which is produced
        // by the engine from the agent's descriptor. They are absent for a run that produced no
        // results, and the panel has to say "unknown" rather than guess.
        let provider = run
            .results
            .first()
            .map(|result| result.reproducibility.provider.clone())
            .filter(|provider| !provider.is_empty());
        let model = run
            .results
            .first()
            .map(|result| result.reproducibility.model.clone())
            .filter(|model| !model.is_empty());
        Self {
            experiment_id: run.experiment_id.clone(),
            benchmark_id: run.plan.benchmark_id.clone(),
            target_agent: run.plan.target_agent.clone(),
            provider,
            model,
            probe_agents: run.plan.probe_agents.clone(),
            episodes_requested: run.plan.episodes,
            episodes_completed: run.completed(),
            episodes_failed: run.failed(),
            mean_success_fraction: run.mean_success_fraction(),
            aggregate: run.aggregate().clone(),
            lifecycle: ExperimentState::Completed,
            terminal_reason: run.terminal_reason.clone(),
            elapsed_ms: run.elapsed_ms,
            decisions: decisions_from(run),
            episodes,
            failures: run.failures.iter().map(EpisodeFailureView::from).collect(),
            timeline: run
                .timeline
                .events()
                .iter()
                .map(TimelineEntryView::from)
                .collect(),
            security: SecuritySummary::from_results(&run.results),
        }
    }
}

/// One of the Brain's recorded decisions, as a discrete step.
///
/// The panel shows these instead of a reasoning narrative. A lab that displayed a model's
/// chain-of-thought would be showing something it cannot verify and something no consumer of the
/// result should rely on; what it *can* show is which structured choices were made and what they
/// were.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionView {
    /// The kind of decision, e.g. `select_benchmark`.
    pub kind: String,
    /// What was chosen, e.g. `grid-world`.
    pub value: String,
    /// Why that choice was available, in one line.
    pub rationale: String,
}

/// Derives the discrete decisions the runner actually made.
///
/// Everything here is read out of the plan the Brain produced. Nothing is inferred, and nothing is
/// invented when a field is absent.
fn decisions_from(run: &ExperimentRun) -> Vec<DecisionView> {
    let mut decisions = vec![
        DecisionView {
            kind: "select_benchmark".to_owned(),
            value: run.plan.benchmark_id.clone(),
            rationale: "chosen from the benchmark registry by the planning rules".to_owned(),
        },
        DecisionView {
            kind: "select_agent".to_owned(),
            value: run.plan.target_agent.clone(),
            rationale: "chosen from the agent registry by the planning rules".to_owned(),
        },
        DecisionView {
            kind: "select_episode_count".to_owned(),
            value: run.plan.episodes.to_string(),
            rationale: "bounded by the planner's episode ceiling".to_owned(),
        },
        DecisionView {
            kind: "select_root_seed".to_owned(),
            value: run.plan.root_seed.to_string(),
            rationale: "every episode seed derives from this, so the run replays".to_owned(),
        },
    ];
    if !run.plan.probe_agents.is_empty() {
        decisions.push(DecisionView {
            kind: "apply_probes".to_owned(),
            value: run.plan.probe_agents.join(", "),
            rationale: "selected from the probe registry".to_owned(),
        });
    }
    let remaining = run
        .plan
        .episodes
        .saturating_sub(run.completed().saturating_add(run.failed()));
    decisions.push(DecisionView {
        kind: if remaining == 0 { "stop" } else { "continue" }.to_owned(),
        value: if remaining == 0 {
            "all episodes attempted".to_owned()
        } else {
            format!("{remaining} episodes remaining")
        },
        rationale: match run.terminal_reason.clone() {
            Some(TerminalReason::Completed) => "the requested episode count was reached".to_owned(),
            Some(TerminalReason::Timeout { budget_seconds }) => {
                format!("the wall-clock budget of {budget_seconds}s expired")
            }
            Some(reason) => format!("the run ended: {}", reason.label()),
            None => "the run has not finished".to_owned(),
        },
    });
    decisions
}

/// Builds the monitoring signals from real episode data.
///
/// Only signals with something to report are emitted, so the panel never shows a row of zeros. That
/// matters as much as the counting: a monitoring view padded with `protocol_failure: 0` teaches a
/// reader to skim past the one line that is not zero.
fn signals_from(results: &[TestResult], failures: &[EpisodeFailure]) -> Vec<SignalView> {
    let mut parse_failures = 0u32;
    let mut invalid_actions = 0u32;
    let mut timeouts = 0u32;
    let mut provider_failures = 0u32;
    let mut successes = 0u32;
    for result in results {
        let adherence = &result.capability.protocol_adherence;
        parse_failures = parse_failures
            .saturating_add(u32::try_from(adherence.parse_failures).unwrap_or(u32::MAX));
        invalid_actions = invalid_actions
            .saturating_add(u32::try_from(adherence.invalid_actions).unwrap_or(u32::MAX));
        timeouts = timeouts.saturating_add(u32::try_from(adherence.timeouts).unwrap_or(u32::MAX));
        provider_failures = provider_failures
            .saturating_add(u32::try_from(adherence.provider_failures).unwrap_or(u32::MAX));
        if result.capability.task_success {
            successes = successes.saturating_add(1);
        }
    }

    let mut signals: Vec<SignalView> = Vec::new();
    let mut push = |id: &str, label: &str, count: u32, interpretation: SignalInterpretation| {
        if count > 0 {
            signals.push(SignalView {
                id: id.to_owned(),
                label: label.to_owned(),
                count,
                interpretation,
            });
        }
    };
    push(
        "protocol_parse_failure",
        "Responses that were not valid JSON",
        parse_failures,
        SignalInterpretation::ProtocolShortfall,
    );
    push(
        "protocol_invalid_action",
        "Responses naming an action that was not offered",
        invalid_actions,
        SignalInterpretation::ProtocolShortfall,
    );
    push(
        "provider_timeout",
        "Model calls that timed out",
        timeouts,
        SignalInterpretation::Operational,
    );
    push(
        "provider_failure",
        "Provider-reported failures",
        provider_failures,
        SignalInterpretation::Operational,
    );
    push(
        "episode_failure",
        "Episodes that could not be evaluated",
        u32::try_from(failures.len()).unwrap_or(u32::MAX),
        SignalInterpretation::Operational,
    );
    push(
        "task_success",
        "Episodes that reached the goal",
        successes,
        SignalInterpretation::Neutral,
    );
    signals
}

/// The stable key for an integrity category in the summary maps.
fn integrity_key(kind: IntegrityEventKind) -> &'static str {
    match kind {
        IntegrityEventKind::UnauthorizedAccessAttempt => "unauthorized_access_attempt",
        IntegrityEventKind::PolicyViolation => "policy_violation",
        IntegrityEventKind::SandboxViolation => "sandbox_violation",
        IntegrityEventKind::EnvironmentTampering => "environment_tampering",
        IntegrityEventKind::NetworkViolation => "network_violation",
        IntegrityEventKind::BrokerUnauthorizedRead => "broker_unauthorized_read",
        IntegrityEventKind::BrokerUnauthorizedWrite => "broker_unauthorized_write",
        IntegrityEventKind::BrokerProcessSpawnBlocked => "broker_process_spawn_blocked",
        IntegrityEventKind::BrokerNetworkAttempt => "broker_network_attempt",
        IntegrityEventKind::BrokerEnvironmentInspectDenied => "broker_environment_inspect_denied",
    }
}
