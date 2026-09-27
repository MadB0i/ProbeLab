use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use crate::{
    Action, BenchmarkSummary, CapabilityPolicy, Episode, EvaluationInput, IntegrityEventKind,
    ModelCallFailure, ModelCallFailureKind, ModelDecision, ModelDescriptor, ModelRequest,
    ModelResponse, Observation, PolicyDecision, ProcessSpawnPlan, RuntimeAuthorization,
    StepOutcome, TelemetryEvent, TestResult,
};

/// Produces model decisions from observations without access to trusted infrastructure.
pub trait ModelProvider: Send {
    /// Returns the stable configuration used in final results.
    fn descriptor(&self) -> ModelDescriptor;

    /// Produces one decision using only model-facing context.
    fn predict(&mut self, request: ModelRequest) -> Result<ModelDecision, String>;
}

/// A boxed future returned by an asynchronous model provider.
pub type ModelProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ModelResponse, ModelCallFailure>> + Send + 'a>>;

/// An object-safe asynchronous model provider boundary.
pub trait AsyncModelProvider: Send + Sync {
    /// Returns the stable configuration used in final results.
    fn descriptor(&self) -> ModelDescriptor;

    /// Produces one response using only model-facing context.
    fn predict(&self, request: ModelRequest) -> ModelProviderFuture<'_>;
}

/// Acts as the model-facing body and delegates decisions to its provider.
pub trait Agent: Send {
    /// Returns the configured model description.
    fn descriptor(&self) -> ModelDescriptor;

    /// Produces one model decision from the current observation.
    fn decide(&mut self, request: ModelRequest) -> Result<ModelDecision, String>;

    /// Produces one model decision and any safe provider metadata available.
    fn decide_with_metadata(
        &mut self,
        request: ModelRequest,
    ) -> Result<ModelResponse, ModelCallFailure> {
        self.decide(request)
            .map(|decision| ModelResponse {
                decision,
                call: None,
            })
            .map_err(|_| {
                ModelCallFailure::new(ModelCallFailureKind::ProviderError, "agent decision failed")
            })
    }
}

/// A controlled test world exposing observations and transitions only.
pub trait Environment: Send {
    /// Returns the current legitimate observation.
    fn observation(&self) -> Observation;

    /// Returns the model-facing action vocabulary.
    fn available_actions(&self) -> Vec<Action>;

    /// Applies one authorized action to trusted environment state.
    fn step(&mut self, action: Action) -> Result<StepOutcome, String>;

    /// Returns a trusted snapshot for evaluation and telemetry.
    fn episode(&self) -> Episode;
}

/// A versioned, executable benchmark definition.
/// How many seeds a benchmark that has not declared its own pool gets.
///
/// Small on purpose. A derived pool is not a methodological claim, so the default does not pretend to
/// be one; a benchmark wanting a real measurement declares disjoint pools and overrides
/// [`Benchmark::eval_seeds`].
pub const DEFAULT_EVAL_SEED_COUNT: usize = 30;

/// A benchmark the engine can run.
///
/// Implementations describe an environment: what it grants, what it protects, and which seeds are
/// reserved for measurement. The defaults are deny-all and derived, so a new benchmark is safe before
/// it is deliberate.
pub trait Benchmark: Send + Sync {
    /// Returns UI-safe benchmark metadata.
    fn summary(&self) -> BenchmarkSummary;

    /// The seeds reserved for capability measurement.
    ///
    /// A measurement draws only from here, and this is the method that makes "disjoint from the seeds
    /// the benchmark was developed against" a property of the benchmark rather than a promise in a
    /// document. The Runner uses it instead of generating a linear sequence from a root seed, which
    /// could silently land on a seed the author had already tuned against.
    ///
    /// The default derives a small pool from the summary seed, so a benchmark that has not declared
    /// pools still runs. It is derived, not disjoint-by-construction, which is why a benchmark that
    /// cares overrides this — and why a report states the seed source.
    fn eval_seeds(&self) -> Vec<u64> {
        let base = self.summary().seed;
        (0..DEFAULT_EVAL_SEED_COUNT)
            .map(|offset| base.wrapping_add(u64::try_from(offset).unwrap_or(0)))
            .collect()
    }

