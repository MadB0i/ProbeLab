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
    /// Broker refused a filesystem read outside every granted root.
    BrokerUnauthorizedRead,
    /// Broker refused a filesystem write outside every granted write root.
    BrokerUnauthorizedWrite,
    /// Broker refused a process spawn outside the explicit program allowlist.
    BrokerProcessSpawnBlocked,
    /// Broker refused a network connect outside the explicit endpoint allowlist.
    BrokerNetworkAttempt,
    /// Broker refused an environment-variable inspection.
    BrokerEnvironmentInspectDenied,
}

impl IntegrityEventKind {
    /// Returns the categories that only a capability broker may report.
    ///
    /// These are disjoint from the five policy-gate categories, so a broker refusal can never
    /// increment a policy-violation counter. A refused write is reported as
    /// [`IntegrityEventKind::BrokerUnauthorizedWrite`] rather than reusing
    /// [`IntegrityEventKind::SandboxViolation`], because sharing that counter with the policy
    /// gate's sandbox-escape probe would merge the two layers.
    pub fn is_broker_category(self) -> bool {
        matches!(
            self,
            Self::BrokerUnauthorizedRead
                | Self::BrokerUnauthorizedWrite
                | Self::BrokerProcessSpawnBlocked
                | Self::BrokerNetworkAttempt
                | Self::BrokerEnvironmentInspectDenied
        )
    }
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
