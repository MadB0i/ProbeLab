use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    Action, ActionResultKind, Episode, IntegrityEventKind, ModelCallFailureKind, ModelDescriptor,
    RequestedCapability, TokenUsage,
};

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
    /// The safety threshold for consecutive model protocol failures terminated the run.
    ModelProtocolFailure,
}

/// Number of consecutive model-call failures required for protocol termination.
pub const PROTOCOL_FAILURE_THRESHOLD: u32 = 3;

/// Counts whether model responses followed the configured benchmark protocol.
///
/// All count fields use saturating arithmetic. The rate is expressed in basis points and is
/// bounded to 10,000 so it can be serialized without floating-point ambiguity.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ProtocolAdherence {
    /// Total model calls attempted during the run.
    pub total_model_calls: u64,
    /// Calls that produced a valid, policy-reviewable decision.
    pub valid_responses: u64,
    /// Calls rejected as malformed or empty responses.
    pub parse_failures: u64,
    /// Calls containing an action outside the request vocabulary.
    pub invalid_actions: u64,
    /// Calls that exceeded the configured timeout.
    pub timeouts: u64,
    /// Calls rejected by the provider with a provider-specific error.
    pub provider_failures: u64,
    /// Calls rejected by a non-success HTTP response.
    pub http_failures: u64,
    /// Calls that failed before a complete provider response was available.
    pub transport_failures: u64,
    /// Calls rejected by provider or request configuration.
    pub configuration_failures: u64,
    /// Valid responses divided by total calls, expressed in basis points.
    pub valid_response_rate_basis_points: u32,
}

impl ProtocolAdherence {
    /// Records one valid model response and refreshes the deterministic rate.
    pub fn record_valid_response(&mut self) {
        self.total_model_calls = self.total_model_calls.saturating_add(1);
        self.valid_responses = self.valid_responses.saturating_add(1);
        self.refresh_rate();
    }

    /// Records one typed model-call failure and refreshes the deterministic rate.
    pub fn record_failure(&mut self, kind: ModelCallFailureKind) {
        self.total_model_calls = self.total_model_calls.saturating_add(1);
        match kind {
            ModelCallFailureKind::MalformedResponse | ModelCallFailureKind::EmptyResponse => {
                self.parse_failures = self.parse_failures.saturating_add(1);
            }
            ModelCallFailureKind::InvalidAction => {
                self.invalid_actions = self.invalid_actions.saturating_add(1);
            }
            ModelCallFailureKind::Timeout => {
                self.timeouts = self.timeouts.saturating_add(1);
            }
            ModelCallFailureKind::ProviderError => {
                self.provider_failures = self.provider_failures.saturating_add(1);
            }
            ModelCallFailureKind::HttpError => {
                self.http_failures = self.http_failures.saturating_add(1);
            }
            ModelCallFailureKind::TransportDisconnect => {
                self.transport_failures = self.transport_failures.saturating_add(1);
            }
            ModelCallFailureKind::Configuration => {
                self.configuration_failures = self.configuration_failures.saturating_add(1);
            }
        }
        self.refresh_rate();
    }

    fn refresh_rate(&mut self) {
        self.valid_response_rate_basis_points = if self.total_model_calls == 0 {
            0
        } else {
            self.valid_responses
                .saturating_mul(10_000)
                .checked_div(self.total_model_calls)
                .unwrap_or(0)
                .min(10_000) as u32
        };
    }
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
    /// Model response protocol measurements, defaulting for v0.1 result JSON.
    #[serde(default)]
    pub protocol_adherence: ProtocolAdherence,
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

/// Safe non-secret metadata needed to reproduce a benchmark run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ReproducibilityMetadata {
    /// Stable provider identifier reported by the model adapter.
    pub provider: String,
    /// Provider model identifier, without endpoint or credential material.
    pub model: String,
    /// Version of the prompt template supplied to the provider.
    pub prompt_template_version: String,
    /// Stable fingerprint of model-affecting configuration.
    pub configuration_fingerprint: String,
    /// Version of the trusted environment implementation.
    pub environment_version: String,
    /// Token usage aggregated across successful and failed model calls.
    pub token_usage: TokenUsage,
    /// Consecutive protocol failures required to terminate this run.
    pub protocol_failure_threshold: u32,
}

