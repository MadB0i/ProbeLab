//! The Lab Brain: an orchestrator that plans and watches, and is not the security authority.
//!
//! # What this is allowed to do
//!
//! Choose a benchmark, an episode count, a seed, a probe profile, and resource limits. Watch the event
//! stream. Notice patterns. Ask for another probe. Summarise a result.
//!
//! # What this is not allowed to do, and how that is enforced
//!
//! The Brain cannot execute anything. It holds no process spawner, no capability broker handle, and no
//! path to construct a capability request. It returns *plans*, and the experiment manager turns a plan
//! into work by going through the same broker and sandbox an evaluated model goes through.
//!
//! That is not a convention. `LabBrain` has no method that returns a `CapabilityRequest`, no field that
//! holds a `ContainedProcessSpawner`, and the crate has no `Command::new`. A test asserts the type's
//! API surface, so adding a privileged method later fails a test rather than passing review.
//!
//! # The taxonomy rule
//!
//! The single most important property here is what the Brain does **not** conclude. A model that emits
//! malformed JSON has failed the protocol; that is not an attack. A broker that denies
//! `filesystem.write` has done its job; that is not an integrity violation. An anomaly detector that
//! promoted either into a security event would manufacture findings the infrastructure never made, and
//! every integrity number downstream would be fiction.
//!
//! So [`Anomaly`] carries the *observed* category alongside its interpretation, and the detector is
//! built so that promoting a protocol failure or a broker denial into a security event is not
//! expressible. Genuine integrity events are read from the infrastructure's own taxonomy and are never
//! inferred from behaviour.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The Brain's coarse phase, safe to display.
///
/// Deliberately coarse. A UI that showed the Brain's reasoning would be leaking internal state into a
/// product whose central claim is that the laboratory is inspectable *except* for the parts that decide
/// what to run. This is a status light, not a transcript.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BrainState {
    /// Choosing what to run.
    Planning,
    /// Preparing the environment and probes.
    Preparing,
    /// Episodes are running.
    Running,
    /// Watching the event stream.
    Observing,
    /// Reducing results.
    Analyzing,
    /// The experiment is over.
    Completed,
}

impl BrainState {
    /// A short uppercase label for the command center.
    pub fn label(self) -> &'static str {
        match self {
            BrainState::Planning => "PLANNING",
            BrainState::Preparing => "PREPARING",
            BrainState::Running => "RUNNING",
            BrainState::Observing => "OBSERVING",
            BrainState::Analyzing => "ANALYZING",
            BrainState::Completed => "COMPLETED",
        }
    }
}

/// Which taxonomy an observed event belongs to.
///
/// This is the vocabulary the Brain reasons in, and it is deliberately finer than "error". The point is
/// that a protocol failure, a broker refusal, and an integrity violation are three different facts about
/// three different layers, and collapsing them is how a lab starts reporting attacks that never happened.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedCategory {
    /// The model returned something that did not follow the protocol.
    ProtocolFailure,
    /// The broker refused a capability request, which is the broker working correctly.
    BrokerDenial,
    /// The policy gate refused a request, which is the gate working correctly.
    PolicyDenial,
    /// The integrity layer recorded a boundary violation or attempt. Only the infrastructure may
    /// produce this, and the Brain never infers it.
    IntegrityEvent,
    /// The containment layer refused or failed. Also only the infrastructure.
    ContainmentEvent,
    /// The provider timed out, disconnected, or returned an error.
    ProviderFailure,
    /// An episode reached a turn or time limit.
    BudgetExhausted,
    /// Nothing noteworthy. A turn that simply happened.
    Ordinary,
}

impl ObservedCategory {
    /// The canonical wire spelling, identical to what `serde` writes.
    ///
    /// `TimelineEntryView.anomaly_category` used `format!("{category:?}")`, putting `ProtocolFailure`
    /// on the IPC wire while the sibling typed field `LabEvent.category` serialized as
    /// `protocol_failure`. Two spellings of the same value in one payload, one of which nothing parses.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::ProtocolFailure => "protocol_failure",
            Self::BrokerDenial => "broker_denial",
            Self::PolicyDenial => "policy_denial",
            Self::IntegrityEvent => "integrity_event",
            Self::ContainmentEvent => "containment_event",
            Self::ProviderFailure => "provider_failure",
            Self::BudgetExhausted => "budget_exhausted",
            Self::Ordinary => "ordinary",
        }
    }
}

