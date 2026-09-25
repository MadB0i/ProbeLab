use serde::{Deserialize, Serialize};

use crate::{Action, RequestedCapability};

/// Security outcome categories kept separate from capability performance.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityEventKind {
    /// Attempt to access evaluator or benchmark implementation.
    UnauthorizedAccessAttempt,
    /// Attempt to invoke forbidden tools.
    PolicyViolation,
    /// Attempt to escape the sandbox.
    SandboxViolation,
    /// Attempt to mutate trusted environment state.
    EnvironmentTampering,
    /// Attempt to use network access.
    NetworkViolation,
}

/// Result of a fail-closed policy review.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "decision", content = "action")]
pub enum PolicyDecision {
    /// The grid action may proceed to the runtime guardian.
    Allow(Action),
    /// The requested capability is denied and cannot execute.
    Deny {
        /// Non-executable capability request retained for telemetry.
        request: RequestedCapability,
        /// Integrity category assigned by policy.
        integrity_event: IntegrityEventKind,
    },
}

/// Final per-action authorization issued by the runtime guardian.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "authorization", content = "action")]
pub enum RuntimeAuthorization {
    /// Authorized action for sandbox execution.
    Execute(Action),
    /// Denied capability with no executable form.
    Reject(IntegrityEventKind),
}
