//! Reference policies: the trusted ceilings each benchmark is measured against.
//!
//! Every strategy here implements [`ReferenceStrategy`], which is the whole point of the phase. A
//! benchmark supplies one of these and inherits the wrapper, the descriptor contract, request
//! validation, and the shared non-privilege proof — none of which it has to reimplement.
//!
//! Two exist today, and they differ by exactly one decision, which is what makes their comparison a
//! measurement rather than an anecdote:
//!
//! * [`GridWorldReference`] — navigate the grid using only the observation. The ceiling for the
//!   grid-world benchmark.
//! * [`SandboxProcessReference`] — ask for the benchmark's contained process on the first turn, then
//!   navigate identically. The ceiling for `sandbox-process`.
//!
//! Before this existed, the second of those was a wrapper written inline inside a test, which meant
//! the reference policy for a shipped benchmark was not a shipped artifact at all.

use probelab_agent::{ReferenceKnowledge, ReferenceStrategy};
use probelab_core::{ModelDecision, ModelRequest};

use crate::LookaheadSolver;

/// The grid-world reference policy.
///
/// A thin adapter over [`LookaheadSolver`], which already had the required property: it is a pure
/// function of the request stream plus its own `&mut self`, and never reads the layout, the goal, or
/// the seed. Implementing the trait on top of it rather than rewriting it means the published grid-world
/// numbers still describe the same policy they always did.
#[derive(Debug)]
pub struct GridWorldReference {
    solver: LookaheadSolver,
}

impl GridWorldReference {
    /// Creates the reference policy for an episode sequence.
    ///
    /// `seed` is recorded and never consulted; see [`ReferenceKnowledge::reads_seed`].
    pub fn new(seed: u64) -> Self {
        Self {
            solver: LookaheadSolver::new(seed),
        }
    }
}

impl ReferenceStrategy for GridWorldReference {
    fn name(&self) -> &'static str {
        "grid_world_reference"
    }

    fn knowledge(&self) -> ReferenceKnowledge {
        ReferenceKnowledge::OBSERVATION_ONLY
    }

    fn decide(&mut self, request: &ModelRequest) -> Result<ModelDecision, String> {
        self.solver.decide(request).map(ModelDecision::GridAction)
    }

    fn reset(&mut self) {
        self.solver.clear_inferred();
    }
}

/// The `sandbox-process` reference policy.
///
/// Asks for the benchmark's contained process on turn one, then delegates to the grid-world reference
/// for every turn after that.
///
/// The single request is the whole capability contract: the model asks for a permission and names
/// nothing, and the benchmark's own trusted plan decides what actually runs. So this policy is not
/// "a solver that also spawns a process" — it is the grid-world solver paying one turn of its budget
/// for a capability, which is precisely the tradeoff the benchmark exists to measure.
#[derive(Debug)]
pub struct SandboxProcessReference {
    inner: GridWorldReference,
    asked: bool,
}

impl SandboxProcessReference {
    /// Creates the reference policy for an episode sequence.
    pub fn new(seed: u64) -> Self {
        Self {
            inner: GridWorldReference::new(seed),
            asked: false,
        }
    }
}

impl ReferenceStrategy for SandboxProcessReference {
    fn name(&self) -> &'static str {
        "sandbox_process_reference"
    }

    fn knowledge(&self) -> ReferenceKnowledge {
        ReferenceKnowledge::OBSERVATION_ONLY
    }

    fn decide(&mut self, request: &ModelRequest) -> Result<ModelDecision, String> {
        if !self.asked && request.turn == 1 {
            self.asked = true;
            return Ok(ModelDecision::ProcessSpawnRequest);
        }
        self.inner.decide(request)
    }

    fn reset(&mut self) {
        self.asked = false;
        self.inner.reset();
    }
}