    /// Returns the capabilities this benchmark grants for one run.
    ///
    /// The default grants nothing, so an existing benchmark stays source-compatible and a new
    /// benchmark is deny-by-default until it declares a grant explicitly.
    fn capability_policy(&self) -> CapabilityPolicy {
        CapabilityPolicy::deny_all()
    }

    /// Returns the process this benchmark is willing to have spawned on a model's behalf, if any.
    ///
    /// A model can ask for the `process.spawn` *permission*; it can never name a program, an
    /// argument, or an environment variable. The plan below is compiled-in trusted configuration, and
    /// even then the broker still has to find the program on its explicit allowlist before anything
    /// runs. `None` means the benchmark exposes no process capability at all, and a spawn request is
    /// then refused.
    fn process_spawn_plan(&self) -> Option<ProcessSpawnPlan> {
        None
    }

    /// Returns the trusted paths a contained child must not be able to read.
    ///
    /// These are the evaluation-critical locations: the benchmark definitions, the evaluator, and
    /// anything else whose contents would tell a child the answer. They are declared here, by trusted
    /// code, and are never influenced by a model response. They are only meaningful once a benchmark
    /// also grants a process capability.
    fn protected_paths(&self) -> &[PathBuf] {
        &[]
    }

    /// Creates an isolated, capability-restricted sandbox from the explicit seed.
    fn create_sandbox(&self, seed: u64) -> Result<Box<dyn Sandbox>, String>;
}

/// A fail-closed policy boundary between a model and runtime execution.
pub trait PolicyGate: Send + Sync {
    /// Converts a model decision into a non-executable or executable policy decision.
    fn review(&self, decision: ModelDecision) -> PolicyDecision;
}

/// The only route through which an authorized action can reach an environment.
pub trait Sandbox: Send {
    /// Returns the current legitimate model-facing observation.
    fn observation(&self) -> Observation;

    /// Returns the model-facing action vocabulary.
    fn available_actions(&self) -> Vec<Action>;

    /// Executes one runtime authorization and returns trusted environment facts.
    fn execute(&mut self, authorization: RuntimeAuthorization) -> Result<StepOutcome, String>;

    /// Consumes one turn after policy denial without executing the requested capability.
    fn consume_denied_turn(&mut self, event: IntegrityEventKind) -> Result<StepOutcome, String>;

    /// Consumes one turn after an authorized capability actually ran.
    ///
    /// A child process is not a grid action, so the world does not move and no reward is produced,
    /// but the turn is still spent: the model used its budget to ask for something. Unlike a denied
    /// turn this records no integrity event, because the broker did its job.
    fn consume_capability_turn(&mut self) -> Result<StepOutcome, String>;

    /// Consumes one turn for a model protocol failure without changing grid state.
    fn consume_protocol_failure_turn(&mut self, terminate: bool) -> Result<StepOutcome, String>;

    /// Returns a trusted snapshot for evaluation and telemetry.
    fn episode(&self) -> Episode;
}

/// Independently verifies policy decisions immediately before sandbox execution.
pub trait RuntimeGuardian: Send + Sync {
    /// Authorizes only plain grid actions and rejects all capability requests.
    fn authorize(&self, decision: &PolicyDecision) -> RuntimeAuthorization;
}

/// Converts trusted execution facts into capability and integrity results.
pub trait Evaluator: Send + Sync {
    /// Evaluates one completed episode without accepting model-reported results.
    fn evaluate(&self, input: EvaluationInput) -> Result<TestResult, String>;
}

/// An append-only sink with no mutation or deletion operation.
pub trait TelemetryRecorder: Send + Sync {
    /// Durably appends exactly one observed event.
    fn record(&self, event: &TelemetryEvent) -> Result<(), String>;
}
