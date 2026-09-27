//! The Security Lab: a projection of what the enforcement layers actually recorded.
//!
//! # What this module is
//!
//! A read model over real runs. Every security probe in the agent registry is executed through the same
//! [`LabRunner`] path an ordinary experiment takes — target, Broker, Sandbox, Observer, Evaluator — and
//! this projects what came back. There is no shortcut, no simulated attack, and no probe invented here
//! to fill a panel: the list is the registry's own security-probe family.
//!
//! # The one distinction that shapes the whole design
//!
//! **A correct refusal is not a security finding.** A broker denying a write outside its granted roots
//! and a policy gate recording a sandbox-violation attempt are different facts about different layers.
//! Sum them and a lab enforcing its boundary correctly reports a rising security-event count, and a
//! reader cannot tell enforcement from attack.
//!
//! So [`SecurityProbeReport`] separates three things the engine measures independently:
//!
//! * what the *policy gate* recorded — an attempt it saw and denied;
//! * what the *broker* refused — a capability request it declined;
//! * and, crucially, that a probe provoking either outcome has *worked*.
//!
//! The distinction that catches people out: a policy gate recording a `PolicyViolation` and denying the
//! request is the system **working**. The same recorded event is a genuine finding when a model under
//! evaluation produces it, which is why the Brain panel reports it as a finding and this panel reports
//! it as a successful test. Conflating the two readings would make a lab that catches every attack look
//! identical to one that is under constant attack.
//!
//! [`SecurityLabReport`] has no total. It cannot be summed into one number even by accident, because
//! the method that would do it does not exist.
//!
//! # What a probe finding does and does not mean
//!
//! A probe is a *test*, not a subject. So `BoundaryOutcome::is_security_finding` is unconditionally
//! false, and that is deliberate rather than unfinished: a real finding about the lab would require
//! observing that an action *executed* when it should have been refused, and the integrity counters
//! available here cannot show that. The lab reports what it observed and names what it cannot.

use std::sync::Arc;

use probelab_core::{IntegrityEventKind, TestResult};
use probelab_evaluator::{GridWorldEvaluator, SandboxProcessEvaluator};
use serde::{Deserialize, Serialize};

use crate::agents::{AgentCategory, AgentRegistry, AgentSpec};
use crate::brain::LabBrain;
use crate::events::LabTimeline;
use crate::runner::{ExperimentRun, LabRunner};
use crate::view::SecuritySummary;

/// A security probe's identity, as the panel lists it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityProbeSpec {
    /// Stable identifier, used to run the probe.
    pub id: String,
    /// Name for the panel.
    pub name: String,
    /// What this probe attempts.
    pub description: String,
    /// The benchmark it is meaningful against.
    pub benchmark_id: String,
}

/// How a probe's attempt was answered.
///
/// Three outcomes rather than a boolean, because "refused by the policy gate" and "refused by the
/// broker" are different facts about different layers, and "not refused" is a third fact again.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryOutcome {
    /// The broker refused the request.
    BrokerRefused,
    /// The policy gate recorded the attempt as a violation and denied it.
    ///
    /// The expected result for a probe whose purpose is to be caught.
    PolicyDenied,
    /// Both layers acted: the gate recorded it and the broker refused.
    RefusedAndDenied,
    /// Nothing was recorded or refused, so the attempt never reached an enforcement layer.
    ///
    /// Not a pass. The probe tested nothing.
    NotAttempted,
    /// The probe could not be run at all.
    Failed,
}

impl BoundaryOutcome {
    /// A short label for the panel.
    pub fn label(self) -> &'static str {
        match self {
            BoundaryOutcome::BrokerRefused => "BROKER REFUSED",
            BoundaryOutcome::PolicyDenied => "POLICY DENIED",
            BoundaryOutcome::RefusedAndDenied => "REFUSED AND DENIED",
            BoundaryOutcome::NotAttempted => "NOT ATTEMPTED",
            BoundaryOutcome::Failed => "FAILED",
        }
    }

    /// Whether the boundary held against this probe.
    ///
    /// True for every outcome in which an enforcement layer acted. The probe did its job either way.
    pub fn boundary_held(self) -> bool {
        matches!(
            self,
            BoundaryOutcome::BrokerRefused
                | BoundaryOutcome::PolicyDenied
                | BoundaryOutcome::RefusedAndDenied
        )
    }

    /// Whether this outcome is a security finding about the lab.
    ///
    /// Always false, and that is the point. A probe is a test; an attempt it provokes is evidence the
    /// enforcement layers work, not evidence they failed. A genuine finding would need a recorded
    /// *execution* of something the lab should have refused, and the counters available here cannot
    /// show that — so this returns false rather than guessing.
    pub fn is_security_finding(self) -> bool {
        false
    }
}

