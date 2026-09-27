//! The agent registry: every agent a user or the Lab Brain can select, described once.
//!
//! # Why a registry and not another list
//!
//! Before this, an agent's identity lived in four unrelated places: a `MockModelConfig` enum variant, a
//! name string inside the policy's own descriptor, a hardcoded allowlist in the desktop IPC layer, and
//! a third hardcoded list in the React dropdown. Adding an agent meant editing all four, and forgetting
//! one produced a control that offered an agent the backend would then refuse.
//!
//! Here, one `AgentSpec` is the description. The UI renders from it, the experiment manager builds
//! through it, and a test proves every registered agent actually constructs and produces a decision.
//!
//! # The label that matters
//!
//! [`AgentCategory::ReferenceCeiling`] exists so the lab never calls its own policies "AI models". A
//! reference policy is a deterministic, trusted ceiling. Presenting it as an evaluated model would
//! inflate every comparison built on it, which is why the category is a distinct type rather than a
//! display string the UI can lose.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use probelab_agent::{ProviderAgent, ReferenceAgent, ReferenceStrategy};
use probelab_core::{Agent, ModelProvider};
use probelab_model_adapters::{
    GridWorldReference, LookaheadSolver, MemoryReactive, MockModelConfig, MockModelProvider,
    SandboxProcessReference, StatelessReactive,
};

/// What kind of agent this is.
///
/// The categories are not presentation. Which category an agent belongs to determines what a result
/// *means*: a security probe's integrity counts are a measurement of the sandbox, a reference policy's
/// score is a ceiling, and a target model's score is a capability claim. Collapsing them would make
/// "the reference agent scored 0.94" indistinguishable from "the model scored 0.94".
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCategory {
    /// An external model under evaluation.
    TargetModel,
    /// A deterministic policy whose only job is to attempt a boundary violation.
    SecurityProbe,
    /// A benign deterministic policy, used as a floor or a calibration point.
    DeterministicBaseline,
    /// A deterministic policy that misbehaves at the protocol layer rather than a boundary.
    ProtocolProbe,
    /// A search or reactive policy, used to test whether a benchmark is solvable.
    PolicyAgent,
    /// A trusted ceiling. Not a model, and never to be reported as one.
    ReferenceCeiling,
}

impl AgentCategory {
    /// A short label safe to show in a results table.
    pub fn label(self) -> &'static str {
        match self {
            AgentCategory::TargetModel => "TARGET MODEL",
            AgentCategory::SecurityProbe => "SECURITY PROBE",
            AgentCategory::DeterministicBaseline => "BASELINE",
            AgentCategory::ProtocolProbe => "PROTOCOL PROBE",
            AgentCategory::PolicyAgent => "POLICY AGENT",
            AgentCategory::ReferenceCeiling => "REFERENCE / CEILING",
        }
    }

    /// Whether results from this category may be described as a model's capability.
    ///
    /// Only [`AgentCategory::TargetModel`] may. Everything else is infrastructure whose score measures
    /// the *lab*, and the UI uses this to refuse the word "model" for them.
    pub fn measures_a_model(self) -> bool {
        self == AgentCategory::TargetModel
    }
}

/// How an agent reaches its decisions, which tells a reader what to expect from it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecution {
    /// Deterministic, in-process, no network.
    InProcessDeterministic,
    /// A real HTTP call to a configured provider.
    RemoteModel,
}

impl AgentExecution {
    /// Whether this agent can fail because of the network.
    pub fn requires_network(self) -> bool {
        self == AgentExecution::RemoteModel
    }
}

/// The complete, serializable description of one registered agent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentSpec {
    /// Stable identifier, unique in the registry.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// What the agent does and why it exists.
    pub description: String,
    /// Which family it belongs to.
    pub category: AgentCategory,
    /// How it decides.
    pub execution: AgentExecution,
    /// The benchmark identifiers it is meaningful for, empty meaning all of them.
    pub applicable_benchmarks: Vec<String>,
    /// The deterministic seed it is configured with, where that is meaningful.
    pub seed: u64,
}

type AgentFactory = Box<dyn Fn(u64) -> Box<dyn Agent> + Send + Sync>;

struct AgentEntry {
    spec: AgentSpec,
    factory: AgentFactory,
}

impl std::fmt::Debug for AgentEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentEntry")
            .field("spec", &self.spec)
            .finish_non_exhaustive()
    }
}

/// Every agent ProbeLab can run as a subject or a control.
#[derive(Debug)]
pub struct AgentRegistry {
    entries: BTreeMap<String, AgentEntry>,
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::built_in()
    }
}

impl AgentRegistry {
    /// Builds the registry from every agent compiled into this binary.
    pub fn built_in() -> Self {
        let mut registry = Self {
            entries: BTreeMap::new(),
        };
        for entry in baseline_entries().into_iter().chain(reference_entries()) {
            registry.entries.insert(entry.spec.id.clone(), entry);
        }
        registry
    }

    /// Every registered agent's description, in stable identifier order.
    pub fn specs(&self) -> Vec<AgentSpec> {
        self.entries
            .values()
            .map(|entry| entry.spec.clone())
            .collect()
    }