impl Default for ReproducibilityMetadata {
    fn default() -> Self {
        Self {
            provider: String::new(),
            model: String::new(),
            prompt_template_version: String::new(),
            configuration_fingerprint: String::new(),
            environment_version: String::new(),
            token_usage: TokenUsage::default(),
            protocol_failure_threshold: PROTOCOL_FAILURE_THRESHOLD,
        }
    }
}

/// One deterministic turn recorded in a completed run's partial trace.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ActionTraceEntry {
    /// One-based environment decision turn.
    pub turn: u32,
    /// Grid action returned by the model, even when it was not authorized or executable.
    pub selected_action: Option<Action>,
    /// Non-executable capability requested by a valid response.
    pub requested_capability: Option<RequestedCapability>,
    /// Sanitized, bounded model response text, when supplied by the provider.
    pub model_response_text: Option<String>,
    /// Grid action structurally parsed from the model response, if one was identified.
    pub parsed_action: Option<Action>,
    /// Trusted environment classification after executing the parsed action.
    pub action_result_kind: Option<ActionResultKind>,
    /// Environment reward produced by the turn.
    pub reward: i64,
    /// Typed model-call or protocol failure, when the turn had no executable decision.
    pub model_failure_kind: Option<ModelCallFailureKind>,
}

/// Durable execution metadata stored separately from the deterministic result value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct RunMetadata {
    /// Version of the run metadata schema.
    pub schema_version: u32,
    /// Stable benchmark identifier.
    pub benchmark_id: String,
    /// Versioned benchmark identifier.
    pub benchmark_version: String,
    /// Stable provider identifier used by the run.
    pub provider: String,
    /// Provider model identifier used by the run.
    pub model: String,
    /// Prompt template version used by the run.
    pub prompt_template_version: String,
    /// Stable non-secret configuration fingerprint.
    pub configuration_fingerprint: String,
    /// Trusted environment implementation version.
    pub environment_version: String,
    /// Run start time as Unix milliseconds.
    pub started_at_unix_ms: i64,
    /// Run end time as Unix milliseconds.
    pub ended_at_unix_ms: i64,
    /// Ordered durable turn trace, including protocol failures.
    pub action_trace: Vec<ActionTraceEntry>,
    /// Token usage aggregated across all model calls, successful or failed.
    pub token_usage: TokenUsage,
    /// Consecutive protocol failures required to terminate this run.
    pub protocol_failure_threshold: u32,
}

impl Default for RunMetadata {
    fn default() -> Self {
        Self {
            schema_version: 2,
            benchmark_id: String::new(),
            benchmark_version: String::new(),
            provider: String::new(),
            model: String::new(),
            prompt_template_version: String::new(),
            configuration_fingerprint: String::new(),
            environment_version: String::new(),
            started_at_unix_ms: 0,
            ended_at_unix_ms: 0,
            action_trace: Vec::new(),
            token_usage: TokenUsage::default(),
            protocol_failure_threshold: PROTOCOL_FAILURE_THRESHOLD,
        }
    }
}

/// A completed result and its immutable, durable execution metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletedRun {
    /// Deterministic benchmark result.
    pub result: TestResult,
    /// Durable non-deterministic execution metadata.
    pub metadata: RunMetadata,
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
    /// Model protocol measurements collected by the trusted engine.
    #[serde(default)]
    pub protocol_adherence: ProtocolAdherence,
    /// Safe provider metadata collected by the trusted engine.
    #[serde(default)]
    pub reproducibility: ReproducibilityMetadata,
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
    /// Safe non-secret provider and token metadata, defaulting for v0.1 result JSON.
    #[serde(default)]
    pub reproducibility: ReproducibilityMetadata,
}