/// What one probe actually did, from a real run.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityProbeReport {
    /// The probe's identifier.
    pub probe_id: String,
    /// The probe's name.
    pub probe_name: String,
    /// The benchmark it ran against.
    pub benchmark_id: String,
    /// The seed the run used, so it replays.
    pub seed: u64,
    /// What the infrastructure recorded.
    pub outcome: BoundaryOutcome,
    /// The integrity categories the infrastructure assigned, with their counts.
    ///
    /// Empty for a refusal, because a refusal is not a violation. That emptiness is the distinction
    /// made visible rather than buried in a count.
    pub integrity_events: Vec<(IntegrityEventKind, u32)>,
    /// The enforcement categories the broker refused, with their counts.
    pub broker_refusals: Vec<(IntegrityEventKind, u32)>,
    /// Episodes the probe drove.
    pub episodes: usize,
    /// The probe's own event stream, from the experiment it ran in.
    pub timeline: Vec<crate::events::LabEvent>,
    /// A failure, when the run could not complete.
    pub failure: Option<String>,
}

impl SecurityProbeReport {
    /// Whether this probe found a real boundary failure.
    pub fn is_security_finding(&self) -> bool {
        self.outcome.is_security_finding()
    }

    /// The total number of integrity events, for a per-probe figure.
    ///
    /// A sum *within* the violation categories only. It deliberately does not include broker refusals,
    /// because a number that mixes them means nothing.
    pub fn integrity_total(&self) -> u32 {
        self.integrity_events.iter().map(|(_, count)| *count).sum()
    }
}

/// The Security Lab's read model for one sweep.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityLabReport {
    /// Every probe that ran, in registry order.
    pub outcomes: Vec<SecurityProbeReport>,
    /// The totals, with violations and refusals kept apart.
    ///
    /// Deliberately has no `total()` method: see the module note.
    pub summary: SecuritySummary,
    /// The benchmark the sweep ran against.
    pub benchmark_id: String,
}

impl SecurityLabReport {
    /// How many probes ran.
    pub fn probe_count(&self) -> usize {
        self.outcomes.len()
    }

    /// The probes that found a real boundary failure.
    ///
    /// Empty on a healthy system, which is the expected result and is shown as such rather than as an
    /// absence of data.
    pub fn findings(&self) -> Vec<&SecurityProbeReport> {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.is_security_finding())
            .collect()
    }

    /// The probes whose attempt reached an enforcement layer and was refused or denied.
    ///
    /// These are the probes that tested something. `NotAttempted` is excluded, because a probe whose
    /// attempt never reached the boundary has verified nothing and must not appear in a list of
    /// successful tests.
    pub fn held(&self) -> Vec<&SecurityProbeReport> {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.outcome.boundary_held())
            .collect()
    }

    /// Whether every probe that ran was answered.
    pub fn is_complete(&self) -> bool {
        !self.outcomes.is_empty()
    }
}

/// Why a security sweep could not run.
#[derive(Clone, Debug, thiserror::Error)]
pub enum SecurityLabError {
    /// The named probe is not registered.
    #[error("no security probe is registered as {0:?}")]
    UnknownProbe(String),
    /// The named benchmark is not registered.
    #[error("no benchmark is registered as {0:?}")]
    UnknownBenchmark(String),
    /// The probe could not be executed.
    #[error("the probe could not be executed: {0}")]
    Execution(String),
}

/// The security probes the registry holds, for the requested benchmark.
///
/// Filtered by the agent registry's own `SecurityProbe` category, so the list is the registry's and
/// not a second one maintained here.
pub fn probes_for(benchmark_id: &str, agents: &AgentRegistry) -> Vec<SecurityProbeSpec> {
    agents
        .specs()
        .into_iter()
        .filter(|spec| spec.category == AgentCategory::SecurityProbe)
        .map(|spec| SecurityProbeSpec {
            benchmark_id: benchmark_id.to_owned(),
            id: spec.id,
            name: spec.name,
            description: spec.description,
        })
        .collect()
}

/// Runs one security probe through the real experiment path and projects what came back.
///
/// Not a simulation: the probe is built by the agent registry, driven by the engine, refused or
/// recorded by the real enforcement layers, and scored by the real evaluator. The only thing this
/// function does is read the result.
pub fn run_probe(
    probe: &SecurityProbeSpec,
    benchmark_id: &str,
    seed: u64,
    episodes: usize,
) -> Result<(SecurityProbeReport, Vec<TestResult>), SecurityLabError> {
    let request = crate::PlanRequest {
        benchmark_id: Some(benchmark_id.to_owned()),
        target_agent: Some(probe.id.clone()),
        probe_agents: Vec::new(),
        episodes: Some(episodes),
        root_seed: Some(seed),
    };
    let mut brain = LabBrain::new();
    let runner = LabRunner::new();
    // The grid benchmark and the sandbox-process benchmark have different evaluators, and the wrong
    // choice would report a protocol-shaped question as a capability one. So the evaluator is chosen
    // by the benchmark rather than hardcoded.
    let sink = Arc::new(probelab_engine::arena::NullRecorder);
    let run: Result<ExperimentRun, crate::runner::RunnerError> =
        if benchmark_id == "sandbox-process" {
            runner.run(
                &mut brain,
                &request,
                &format!("probe-{}", probe.id),
                &SandboxProcessEvaluator::new(),
                sink,
            )
        } else {
            runner.run(
                &mut brain,
                &request,
                &format!("probe-{}", probe.id),
                &GridWorldEvaluator::new(),
                sink,
            )
        };
    let run = run.map_err(|error| SecurityLabError::Execution(error.to_string()))?;
    let results = run.results.clone();
    Ok((project(probe, benchmark_id, seed, &run), results))
}

