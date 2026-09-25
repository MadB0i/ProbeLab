use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{Episode, IntegrityEventKind, ModelDescriptor};

/// A discoverable benchmark description safe for the UI.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BenchmarkSummary {
    /// Stable benchmark identifier.
    pub benchmark_id: String,
    /// Versioned definition identifier.
    pub benchmark_version: String,
    /// Human-readable title.
    pub title: String,
    /// Short benchmark purpose.
    pub description: String,
    /// Single honest capability dimension.
    pub capability_dimension: String,
    /// Default deterministic environment seed.
    pub seed: u64,
    /// Configured episode limit.
    pub max_turns: u32,
    /// Additional frontend-safe metadata.
    pub metadata: BTreeMap<String, String>,
}

/// Terminal status reported by the trusted environment.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionStatus {
    /// Reward reached before terminal failure or timeout.
    Success,
    /// Danger reached.
    Failure,
    /// Turn limit reached without reward or danger.
    TurnLimit,
}

/// Capability measurement for exactly one implemented benchmark dimension.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilityResult {
    /// Stable dimension name.
    pub dimension: String,
    /// Whether the task reached its reward.
    pub task_success: bool,
    /// Environment-generated total reward.
    pub total_reward: i64,
    /// Environment-generated turns consumed.
    pub turns_used: u32,
    /// Environment-generated invalid action count.
    pub invalid_action_count: u32,
    /// Trusted environment completion status.
    pub completion_status: CompletionStatus,
}

/// Security measurements kept separate from opaque capability scoring.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct IntegrityResult {
    /// Unauthorized evaluator or infrastructure access attempts.
    pub unauthorized_access_attempts: u32,
    /// Forbidden tool-use attempts.
    pub policy_violations: u32,
    /// Sandbox escape attempts.
    pub sandbox_violations: u32,
    /// Direct environment mutation attempts.
    pub environment_tampering: u32,
    /// Network access attempts.
    pub network_violations: u32,
}

impl IntegrityResult {
    /// Counts one observed event in its independent category.
    pub fn record(&mut self, event: IntegrityEventKind) {
        match event {
            IntegrityEventKind::UnauthorizedAccessAttempt => {
                self.unauthorized_access_attempts += 1;
            }
            IntegrityEventKind::PolicyViolation => self.policy_violations += 1,
            IntegrityEventKind::SandboxViolation => self.sandbox_violations += 1,
            IntegrityEventKind::EnvironmentTampering => self.environment_tampering += 1,
            IntegrityEventKind::NetworkViolation => self.network_violations += 1,
        }
    }

    /// Returns true only when no integrity event was observed.
    pub fn is_compliant(&self) -> bool {
        self.total_events() == 0
    }

    /// Returns the sum across all independent violation categories.
    pub fn total_events(&self) -> u32 {
        self.unauthorized_access_attempts
            + self.policy_violations
            + self.sandbox_violations
            + self.environment_tampering
            + self.network_violations
    }
}

/// Trusted facts supplied to the independent evaluator.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EvaluationInput {
    /// Benchmark identifier.
    pub benchmark_id: String,
    /// Versioned benchmark definition.
    pub benchmark_version: String,
    /// Single capability dimension declared by the benchmark.
    pub capability_dimension: String,
    /// Model configuration selected before execution.
    pub model: ModelDescriptor,
    /// Environment and model seed.
    pub seed: u64,
    /// Terminal environment facts.
    pub episode: Episode,
    /// Integrity events independently observed during execution.
    pub integrity_events: Vec<IntegrityEventKind>,
    /// Number of durably recorded events.
    pub telemetry_event_count: u32,
    /// Digest of canonical deterministic event content.
    pub replay_digest: String,
}

/// Deterministic final benchmark result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TestResult {
    /// Stable result schema version.
    pub schema_version: u32,
    /// Benchmark identifier.
    pub benchmark_id: String,
    /// Versioned benchmark definition.
    pub benchmark_version: String,
    /// Model configuration used for the run.
    pub model: ModelDescriptor,
    /// Seed governing environment map generation; model RNG seed is in the descriptor.
    pub seed: u64,
    /// Capability measurements.
    pub capability: CapabilityResult,
    /// Integrity measurements.
    pub integrity: IntegrityResult,
    /// Number of committed telemetry events.
    pub telemetry_event_count: u32,
    /// SHA-256 digest of canonical deterministic event content.
    pub replay_digest: String,
}
