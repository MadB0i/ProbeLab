use crate::{
    Action, BenchmarkSummary, Episode, EvaluationInput, IntegrityEventKind, ModelDecision,
    ModelDescriptor, ModelRequest, Observation, PolicyDecision, RuntimeAuthorization, StepOutcome,
    TelemetryEvent, TestResult,
};

/// Produces model decisions from observations without access to trusted infrastructure.
pub trait ModelProvider: Send {
    /// Returns the stable configuration used in final results.
    fn descriptor(&self) -> ModelDescriptor;

    /// Produces one decision using only model-facing context.
    fn predict(&mut self, request: ModelRequest) -> Result<ModelDecision, String>;
}

/// Acts as the model-facing body and delegates decisions to its provider.
pub trait Agent: Send {
    /// Returns the configured model description.
    fn descriptor(&self) -> ModelDescriptor;

    /// Produces one model decision from the current observation.
    fn decide(&mut self, request: ModelRequest) -> Result<ModelDecision, String>;
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
pub trait Benchmark: Send + Sync {
    /// Returns UI-safe benchmark metadata.
    fn summary(&self) -> BenchmarkSummary;

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
