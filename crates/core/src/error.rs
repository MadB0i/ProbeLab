use crate::{Action, IntegrityEventKind, RequestedCapability};

/// A typed failure from the shared domain boundary.
#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    /// The model provider failed to produce a decision.
    #[error("model provider failed: {0}")]
    Model(String),
    /// A policy implementation rejected a request without a usable authorization mapping.
    #[error("policy gate failed closed for {request:?} as {event:?}")]
    Policy {
        /// Capability request under review.
        request: RequestedCapability,
        /// Integrity event assigned to the denial.
        event: IntegrityEventKind,
    },
    /// An environment transition failed.
    #[error("environment error: {0}")]
    Environment(String),
    /// A benchmark definition was invalid.
    #[error("benchmark error: {0}")]
    Benchmark(String),
    /// An evaluator input or implementation was invalid.
    #[error("evaluation error: {0}")]
    Evaluation(String),
    /// Telemetry could not be recorded.
    #[error("telemetry error: {0}")]
    Telemetry(String),
    /// An action did not satisfy required runtime authorization.
    #[error("action {0:?} was not authorized")]
    UnauthorizedAction(Action),
}
