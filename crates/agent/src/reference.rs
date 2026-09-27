//! The reference agent: a trusted, non-privileged ceiling for any benchmark.
//!
//! # What this is for
//!
//! Every benchmark needs a number that says "a good policy gets about this much", and that number
//! has to come from somewhere other than the thing being measured. This is that somewhere. It is a
//! *component*, not a product: it has no objective of its own, no memory across benchmarks, and no
//! way to be configured into looking better than it is.
//!
//! # Why a trait rather than another solver
//!
//! Before this, the grid world had `LookaheadSolver` and the sandbox-process benchmark had a
//! hand-written wrapper that requested the process and then delegated. Both were reference policies,
//! and neither could be used on a third benchmark without copying it. Adding a benchmark meant adding
//! a near-duplicate of someone else's agent.
//!
//! `ReferenceStrategy` is that seam. A strategy answers exactly one question — given this
//! `ModelRequest`, what do you do — and knows nothing about harnesses, telemetry, evaluators, or
//! containment. A new benchmark supplies a strategy; the wrapper, the descriptor, and the
//! non-privilege proof come for free.
//!
//! # The non-privilege guarantee
//!
//! A reference policy that could see the layout, the goal, or the answer would not be a ceiling, it
//! would be an oracle, and every number compared against it would be meaningless. Three things
//! enforce that here, and they are deliberately structural rather than documentary:
//!
//! 1. A strategy receives `&ModelRequest` and `&mut self`. Nothing else is in scope, so there is no
//!    layout to read even by accident.
//! 2. The seed is *not* passed to `decide`. It is recorded in the descriptor and in the replay
//!    fingerprint, and it reaches only `reset`, so a strategy cannot branch on it mid-episode.
//! 3. `ReferenceAgent` publishes what the strategy is allowed to read into the run's descriptor, so a
//!    result bundle states the guarantee rather than the reader having to trust this comment.
//!
//! The behavioural half of the proof — that two instances built with different seeds, replayed
//! against the same request stream over provably different worlds, produce identical decisions — is a
//! test rather than a type, because no type can prevent a strategy from reading a global. It is
//! written once, generically, over every registered strategy.

use std::collections::BTreeMap;

use probelab_core::{Agent, ModelDecision, ModelDescriptor, ModelRequest};

/// Exactly what a reference strategy is permitted to consult when choosing an action.
///
/// Published into the descriptor so a result bundle carries its own non-privilege claim. Every field
/// is a `false` in practice; they exist so that saying "this policy sees only the observation" is a
/// structured, checkable statement rather than prose.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReferenceKnowledge {
    /// Whether the strategy may read the environment's hidden layout.
    pub reads_layout: bool,
    /// Whether the strategy may read the goal position directly rather than inferring it.
    pub reads_goal: bool,
    /// Whether the strategy may read the answer or an expected result.
    pub reads_answer: bool,
    /// Whether the strategy may read the episode seed.
    ///
    /// A `true` here would make the policy unreproducible from a result bundle, and would let a
    /// strategy tune itself per seed while still looking like one policy.
    pub reads_seed: bool,
}

impl ReferenceKnowledge {
    /// The only knowledge a legitimate reference policy has.
    ///
    /// The current observation, the bounded history window, the action vocabulary, and whatever it
    /// has inferred itself into `&mut self` across turns.
    pub const OBSERVATION_ONLY: Self = Self {
        reads_layout: false,
        reads_goal: false,
        reads_answer: false,
        reads_seed: false,
    };

    /// Returns true when nothing outside the model's own view is consulted.
    pub fn is_observation_only(&self) -> bool {
        *self == Self::OBSERVATION_ONLY
    }
}

/// A reference policy for one benchmark, or one family of benchmarks.
///
/// Implementations are ordinary Rust and cannot be *forced* to be honest — nothing in the type system
/// stops a strategy from reading a global. What the type system does guarantee is that it is given
/// no privileged input, and the shared test proves the shipped strategies do not reach for one.
pub trait ReferenceStrategy: Send {
    /// A stable name for this strategy, recorded in the descriptor.
    ///
    /// Must be stable across runs: it is half of what makes a result bundle interpretable, and a
    /// name that changes between releases silently breaks comparability.
    fn name(&self) -> &'static str;

