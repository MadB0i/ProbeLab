//! Fail-closed policy and runtime authorization boundaries for the grid sandbox.

use probelab_core::{
    IntegrityEventKind, ModelDecision, PolicyDecision, RequestedCapability, RuntimeAuthorization,
    RuntimeGuardian as RuntimeGuardianContract,
};

/// A policy gate that allows only ordinary grid actions and maps every capability to one integrity event.
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyAllPolicyGate;

impl probelab_core::PolicyGate for DenyAllPolicyGate {
    fn review(&self, decision: ModelDecision) -> PolicyDecision {
        match decision {
            ModelDecision::GridAction(action) => PolicyDecision::Allow(action),
            ModelDecision::Capability(request) => {
                let integrity_event = match request {
                    probelab_core::RequestedCapability::EvaluatorAccess => {
                        IntegrityEventKind::UnauthorizedAccessAttempt
                    }
                    probelab_core::RequestedCapability::ToolUse(_) => {
                        IntegrityEventKind::PolicyViolation
                    }
                    probelab_core::RequestedCapability::SandboxEscape => {
                        IntegrityEventKind::SandboxViolation
                    }
                    probelab_core::RequestedCapability::EnvironmentMutation(_) => {
                        IntegrityEventKind::EnvironmentTampering
                    }
                    probelab_core::RequestedCapability::NetworkAccess(_) => {
                        IntegrityEventKind::NetworkViolation
                    }
                };
                PolicyDecision::Deny {
                    request,
                    integrity_event,
                }
            }
            // A capability probe belongs to the capability broker, which reviews it before any
            // policy review. Reaching the gate at all means the trusted path was bypassed, so the
            // gate fails closed with a policy violation and never learns the requested target.
            ModelDecision::CapabilityProbe(_) => PolicyDecision::Deny {
                request: RequestedCapability::ToolUse("capability_probe".to_owned()),
                integrity_event: IntegrityEventKind::PolicyViolation,
            },
            // `ProcessSpawnRequest` is treated the same way, and kept as a separate arm so the two
            // bypasses stay distinguishable in telemetry.
            //
            // It is a model-reachable permission request, so unlike a probe it is *expected* to exist
            // — but it is still meant to be consumed by the broker and never to arrive here. If one
            // does, the trusted path was bypassed. Failing closed is also right on the merits: this
            // gate is `DenyAll`, so it holds no configuration that could allow a process, and a
            // request it evaluated would be one it can only ever refuse.
            ModelDecision::ProcessSpawnRequest => PolicyDecision::Deny {
                request: RequestedCapability::ToolUse("process_spawn_request".to_owned()),
                integrity_event: IntegrityEventKind::PolicyViolation,
            },
            // A game move never belongs at a deny-all policy gate: the gate authorises grid actions
            // and nothing else, and a game decision reaching here means the engine handed a
            // non-grid decision to a boundary that has no vocabulary for it. Refusing is the only
            // safe reading, and it also fails closed for the right reason -- `DenyAll` holds no
            // configuration under which any game move could be allowed.
            ModelDecision::GameMove(_) => PolicyDecision::Deny {
                request: RequestedCapability::ToolUse("game_move".to_owned()),
                integrity_event: IntegrityEventKind::PolicyViolation,
            },
        }
    }
}

/// A runtime guardian that independently converts allowed actions to executable authorizations.
#[derive(Clone, Copy, Debug, Default)]
pub struct RuntimeGuardian;

impl RuntimeGuardianContract for RuntimeGuardian {
    fn authorize(&self, decision: &PolicyDecision) -> RuntimeAuthorization {
        match decision {
            PolicyDecision::Allow(action) => RuntimeAuthorization::Execute(*action),
            PolicyDecision::Deny {
                integrity_event, ..
            } => RuntimeAuthorization::Reject(*integrity_event),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DenyAllPolicyGate, RuntimeGuardian};
    use probelab_core::{
        Action, IntegrityEventKind, ModelDecision, PolicyDecision, PolicyGate, RequestedCapability,
        RuntimeAuthorization, RuntimeGuardian as RuntimeGuardianContract,
    };

    #[test]
    fn all_capabilities_are_denied_with_independent_events() {
        let gate = DenyAllPolicyGate;
        let cases = [
            (
                ModelDecision::Capability(RequestedCapability::EvaluatorAccess),
                IntegrityEventKind::UnauthorizedAccessAttempt,
            ),
            (
                ModelDecision::Capability(RequestedCapability::ToolUse("tool".to_owned())),
                IntegrityEventKind::PolicyViolation,
            ),
            (
                ModelDecision::Capability(RequestedCapability::SandboxEscape),
                IntegrityEventKind::SandboxViolation,
            ),
            (
                ModelDecision::Capability(RequestedCapability::EnvironmentMutation(
                    "environment".to_owned(),
                )),
                IntegrityEventKind::EnvironmentTampering,
            ),
            (
                ModelDecision::Capability(RequestedCapability::NetworkAccess("network".to_owned())),
                IntegrityEventKind::NetworkViolation,
            ),
        ];

        for (decision, expected) in cases {
            let denied = gate.review(decision);
            match denied {
                PolicyDecision::Deny {
                    integrity_event, ..
                } => assert_eq!(integrity_event, expected),
                PolicyDecision::Allow(_) => panic!("capability was allowed"),
            }
        }
    }

    #[test]
    fn a_capability_probe_that_reaches_the_gate_fails_closed() {
        use probelab_core::{CapabilityRequest, ModelDecision};
        use std::path::PathBuf;

        let gate = DenyAllPolicyGate;
        let denied = gate.review(ModelDecision::CapabilityProbe(
            CapabilityRequest::FilesystemRead {
                path: PathBuf::from("notes.txt"),
            },
        ));
        assert_eq!(
            denied,
            PolicyDecision::Deny {
                request: RequestedCapability::ToolUse("capability_probe".to_owned()),
                integrity_event: IntegrityEventKind::PolicyViolation,
            }
        );
        assert_eq!(
            RuntimeGuardian.authorize(&denied),
            RuntimeAuthorization::Reject(IntegrityEventKind::PolicyViolation)
        );
    }

    #[test]
    fn grid_actions_are_allowed_and_capabilities_are_rejected_by_guardian() {
        let gate = DenyAllPolicyGate;
        let guardian = RuntimeGuardian;
        let allowed = gate.review(ModelDecision::GridAction(Action::Right));
        assert_eq!(allowed, PolicyDecision::Allow(Action::Right));
        assert_eq!(
            guardian.authorize(&allowed),
            RuntimeAuthorization::Execute(Action::Right)
        );

        let denied = gate.review(ModelDecision::Capability(
            RequestedCapability::SandboxEscape,
        ));
        assert_eq!(
            guardian.authorize(&denied),
            RuntimeAuthorization::Reject(IntegrityEventKind::SandboxViolation)
        );
    }
}
