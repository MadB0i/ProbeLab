//! Trusted domain contracts and data types shared by ProbeLab components.

#![deny(missing_docs)]

mod action;
mod error;
mod model;
mod observation;
mod outcome;
mod result;
mod security;
mod telemetry;
mod traits;

pub use action::Action;
pub use error::DomainError;
pub use model::{
    ModelCallFailure, ModelCallFailureKind, ModelCallInfo, ModelDecision, ModelDescriptor,
    ModelRequest, ModelResponse, RequestedCapability, TokenUsage,
};
pub use observation::{Coordinate, GridSize, Observation, TileKind, VisibleTile};
pub use outcome::{ActionOutcome, ActionResultKind, Episode, EpisodeStatus, StepOutcome};
pub use result::{
    ActionTraceEntry, BenchmarkSummary, CapabilityResult, CompletedRun, CompletionStatus,
    EvaluationInput, IntegrityResult, ProtocolAdherence, ReproducibilityMetadata, RunMetadata,
    TestResult, PROTOCOL_FAILURE_THRESHOLD,
};
pub use security::{IntegrityEventKind, PolicyDecision, RuntimeAuthorization};
pub use telemetry::{ResourceInfo, TelemetryEvent};
pub use traits::{
    Agent, AsyncModelProvider, Benchmark, Environment, Evaluator, ModelProvider,
    ModelProviderFuture, PolicyGate, RuntimeGuardian, Sandbox, TelemetryRecorder,
};