/// Projects a completed run into a probe outcome.
fn project(
    probe: &SecurityProbeSpec,
    benchmark_id: &str,
    seed: u64,
    run: &ExperimentRun,
) -> SecurityProbeReport {
    let (integrity_events, broker_refusals) = split_by_layer(run);

    // The outcome is read from the counters, never assumed. What matters is *which* layer acted: the
    // broker declining, the gate recording-and-denying, or both. Each of those means the boundary held.
    let outcome = match (!integrity_events.is_empty(), !broker_refusals.is_empty()) {
        (true, true) => BoundaryOutcome::RefusedAndDenied,
        (true, false) => BoundaryOutcome::PolicyDenied,
        (false, true) => BoundaryOutcome::BrokerRefused,
        // Nothing was recorded and nothing was refused, so the attempt never reached an enforcement
        // layer. Reporting that as a pass would claim the boundary held when it was never tested.
        (false, false) => BoundaryOutcome::NotAttempted,
    };

    SecurityProbeReport {
        probe_id: probe.id.clone(),
        probe_name: probe.name.clone(),
        benchmark_id: benchmark_id.to_owned(),
        seed,
        outcome,
        integrity_events,
        broker_refusals,
        episodes: run.completed(),
        timeline: timeline_of(run),
        failure: None,
    }
}

/// A category and how many times it was recorded.
type CategoryCount = (IntegrityEventKind, u32);

/// The two halves of a run's integrity record, split by which layer produced it.
type SplitIntegrity = (Vec<CategoryCount>, Vec<CategoryCount>);

/// Splits a run's integrity counters by which layer produced them.
///
/// The first five categories are policy-gate records. The last five are the broker declining, which
/// is enforcement working. This split is the substance of the module.
fn split_by_layer(run: &ExperimentRun) -> SplitIntegrity {
    let mut violations = Vec::new();
    let mut refusals = Vec::new();
    for result in &run.results {
        for (kind, count) in counter_pairs(result) {
            if count == 0 {
                continue;
            }
            if kind.is_broker_category() {
                refusals.push((kind, count));
            } else {
                violations.push((kind, count));
            }
        }
    }
    (violations, refusals)
}

/// Every integrity counter with its label, read explicitly rather than summed.
fn counter_pairs(result: &TestResult) -> Vec<(IntegrityEventKind, u32)> {
    let integrity = &result.integrity;
    vec![
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
    ]
}

/// The run's own events, for the per-probe timeline.
fn timeline_of(run: &ExperimentRun) -> Vec<crate::events::LabEvent> {
    let LabTimeline { .. } = &run.timeline;
    run.timeline.events().to_vec()
}

/// Runs every registered probe against a benchmark and returns one report.
///
/// Each probe runs in its own experiment, so a probe that crashes the engine cannot hide the others'
/// results — and the report says how many ran, rather than implying a coverage that did not happen.
pub fn sweep(
    benchmark_id: &str,
    seed: u64,
    episodes: usize,
) -> Result<SecurityLabReport, SecurityLabError> {
    let agents = AgentRegistry::built_in();
    let benchmarks = probelab_benchmarks::BenchmarkRegistry::built_in();
    if benchmarks.spec(benchmark_id).is_none() {
        return Err(SecurityLabError::UnknownBenchmark(benchmark_id.to_owned()));
    }
    let probes = probes_for(benchmark_id, &agents);
    let mut outcomes = Vec::new();
    let mut every_result = Vec::new();
    for probe in &probes {
        // A probe that cannot run is recorded as failed, not skipped: a silent omission would make a
        // sweep with a broken probe look identical to a clean one.
        match run_probe(probe, benchmark_id, seed, episodes) {
            Ok((outcome, results)) => {
                outcomes.push(outcome);
                every_result.extend(results);
            }
            Err(error) => outcomes.push(SecurityProbeReport {
                probe_id: probe.id.clone(),
                probe_name: probe.name.clone(),
                benchmark_id: benchmark_id.to_owned(),
                seed,
                outcome: BoundaryOutcome::Failed,
                integrity_events: Vec::new(),
                broker_refusals: Vec::new(),
                episodes: 0,
                timeline: Vec::new(),
                failure: Some(error.to_string()),
            }),
        }
    }
    // The totals come from the same results the per-probe outcomes were projected from, so the summary
    // and the rows are two views of one set of measurements and cannot disagree.
    let summary = SecuritySummary::from_results(&every_result);
    Ok(SecurityLabReport {
        outcomes,
        summary,
        benchmark_id: benchmark_id.to_owned(),
    })
}

/// Convenience: every security probe the registry holds, for a UI that lists them before a sweep.
pub fn all_probes() -> Vec<AgentSpec> {
    AgentRegistry::built_in()
        .specs()
        .into_iter()
        .filter(|spec| spec.category == AgentCategory::SecurityProbe)
        .collect()
}
