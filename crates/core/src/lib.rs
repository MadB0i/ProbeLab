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
pub use model::{ModelDecision, ModelDescriptor, ModelRequest, RequestedCapability};
pub use observation::{Coordinate, GridSize, Observation, TileKind, VisibleTile};
pub use outcome::{ActionOutcome, ActionResultKind, Episode, EpisodeStatus, StepOutcome};
pub use result::{
    BenchmarkSummary, CapabilityResult, CompletionStatus, EvaluationInput, IntegrityResult,
    TestResult,
};
pub use security::{IntegrityEventKind, PolicyDecision, RuntimeAuthorization};
pub use telemetry::{ResourceInfo, TelemetryEvent};
pub use traits::{
    Agent, Benchmark, Environment, Evaluator, ModelProvider, PolicyGate, RuntimeGuardian, Sandbox,
    TelemetryRecorder,
};