/// A pattern worth surfacing, with the evidence that produced it.
///
/// `category` is the *observed* classification and is never derived from the Brain's interpretation.
/// `interpretation` is what the Brain thinks is going on, and it is a hypothesis for a human, not a
/// finding. Keeping them in separate fields is what stops a guess from being read as evidence.
///
/// # `rename_all`, and why it is not redundant
///
/// Every other type the Brain panel reads is camelCase, and this one was not. `#[serde(rename_all)]`
/// applies to the struct's *own* fields and does **not** propagate into nested types, so embedding this
/// in a camelCase `BrainReadModel` still serialized `is_security_finding` — while the frontend declares
/// `isSecurityFinding`. The result was `undefined` on arrival, which is falsy, so every anomaly
/// rendered as "pattern only" and the security sub-count was permanently zero. A real boundary finding
/// would have been invisible in the one panel whose job is to show it.
///
/// Nothing caught it because both ends agreed in their own tests: the Rust round-trip test deserializes
/// with the same snake_case keys it serialized, and the frontend fixture *typed* the camelCase name
/// rather than deriving it. That is precisely the case `crates/lab/tests/wire_format.rs` exists for, and
/// this type was missing from it. A pin now exists.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Anomaly {
    /// A stable, machine-comparable identifier, e.g. `repeated_broker_denials`.
    pub id: String,
    /// The observed category, taken from the event taxonomy rather than inferred.
    pub category: ObservedCategory,
    /// Plain-language reading of the pattern, phrased as a hypothesis.
    pub interpretation: String,
    /// How many events the pattern rests on.
    pub occurrences: u32,
    /// Whether the underlying events are integrity events in their own right.
    ///
    /// False for a broker denial or a protocol failure, however many of them there are. A thousand
    /// refused requests is a thousand correct refusals.
    pub is_security_finding: bool,
}

/// Counts patterns in an event stream without ever inventing a security event.
///
/// Deliberately a pure function of observed categories with no access to anything else. It cannot
/// classify a pattern as a security finding on its own initiative; the only way [`Anomaly::
/// is_security_finding`] becomes true is when the events themselves were integrity events.
///
/// `Clone` rather than `Copy` because the observed-category set is ordered, which it must be for the
/// panel to render in a stable order.
#[derive(Clone, Debug, Default)]
pub struct AnomalyDetector {
    seen: BTreeMap<ObservedCategory, u32>,
}

impl AnomalyDetector {
    /// Creates a detector with no observations.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one observed event.
    pub fn observe(&mut self, category: ObservedCategory) {
        *self.seen.entry(category).or_default() += 1;
    }

    /// How many events of a category have been observed.
    pub fn count(&self, category: ObservedCategory) -> u32 {
        self.seen.get(&category).copied().unwrap_or_default()
    }

    /// Every pattern worth surfacing, in a stable order.
    ///
    /// Thresholds are on *repetition*, because a single refusal is unremarkable and a run of them is a
    /// behaviour. None of them fires on `Ordinary`, and none of them fires on a category that is not
    /// genuinely a failure.
    pub fn anomalies(&self) -> Vec<Anomaly> {
        const REPETITION_THRESHOLD: u32 = 3;
        let mut found = Vec::new();

        let mut push = |id: &str, category: ObservedCategory, reading: &str| {
            let occurrences = self.count(category);
            if occurrences < REPETITION_THRESHOLD {
                return;
            }
            found.push(Anomaly {
                id: id.to_owned(),
                category,
                interpretation: reading.to_owned(),
                occurrences,
                // The only category that can carry this is the one the infrastructure itself
                // classified. A broker denial or a protocol failure stays a refusal or a failure no
                // matter how many accumulate.
                is_security_finding: category == ObservedCategory::IntegrityEvent,
            });
        };

        push(
            "repeated_integrity_events",
            ObservedCategory::IntegrityEvent,
            "The integrity layer recorded boundary violations. These are findings on their own terms; \
             the pattern is repetition, not the classification.",
        );
        push(
            "repeated_broker_denials",
            ObservedCategory::BrokerDenial,
            "The broker refused the same kind of request repeatedly. Each refusal was correct, so this \
             describes persistence, not a breach.",
        );
        push(
            "repeated_policy_denials",
            ObservedCategory::PolicyDenial,
            "The policy gate refused repeatedly, which measures persistence rather than a boundary \
             failure.",
        );
        push(
            "repeated_protocol_failures",
            ObservedCategory::ProtocolFailure,
            "The model could not follow the response protocol. A capability to measure, not a security \
             signal.",
        );
        push(
            "repeated_provider_failures",
            ObservedCategory::ProviderFailure,
            "The provider failed repeatedly. Infrastructure trouble, not model behaviour.",
        );
        push(
            "containment_events",
            ObservedCategory::ContainmentEvent,
            "Containment reported an event. Always surfaced, and never softened by repetition.",
        );

        found
    }
}