    /// What this strategy is permitted to consult.
    fn knowledge(&self) -> ReferenceKnowledge;

    /// Chooses one decision from the request and whatever this strategy remembers.
    fn decide(&mut self, request: &ModelRequest) -> Result<ModelDecision, String>;

    /// Discards everything inferred during the previous episode.
    ///
    /// Called by the harness between episodes. The default is correct for a strategy that only ever
    /// builds state as it goes; a strategy that caches across episodes must override this, because
    /// leaking an episode's inferences into the next one is exactly the kind of quiet advantage a
    /// reference policy must not have.
    fn reset(&mut self) {}
}

/// Puts a reference policy into an agent seat.
///
/// # Why this exists
///
/// The arena's `play_match` puts a *model* in the tested seat and a *reference policy* in the
/// opponent seat. That is the right shape for measuring a model — you want a fixed, unbeatable
/// opponent — but it leaves no way to seat two policies against each other, and a match between two
/// ceilings is the cheapest way to exercise a game implementation end to end.
///
/// This adapter was written as a local helper inside the arena test file, where it served one test.
/// It lives here now because the arena needs it in production: without a policy that can sit in the
/// tested seat, the arena can only ever be driven by a model, and the command center would have no
/// way to demonstrate a real match at all.
///
/// # What it does not do
///
/// It does not grant any capability. It reads a `ModelRequest` and returns a `ModelDecision`, which is
/// the whole contract every agent in this lab satisfies. The environment still decides whether the move
/// it returns is legal.
pub struct ReferenceAsAgent {
    strategy: Box<dyn ReferenceStrategy>,
}

impl std::fmt::Debug for ReferenceAsAgent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `dyn ReferenceStrategy` is not `Debug`, and requiring every policy to derive it would be a
        // tax on all of them for the sake of one log line. The name is the part that identifies it.
        formatter
            .debug_struct("ReferenceAsAgent")
            .field("strategy", &self.strategy.name())
            .finish()
    }
}

impl ReferenceAsAgent {
    /// Wraps a strategy so it can be used wherever an agent is expected.
    pub fn new(strategy: Box<dyn ReferenceStrategy>) -> Self {
        Self { strategy }
    }

    /// The wrapped strategy's stable name.
    pub fn strategy_name(&self) -> &'static str {
        self.strategy.name()
    }
}

impl Agent for ReferenceAsAgent {
    fn descriptor(&self) -> ModelDescriptor {
        ModelDescriptor {
            adapter: "reference-as-agent-v1".to_owned(),
            strategy: self.strategy.name().to_owned(),
            seed: 0,
            parameters: reference_parameters(self.strategy.knowledge()),
        }
    }

    fn decide(&mut self, request: ModelRequest) -> Result<ModelDecision, String> {
        self.strategy.decide(&request)
    }
}

/// Lets a strategy be held behind a trait object, which is what a registry needs.
///
/// Without this, a benchmark selected from a table would have to be generic all the way up to the
/// harness, and the reusable part of this phase would stop being reusable the moment two benchmarks
/// had to be measured in the same process.
impl ReferenceStrategy for Box<dyn ReferenceStrategy> {
    fn name(&self) -> &'static str {
        (**self).name()
    }

    fn knowledge(&self) -> ReferenceKnowledge {
        (**self).knowledge()
    }

    fn decide(&mut self, request: &ModelRequest) -> Result<ModelDecision, String> {
        (**self).decide(request)
    }

    fn reset(&mut self) {
        (**self).reset();
    }
}

/// A failure constructing or running a reference policy.
#[derive(Debug, thiserror::Error)]
pub enum ReferenceError {
    /// No reference strategy is registered for the requested benchmark.
    #[error("no reference strategy is registered for benchmark {benchmark_id:?}")]
    UnknownBenchmark {
        /// The benchmark that was asked for.
        benchmark_id: String,
    },
    /// A strategy returned a decision the engine would have to reject.
    #[error("reference strategy {strategy:?} produced an unusable decision: {reason}")]
    UnusableDecision {
        /// The strategy that misbehaved.
        strategy: &'static str,
        /// What was wrong with what it returned.
        reason: String,
    },
}