    /// Looks up one description by identifier.
    pub fn spec(&self, id: &str) -> Option<&AgentSpec> {
        self.entries.get(id).map(|entry| &entry.spec)
    }

    /// Builds a fresh agent for one episode sequence.
    pub fn build(&self, id: &str, seed: u64) -> Result<Box<dyn Agent>, AgentBuildError> {
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| AgentBuildError::UnknownAgent { id: id.to_owned() })?;
        Ok((entry.factory)(seed))
    }

    /// Registers an additional agent, for a benchmark or probe that needs one.
    pub fn register(
        &mut self,
        spec: AgentSpec,
        factory: impl Fn(u64) -> Box<dyn Agent> + Send + Sync + 'static,
    ) -> Result<(), AgentRegistrationError> {
        if self.entries.contains_key(&spec.id) {
            return Err(AgentRegistrationError::DuplicateId { id: spec.id });
        }
        self.entries.insert(
            spec.id.clone(),
            AgentEntry {
                spec,
                factory: Box::new(factory),
            },
        );
        Ok(())
    }

    /// Every agent in a category.
    pub fn in_category(&self, category: AgentCategory) -> Vec<AgentSpec> {
        self.specs()
            .into_iter()
            .filter(|spec| spec.category == category)
            .collect()
    }

    /// How many agents are registered.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A failure building an agent.
#[derive(Debug, thiserror::Error)]
pub enum AgentBuildError {
    /// No agent is registered under that identifier.
    #[error("unknown agent id {id:?}")]
    UnknownAgent {
        /// The unresolved identifier.
        id: String,
    },
}

/// A registration failure.
#[derive(Debug, thiserror::Error)]
pub enum AgentRegistrationError {
    /// Two agents claimed the same identifier.
    #[error("agent id {id} is already registered")]
    DuplicateId {
        /// The clashing identifier.
        id: String,
    },
}

fn spec(
    id: &str,
    name: &str,
    description: &str,
    category: AgentCategory,
    execution: AgentExecution,
    applicable_benchmarks: Vec<String>,
) -> AgentSpec {
    AgentSpec {
        id: id.to_owned(),
        name: name.to_owned(),
        description: description.to_owned(),
        category,
        execution,
        applicable_benchmarks,
        seed: 0,
    }
}

fn entry(spec: AgentSpec, factory: AgentFactory) -> AgentEntry {
    AgentEntry { spec, factory }
}

/// The ten deterministic mock strategies, split across their three honest categories.
fn baseline_entries() -> Vec<AgentEntry> {
    let describe = |config: MockModelConfig| -> (&'static str, &'static str, AgentCategory) {
        match config {
            MockModelConfig::Optimal => (
                "One-step greedy: take an adjacent reward, else an adjacent safe tile, else any \
                 legal action. A capable floor, not a ceiling.",
                "Greedy Baseline",
                AgentCategory::DeterministicBaseline,
            ),
            MockModelConfig::Random => (
                "Uniform random legal action, seeded so a run replays identically. The chance floor.",
                "Random Baseline",
                AgentCategory::DeterministicBaseline,
            ),
            MockModelConfig::GreedyMistake => (
                "Waits one turn then plays greedily, so a benchmark can be checked for sensitivity \
                 to a small handicap.",
                "Greedy With A Mistake",
                AgentCategory::DeterministicBaseline,
            ),
            MockModelConfig::InvalidActionSpam => (
                "Repeatedly attempts an illegal move. Exercises the protocol boundary and must be \
                 counted as invalid actions, never as an integrity violation.",
                "Invalid Action Spam",
                AgentCategory::ProtocolProbe,
            ),
            MockModelConfig::SecurityProbeUnauthorizedAccess => (
                "Requests the evaluator and benchmark source on every turn.",
                "Unauthorized Access Probe",
                AgentCategory::SecurityProbe,
            ),
            MockModelConfig::SecurityProbePolicyViolation => (
                "Requests a tool the policy forbids on every turn.",
                "Policy Violation Probe",
                AgentCategory::SecurityProbe,
            ),
            MockModelConfig::SecurityProbeSandboxViolation => (
                "Requests to escape the sandbox on every turn.",
                "Sandbox Escape Probe",
                AgentCategory::SecurityProbe,
            ),
            MockModelConfig::SecurityProbeEnvironmentTampering => (
                "Requests to mutate the trusted environment on every turn.",
                "Environment Tampering Probe",
                AgentCategory::SecurityProbe,
            ),
            MockModelConfig::SecurityProbeNetworkViolation => (
                "Requests network access on every turn.",
                "Network Violation Probe",
                AgentCategory::SecurityProbe,
            ),
            MockModelConfig::ForbiddenActionSpam => (
                "Names each of the five broker permissions in turn, then waits forever. Proves every \
                 permission is refused rather than one being quietly unimplemented.",
                "Forbidden Capability Spam",
                AgentCategory::SecurityProbe,
            ),
        }
    };

    [
        MockModelConfig::Optimal,
        MockModelConfig::Random,
        MockModelConfig::GreedyMistake,
        MockModelConfig::InvalidActionSpam,
        MockModelConfig::SecurityProbeUnauthorizedAccess,
        MockModelConfig::SecurityProbePolicyViolation,
        MockModelConfig::SecurityProbeSandboxViolation,
        MockModelConfig::SecurityProbeEnvironmentTampering,
        MockModelConfig::SecurityProbeNetworkViolation,
        MockModelConfig::ForbiddenActionSpam,
    ]
    .into_iter()
    .map(|config| {
        let (description, name, category) = describe(config);
        let id = mock_id(&config);
        entry(
            spec(
                &id,
                name,
                description,
                category,
                AgentExecution::InProcessDeterministic,
                Vec::new(),
            ),
            Box::new(move |seed| {
                Box::new(ProviderAgent::from_provider(MockModelProvider::new(
                    config, seed,
                ))) as Box<dyn Agent>
            }),
        )
    })
    .collect()
}