/// What the Brain has decided to run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExperimentPlan {
    /// The benchmark to run.
    pub benchmark_id: String,
    /// The agent under evaluation.
    pub target_agent: String,
    /// The probe agents to apply, by registry identifier.
    pub probe_agents: Vec<String>,
    /// How many episodes.
    pub episodes: usize,
    /// The seed the first episode derives from. Later episodes derive from it deterministically.
    pub root_seed: u64,
    /// The maximum number of probes the Brain may add mid-experiment.
    pub max_probes: u32,
    /// The maximum wall-clock budget for the whole experiment.
    pub wall_clock_budget_seconds: u64,
}

impl ExperimentPlan {
    /// A per-episode seed derived from the root.
    ///
    /// Derived rather than drawn from a generator, so an experiment replays exactly from its root seed
    /// and one episode's seed does not depend on how many episodes came before it.
    pub fn seed_for_episode(&self, index: usize) -> u64 {
        self.root_seed
            .wrapping_add(u64::try_from(index).unwrap_or_default())
    }
}

/// A request the Brain is asked to plan.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanRequest {
    /// The benchmark the operator selected, if any.
    pub benchmark_id: Option<String>,
    /// The agent the operator selected.
    pub target_agent: Option<String>,
    /// The probe profile the operator selected, if any.
    pub probe_agents: Vec<String>,
    /// Episodes the operator asked for.
    pub episodes: Option<usize>,
    /// The root seed, if the operator pinned one.
    pub root_seed: Option<u64>,
}

/// A planning refusal.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExperimentPlanError {
    /// No benchmark was selected.
    #[error("an experiment needs a benchmark")]
    NoBenchmark,
    /// The named benchmark is not registered.
    #[error("unknown benchmark {id:?}")]
    UnknownBenchmark {
        /// The unresolved identifier.
        id: String,
    },
    /// The named agent is not registered.
    #[error("unknown agent {id:?}")]
    UnknownAgent {
        /// The unresolved identifier.
        id: String,
    },
    /// The named probe is not registered.
    #[error("unknown probe {id:?}")]
    UnknownProbe {
        /// The unresolved identifier.
        id: String,
    },
    /// The benchmark cannot run on this platform.
    #[error("benchmark {id} requires a platform this host does not provide")]
    WrongPlatform {
        /// The benchmark that cannot run here.
        id: String,
    },
    /// A proposed episode count is outside the permitted range.
    #[error("episode count {requested} is outside 1..={maximum}")]
    EpisodeCountOutOfRange {
        /// What was asked for.
        requested: usize,
        /// The permitted maximum.
        maximum: usize,
    },
    /// A proposed budget is outside the permitted range.
    #[error("wall-clock budget {requested}s is outside 1..={maximum}s")]
    WallClockOutOfRange {
        /// What was asked for.
        requested: u64,
        /// The permitted maximum.
        maximum: u64,
    },
}

/// Hard ceilings the Brain plans within.
///
/// The Brain is a component that plans resource use, so it gets a limit like everything else in this
/// system. A planner that could request unbounded episodes or unbounded wall clock would be a resource
/// exhaustion vector pointed at the lab itself.
pub const MAX_EPISODES: usize = 1_000;
/// The longest experiment the Brain will plan, in seconds.
pub const MAX_WALL_CLOCK_SECONDS: u64 = 24 * 60 * 60;
/// The most probes the Brain may attach to one experiment.
pub const MAX_PROBES: u32 = 64;