/// Wraps a [`ReferenceStrategy`] as an [`Agent`], and publishes its non-privilege claim.
///
/// The wrapper is what makes the strategy reusable: it owns request validation, the descriptor
/// contract, and the per-episode reset, so a strategy author implements four methods and nothing else.
pub struct ReferenceAgent<S: ReferenceStrategy> {
    strategy: S,
    episode_id: Option<String>,
}

impl<S: ReferenceStrategy> ReferenceAgent<S> {
    /// Wraps a strategy for a fresh episode sequence.
    pub fn new(strategy: S) -> Self {
        Self {
            strategy,
            episode_id: None,
        }
    }

    /// Returns the wrapped strategy.
    pub fn strategy(&self) -> &S {
        &self.strategy
    }

    /// Returns the strategy's stable name.
    pub fn strategy_name(&self) -> &'static str {
        self.strategy.name()
    }

    /// Returns what the strategy is permitted to consult.
    pub fn knowledge(&self) -> ReferenceKnowledge {
        self.strategy.knowledge()
    }
}

/// Builds the descriptor parameters that publish what a reference policy is allowed to consult.
///
/// Shared by [`ReferenceAgent`] and [`ReferenceAsAgent`] so the non-privilege claim is emitted
/// identically whichever seat the policy occupies. Two copies of this would be two chances for one to
/// drift, and the whole point of the claim is that a reader can rely on it.
fn reference_parameters(knowledge: ReferenceKnowledge) -> BTreeMap<String, serde_json::Value> {
    let mut parameters: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    parameters.insert("reference_policy".to_owned(), serde_json::Value::Bool(true));
    parameters.insert(
        "strategy".to_owned(),
        serde_json::Value::String(String::new()),
    );
    parameters.insert(
        "reads_layout".to_owned(),
        serde_json::Value::Bool(knowledge.reads_layout),
    );
    parameters.insert(
        "reads_goal".to_owned(),
        serde_json::Value::Bool(knowledge.reads_goal),
    );
    parameters.insert(
        "reads_answer".to_owned(),
        serde_json::Value::Bool(knowledge.reads_answer),
    );
    parameters.insert(
        "reads_seed".to_owned(),
        serde_json::Value::Bool(knowledge.reads_seed),
    );
    parameters
}

impl<S: ReferenceStrategy> Agent for ReferenceAgent<S> {
    fn descriptor(&self) -> ModelDescriptor {
        let knowledge = self.strategy.knowledge();
        let mut parameters = reference_parameters(knowledge);
        parameters.insert(
            "strategy".to_owned(),
            serde_json::Value::String(self.strategy.name().to_owned()),
        );
        ModelDescriptor {
            adapter: "probelab-reference-v1".to_owned(),
            strategy: self.strategy.name().to_owned(),
            // Recorded, never consulted: see `ReferenceKnowledge::reads_seed`.
            seed: 0,
            parameters,
        }
    }

    fn decide(&mut self, request: ModelRequest) -> Result<ModelDecision, String> {
        // Two independent signals that a new episode has begun, and both are checked because either
        // alone is fragile.
        //
        // `episode_id` changing is the obvious one, but a caller that reuses an id — across two
        // measurements in a process, say — would silently carry the previous episode's inferences
        // into the next, and a reference policy that starts a run already knowing tiles it never
        // observed is not a ceiling. Turn one is unambiguous and cannot be faked, so it is checked
        // too. Resetting on turn one costs nothing: a strategy has nothing to carry into turn one.
        let new_episode =
            request.turn == 1 || self.episode_id.as_deref() != Some(request.episode_id.as_str());
        if new_episode {
            self.strategy.reset();
            self.episode_id = Some(request.episode_id.clone());
        }
        request.validate()?;
        self.strategy.decide(&request)
    }
}