/// Builds a fresh in-process agent for a seed.
type PolicyFactory = fn(u64) -> Box<dyn Agent>;

/// Builds a fresh reference strategy for a seed.
type ReferenceFactory = fn(u64) -> Box<dyn ReferenceStrategy>;

/// A policy agent's registration data.
type PolicySeed = (
    &'static str,
    &'static str,
    &'static str,
    AgentCategory,
    PolicyFactory,
);

/// A benchmark-scoped reference agent's registration data.
type WrappedReference = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    ReferenceFactory,
);

/// The reference and policy agents, which is where the "never call this a model" rule is enforced.
fn reference_entries() -> Vec<AgentEntry> {
    let policy_seed: Vec<PolicySeed> = vec![
        (
            "lookahead_solver",
            "Lookahead Solver",
            "Triangulates the goal from distance readings it was handed, flood-fills only what it \
             has observed, and remembers where it has already been stuck. The raw search policy; \
             the registered ceiling that wraps it is `grid_world_reference`.",
            // A policy, not a ceiling. The distinction is not pedantic: a ceiling must publish the
            // non-privilege claim in its descriptor, and this raw provider does not, so calling it
            // one would be a claim its own descriptor contradicts. The wrapped, registered form is
            // `grid_world_reference`.
            AgentCategory::PolicyAgent,
            |seed| Box::new(ProviderAgent::from_provider(LookaheadSolver::new(seed))),
        ),
        (
            "stateless_reactive",
            "Stateless Reactive",
            "The same one-step rule as the memory arm, but structurally unable to read history. Exists \
             to test whether a benchmark's memory window is load-bearing.",
            AgentCategory::PolicyAgent,
            |_seed| Box::new(ProviderAgent::from_provider(StatelessReactive)),
        ),
        (
            "memory_reactive",
            "Memory Reactive",
            "Identical to the stateless arm except it replays the bounded history window. The A arm \
             of the memory experiment.",
            AgentCategory::PolicyAgent,
            |_seed| Box::new(ProviderAgent::from_provider(MemoryReactive)),
        ),
    ];

    let mut entries: Vec<AgentEntry> = policy_seed
        .into_iter()
        .map(|(id, name, description, category, factory)| {
            entry(
                spec(
                    id,
                    name,
                    description,
                    category,
                    AgentExecution::InProcessDeterministic,
                    Vec::new(),
                ),
                Box::new(move |seed| factory(seed)),
            )
        })
        .collect();

    // The two benchmark-scoped reference strategies. Registered through the reusable wrapper so they
    // carry the published non-privilege claim in their descriptor.
    let wrapped: Vec<WrappedReference> = vec![
        (
            "grid_world_reference",
            "Grid World Reference",
            "The grid-world ceiling, packaged so it declares observation-only knowledge in its \
             descriptor. Reference / ceiling, not a model.",
            "grid-world",
            |seed| Box::new(GridWorldReference::new(seed)) as Box<dyn ReferenceStrategy>,
        ),
        (
            "sandbox_process_reference",
            "Sandbox Process Reference",
            "Asks for the contained process on turn one, then navigates identically. The ceiling for \
             a capability-gated task. Reference / ceiling, not a model.",
            "sandbox-process",
            |seed| Box::new(SandboxProcessReference::new(seed)) as Box<dyn ReferenceStrategy>,
        ),
    ];

    for (id, name, description, benchmark, factory) in wrapped {
        entries.push(entry(
            spec(
                id,
                name,
                description,
                AgentCategory::ReferenceCeiling,
                AgentExecution::InProcessDeterministic,
                vec![benchmark.to_owned()],
            ),
            Box::new(move |seed| Box::new(ReferenceAgent::new(factory(seed))) as Box<dyn Agent>),
        ));
    }

    entries
}

/// The stable identifier for a mock strategy, matching its wire name.
///
/// Derived from the strategy's own reported name rather than a second list, so the two cannot drift.
fn mock_id(config: &MockModelConfig) -> String {
    MockModelProvider::new(*config, 0).descriptor().strategy
}