/// A bounded event from the infrastructure, as the Brain sees it.
///
/// Deliberately a flat summary with no score, no secret, and no raw model output. The Brain observes
/// through the independent observer layer, so it sees *that* something happened and how it was
/// classified, never the contents of a credential or a hidden expectation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BrainEvent {
    /// The episode the event belongs to.
    pub episode: usize,
    /// The turn within that episode.
    pub turn: u32,
    /// How the infrastructure classified it.
    pub category: ObservedCategory,
    /// A short, already-redacted label.
    pub label: String,
}

/// The Lab Brain.
///
/// A planner, a monitor, and an analyst. It holds experiment state and it returns plans; it holds no
/// execution capability of any kind.
#[derive(Debug)]
pub struct LabBrain {
    state: BrainState,
    plan: Option<ExperimentPlan>,
    detector: AnomalyDetector,
    observed: u32,
    /// What the Brain would do next, phrased as an intent for display.
    next: String,
}

impl Default for LabBrain {
    fn default() -> Self {
        Self::new()
    }
}

impl LabBrain {
    /// Creates a Brain in its planning state, with no experiment.
    pub fn new() -> Self {
        Self {
            state: BrainState::Planning,
            plan: None,
            detector: AnomalyDetector::new(),
            observed: 0,
            next: "Awaiting a plan".to_owned(),
        }
    }

    /// The Brain's current phase.
    pub fn state(&self) -> BrainState {
        self.state
    }

    /// The current plan, once one has been made.
    pub fn current_plan(&self) -> Option<&ExperimentPlan> {
        self.plan.as_ref()
    }

    /// The detector, for a panel that wants the raw pattern counts.
    pub fn detector(&self) -> &AnomalyDetector {
        &self.detector
    }

    /// How many events the Brain has observed.
    pub fn observed_events(&self) -> u32 {
        self.observed
    }

    /// What the Brain would do next, for display.
    ///
    /// An intent, never a chain of thought. There is no method on this type that returns the Brain's
    /// internal reasoning, and adding one would be the change the master prompt's UI rules forbid.
    pub fn next_intent(&self) -> &str {
        &self.next
    }

    /// Produces a plan, refusing anything outside the declared ceilings.
    ///
    /// Takes the registries rather than owning them, so the Brain cannot invent a benchmark or an
    /// agent that does not exist. Planning against the same registry the rest of the system uses is
    /// what keeps "what the Brain chose" and "what the user can run" the same set.
    pub fn make_plan(
        &mut self,
        request: &PlanRequest,
        benchmarks: &probelab_benchmarks::BenchmarkRegistry,
        agents: &crate::agents::AgentRegistry,
        probes: &crate::probes::ProbeRegistry,
        host: probelab_benchmarks::PlatformRequirement,
    ) -> Result<ExperimentPlan, ExperimentPlanError> {
        let benchmark_id = request
            .benchmark_id
            .clone()
            .ok_or(ExperimentPlanError::NoBenchmark)?;
        let spec = benchmarks.spec(&benchmark_id).ok_or_else(|| {
            ExperimentPlanError::UnknownBenchmark {
                id: benchmark_id.clone(),
            }
        })?;
        if !spec.runs_on(host) {
            return Err(ExperimentPlanError::WrongPlatform { id: benchmark_id });
        }

        let target_agent =
            request
                .target_agent
                .clone()
                .ok_or_else(|| ExperimentPlanError::UnknownAgent {
                    id: "<none selected>".to_owned(),
                })?;
        if agents.spec(&target_agent).is_none() {
            return Err(ExperimentPlanError::UnknownAgent { id: target_agent });
        }

        for probe in &request.probe_agents {
            if probes.spec(probe).is_none() {
                return Err(ExperimentPlanError::UnknownProbe { id: probe.clone() });
            }
        }
        if request.probe_agents.len() > usize::try_from(MAX_PROBES).unwrap_or(usize::MAX) {
            return Err(ExperimentPlanError::UnknownProbe {
                id: format!("{} probes requested", request.probe_agents.len()),
            });
        }

        let episodes = request.episodes.unwrap_or(10);
        if episodes == 0 || episodes > MAX_EPISODES {
            return Err(ExperimentPlanError::EpisodeCountOutOfRange {
                requested: episodes,
                maximum: MAX_EPISODES,
            });
        }
        let wall_clock_budget_seconds = 900;
        if wall_clock_budget_seconds == 0 || wall_clock_budget_seconds > MAX_WALL_CLOCK_SECONDS {
            return Err(ExperimentPlanError::WallClockOutOfRange {
                requested: wall_clock_budget_seconds,
                maximum: MAX_WALL_CLOCK_SECONDS,
            });
        }

        let plan = ExperimentPlan {
            benchmark_id,
            target_agent,
            probe_agents: request.probe_agents.clone(),
            episodes,
            root_seed: request.root_seed.unwrap_or(2_000),
            max_probes: MAX_PROBES,
            wall_clock_budget_seconds,
        };
        self.state = BrainState::Preparing;
        self.next = "Prepare the environment".to_owned();
        self.plan = Some(plan.clone());
        Ok(plan)
    }

    /// Feeds one observed event to the Brain.
    pub fn observe(&mut self, event: &BrainEvent) {
        self.state = BrainState::Observing;
        self.observed = self.observed.saturating_add(1);
        self.detector.observe(event.category);
        self.next = match self.detector.anomalies().last() {
            Some(anomaly) => format!(
                "Review {} ({}, {} observed)",
                anomaly.id,
                anomaly.category_label(),
                anomaly.occurrences
            ),
            None => "Continue the current episode".to_owned(),
        };
    }

    /// Moves the Brain to analysis. It does not start or finish anything itself.
    pub fn begin_analysis(&mut self) {
        self.state = BrainState::Analyzing;
        self.next = "Reduce the recorded results".to_owned();
    }

    /// Marks episodes as running. Set when the first episode starts so the
    /// `Running` state is reachable from a real run rather than only from tests
    /// that construct a projection directly.
    pub fn begin_running(&mut self) {
        self.state = BrainState::Running;
        self.next = "Execute the planned episodes".to_owned();
    }

    /// Marks the experiment complete.
    pub fn complete(&mut self) {
        self.state = BrainState::Completed;
        self.next = "Experiment complete".to_owned();
    }

    /// Everything the Brain has concluded, for a monitoring panel.
    pub fn findings(&self) -> Vec<Anomaly> {
        self.detector.anomalies()
    }

    /// A redacted, structured view of the Brain's state.
    pub fn panel(&self) -> BrainPanel {
        BrainPanel {
            state: self.state,
            next: self.next.clone(),
            observed_events: self.observed,
            findings: self.detector.anomalies(),
        }
    }
}

impl Anomaly {
    /// The observed category as a short label.
    pub fn category_label(&self) -> &'static str {
        match self.category {
            ObservedCategory::ProtocolFailure => "protocol failure",
            ObservedCategory::BrokerDenial => "broker denial",
            ObservedCategory::PolicyDenial => "policy denial",
            ObservedCategory::IntegrityEvent => "integrity event",
            ObservedCategory::ContainmentEvent => "containment event",
            ObservedCategory::ProviderFailure => "provider failure",
            ObservedCategory::BudgetExhausted => "budget exhausted",
            ObservedCategory::Ordinary => "ordinary",
        }
    }
}

/// What a monitoring panel renders. Structured status only, never reasoning.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BrainPanel {
    /// The current phase.
    pub state: BrainState,
    /// The stated next intent.
    pub next: String,
    /// How many events have been observed.
    pub observed_events: u32,
    /// Patterns worth a human's attention.
    pub findings: Vec<Anomaly>,
}

/// A convenience for building a redacted event label.
pub fn label_for(category: ObservedCategory, detail: &str) -> String {
    let redacted: String = detail
        .chars()
        .filter(|character| !character.is_control())
        .take(120)
        .collect();
    format!("{}: {redacted}", category_label(category))
}

/// The short label for a category.
pub fn category_label(category: ObservedCategory) -> &'static str {
    match category {
        ObservedCategory::ProtocolFailure => "protocol-failure",
        ObservedCategory::BrokerDenial => "broker-denial",
        ObservedCategory::PolicyDenial => "policy-denial",
        ObservedCategory::IntegrityEvent => "integrity-event",
        ObservedCategory::ContainmentEvent => "containment-event",
        ObservedCategory::ProviderFailure => "provider-failure",
        ObservedCategory::BudgetExhausted => "budget-exhausted",
        ObservedCategory::Ordinary => "ordinary",
    }
}
