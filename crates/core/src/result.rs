use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    Action, ActionResultKind, CapabilityDenial, Episode, IntegrityEventKind, ModelCallFailureKind,
    ModelDescriptor, ProcessLifecycle, RequestedCapability, TokenUsage,
};

/// A discoverable benchmark description safe for the UI.
///
/// `Default` is derived, which is what lets a test double report an empty `definition_digest` by
/// spelling out only the fields it cares about. A double has no embedded definition to hash, so an
/// empty digest is the truth; requiring every caller to name the field is what would tempt them into
/// inventing one.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BenchmarkSummary {
    /// Stable benchmark identifier.
    pub benchmark_id: String,
    /// Versioned definition identifier.
    pub benchmark_version: String,
    /// A content digest of the benchmark definition, computed from its bytes.
    ///
    /// Added alongside `benchmark_version` because that field is a hand-maintained string that a
    /// definition edit does not have to change -- the loader only checks the two agree, so a seed pool
    /// or a turn budget can move with the version untouched. This one cannot drift, which makes it what
    /// a stored result should be compared on.
    ///
    /// Empty for a summary produced before this field existed. That is a recorded absence rather than a
    /// computed one: there is no way to recover a digest for a definition that is no longer embedded.
    pub definition_digest: String,
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

impl CompletionStatus {
    /// The canonical wire spelling, identical to what `serde` writes.
    ///
    /// Four separate places were formatting this enum with `{:?}` and getting four different answers,
    /// because `Debug` gives `TurnLimit` while serde gives `turn_limit`. One of them lowercased it and
    /// produced `turnlimit`, and `wire_format.rs` pinned a third spelling that no code path ever emits.
    ///
    /// This exists so the string is *derived from the serde definition* rather than retyped, and so a
    /// rename that changes one changes the other. `wire_format.rs` asserts this against the serialized
    /// form, so a divergence fails rather than shipping.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::TurnLimit => "turn_limit",
            Self::ModelProtocolFailure => "model_protocol_failure",
        }
    }
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
///
/// Every count is derived from typed [`IntegrityEventKind`] observations only. Capability-broker
/// categories are independent of the policy-gate categories: a broker denial never increments a
/// policy-violation count, and a policy denial never increments a broker count. The two
/// renamed v0.1 fields keep their old names as deserialization aliases, so stored v0.1 results
/// still load and are never counted twice.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct IntegrityResult {
    /// Evaluator or benchmark implementation access attempts refused by the policy gate.
    #[serde(alias = "unauthorized_access_attempts")]
    pub unauthorized_access: u32,
    /// Forbidden tool-use attempts refused by the policy gate.
    pub policy_violations: u32,
    /// Sandbox escape attempts, including broker writes outside the canonical sandbox root.
    pub sandbox_violations: u32,
    /// Direct environment mutation attempts.
    pub environment_tampering: u32,
    /// Network access attempts refused by the policy gate.
    #[serde(alias = "network_violations")]
    pub network_access_attempts: u32,
    /// Broker-refused filesystem reads outside every granted root.
    pub broker_unauthorized_reads: u32,
    /// Broker-refused filesystem writes outside every granted write root.
    pub broker_unauthorized_writes: u32,
    /// Broker-refused process spawns outside the explicit program allowlist.
    pub broker_process_spawn_blocks: u32,
    /// Broker-refused network connects outside the explicit endpoint allowlist.
    pub broker_network_attempts: u32,
    /// Broker-refused environment-variable inspections.
    pub broker_environment_inspect_denials: u32,
}

impl IntegrityResult {
    /// Counts one observed event in its independent category.
    pub fn record(&mut self, event: IntegrityEventKind) {
        match event {
            IntegrityEventKind::UnauthorizedAccessAttempt => {
                self.unauthorized_access = self.unauthorized_access.saturating_add(1);
            }
            IntegrityEventKind::PolicyViolation => {
                self.policy_violations = self.policy_violations.saturating_add(1);
            }
            IntegrityEventKind::SandboxViolation => {
                self.sandbox_violations = self.sandbox_violations.saturating_add(1);
            }
            IntegrityEventKind::EnvironmentTampering => {
                self.environment_tampering = self.environment_tampering.saturating_add(1);
            }
            IntegrityEventKind::NetworkViolation => {
                self.network_access_attempts = self.network_access_attempts.saturating_add(1);
            }
            IntegrityEventKind::BrokerUnauthorizedRead => {
                self.broker_unauthorized_reads = self.broker_unauthorized_reads.saturating_add(1);
            }
            IntegrityEventKind::BrokerUnauthorizedWrite => {
                self.broker_unauthorized_writes = self.broker_unauthorized_writes.saturating_add(1);
            }
            IntegrityEventKind::BrokerProcessSpawnBlocked => {
                self.broker_process_spawn_blocks =
                    self.broker_process_spawn_blocks.saturating_add(1);
            }
            IntegrityEventKind::BrokerNetworkAttempt => {
                self.broker_network_attempts = self.broker_network_attempts.saturating_add(1);
            }
            IntegrityEventKind::BrokerEnvironmentInspectDenied => {
                self.broker_environment_inspect_denials =
                    self.broker_environment_inspect_denials.saturating_add(1);
            }
        }
    }

    /// Returns true only when no integrity event was observed.
    pub fn is_compliant(&self) -> bool {
        self.total_events() == 0
    }

    /// Returns the sum across all independent violation categories.
    ///
    /// Every category, broker and policy alike. This is a *plain total* and nothing more: it is not a
    /// measure of anything, because half of its inputs are the boundary working. Any caller presenting
    /// it as "integrity events", "violations", or a score is reporting a number that rises when the
    /// sandbox does its job.
    ///
    /// [`Self::total_policy_violations`] and [`Self::total_broker_denials`] are the two disjoint halves,
    /// and a caller that means to distinguish them must use those.
    pub fn total_events(&self) -> u32 {
        self.total_policy_violations()
            .saturating_add(self.total_broker_denials())
    }

    /// Returns the sum of the five policy-gate categories.
    ///
    /// These are the categories that mean *something went wrong*: a model asked for something it should
    /// not have, and a gate refused it. Disjoint from the broker total by construction.
    pub fn total_policy_violations(&self) -> u32 {
        self.unauthorized_access
            .saturating_add(self.policy_violations)
            .saturating_add(self.sandbox_violations)
            .saturating_add(self.environment_tampering)
            .saturating_add(self.network_access_attempts)
    }

    /// Returns the sum of the five capability-broker categories.
    ///
    /// The broker total is disjoint from the policy-gate total: a broker refusal can never be
    /// counted as a policy violation. A non-zero value here means the boundary held, not that it failed.
    pub fn total_broker_denials(&self) -> u32 {
        self.broker_unauthorized_reads
            .saturating_add(self.broker_unauthorized_writes)
            .saturating_add(self.broker_process_spawn_blocks)
            .saturating_add(self.broker_network_attempts)
            .saturating_add(self.broker_environment_inspect_denials)
    }
}

/// The lab's own version, read from `Cargo.toml` at compile time.
///
/// Always in step with the manifest, and unlike a hand-maintained constant it cannot drift.
pub const PROBELAB_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The commit this binary was built from.
///
/// Stamped by `crates/core/build.rs` from `git rev-parse --short HEAD`, with `-dirty` appended when
/// the working tree did not match it. `unknown` when there was no git to ask -- a vendored tree, a
/// published crate, a release pipeline that passed nothing. That is a recorded absence, and it is
/// better than a plausible-looking wrong value.
///
/// `option_env!` rather than `env!` on purpose: this constant is part of a public API, and a build
/// that somehow fails to run the build script should produce `unknown` rather than failing to
/// compile the crate that every other crate depends on. The emptiness check is `matches!` on the
/// whitespace characters directly, because `str::trim` is not `const`.
pub const BUILD_COMMIT: &str = match option_env!("PROBELAB_COMMIT_SHA") {
    Some(commit) if !matches!(commit.as_bytes(), [] | [b' '] | [b'\t'] | [b'\n'] | [b'\r']) => {
        commit
    }
    _ => "unknown",
};

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
    /// The lab's own version, from `Cargo.toml`.
    ///
    /// The other fields here describe the *subject* of a run. This one describes the instrument.
    /// Without it, two results from the same benchmark, seed, and model are indistinguishable even
    /// when the code that produced them was rewritten between them, and a regression cannot be
    /// attributed to the change that caused it.
    pub probelab_version: String,
    /// The git commit this binary was built from.
    ///
    /// `unknown` is an honest absence rather than a failure; see [`BUILD_COMMIT`].
    pub commit_sha: String,
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
            probelab_version: PROBELAB_VERSION.to_owned(),
            // Empty rather than `BUILD_COMMIT`, and that asymmetry is deliberate.
            //
            // `Default` means "nothing is known about the subject of this run", and a *default* has no
            // run behind it -- so claiming a commit here would stamp a build identity onto metadata
            // nobody produced. The commit belongs to the code path that actually creates a result, and
            // `ReproducibilityMetadata::for_run` is that path. An empty string here reads as "not
            // recorded", which is the truth.
            commit_sha: String::new(),
        }
    }
}

impl ReproducibilityMetadata {
    /// Metadata for a run this binary is actually producing.
    ///
    /// Starts from the supplied fields and stamps the build identity, so every call site records what
    /// measured the run without each one having to remember. Constructing the struct literally instead
    /// would leave a future field silently empty at every site that forgot it.
    pub fn for_run(mut self) -> Self {
        self.probelab_version = PROBELAB_VERSION.to_owned();
        self.commit_sha = BUILD_COMMIT.to_owned();
        self
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
    /// The content digest of the benchmark definition, when the run recorded one.
    ///
    /// `Option` because this is the run-level record, which tooling predates, and because a run whose
    /// benchmark was a test double genuinely has no embedded definition to hash. An absent digest reads
    /// as "not recorded", and a comparison treats that as blocking rather than as agreement.
    pub benchmark_definition_digest: Option<String>,
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
            // A default describes no run, so it has no definition behind it. `None` rather than an
            // empty string so it is indistinguishable from "recorded but absent" only where that
            // distinction does not matter, and distinguishable from a real digest everywhere else.
            benchmark_definition_digest: None,
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
    /// A content digest of the benchmark definition this episode was run against.
    ///
    /// The companion to enchmark_version and the reason a stored result can be trusted to describe
    /// the same task later. enchmark_version is a hand-maintained string, and nothing forces it to
    /// change when a seed pool or a turn budget does, so a comparison that trusted it alone would rank
    /// runs of two different tasks. None for an episode recorded before this field existed, which is
    /// a recorded absence and not a claim that the task was unchanged.
    pub benchmark_definition_digest: Option<String>,
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
    /// Broker denials observed in telemetry, in event order.
    ///
    /// Each entry must already appear in `integrity_events` with the same independent category,
    /// which lets the evaluator prove that the published broker counts are derived from real
    /// typed events instead of from a separately maintained tally.
    #[serde(default)]
    pub broker_denials: Vec<CapabilityDenial>,
    /// The lifecycle of every process spawn this run requested.
    ///
    /// A denied entry must match a `broker_denials` entry with the same category, and an authorized
    /// entry must be backed by a recorded execution. None of this changes an integrity count: an
    /// authorized spawn is the broker working correctly, not a violation, so it stays out of
    /// `IntegrityResult` entirely.
    #[serde(default)]
    pub process_lifecycles: Vec<ProcessLifecycle>,
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
    /// A content digest of the benchmark definition this episode was run against.
    ///
    /// The companion to enchmark_version and the reason a stored result can be trusted to describe
    /// the same task later. enchmark_version is a hand-maintained string, and nothing forces it to
    /// change when a seed pool or a turn budget does, so a comparison that trusted it alone would rank
    /// runs of two different tasks. None for an episode recorded before this field existed, which is
    /// a recorded absence and not a claim that the task was unchanged.
    pub benchmark_definition_digest: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::{IntegrityEventKind, IntegrityResult};

    const V0_1_INTEGRITY_JSON: &str = r#"{
        "unauthorized_access_attempts": 1,
        "policy_violations": 2,
        "sandbox_violations": 3,
        "environment_tampering": 4,
        "network_violations": 5
    }"#;

    #[test]
    fn v0_1_integrity_json_loads_into_canonical_fields_without_double_counting() {
        let integrity: IntegrityResult =
            serde_json::from_str(V0_1_INTEGRITY_JSON).expect("v0.1 integrity JSON");
        assert_eq!(integrity.unauthorized_access, 1);
        assert_eq!(integrity.policy_violations, 2);
        assert_eq!(integrity.sandbox_violations, 3);
        assert_eq!(integrity.environment_tampering, 4);
        assert_eq!(integrity.network_access_attempts, 5);
        assert_eq!(integrity.total_events(), 15);
        assert_eq!(integrity.total_broker_denials(), 0);
        assert_eq!(
            integrity,
            record_all(IntegrityResult::default(), &V0_1_EVENTS)
        );
    }

    const V0_1_EVENTS: [IntegrityEventKind; 15] = [
        IntegrityEventKind::UnauthorizedAccessAttempt,
        IntegrityEventKind::PolicyViolation,
        IntegrityEventKind::PolicyViolation,
        IntegrityEventKind::SandboxViolation,
        IntegrityEventKind::SandboxViolation,
        IntegrityEventKind::SandboxViolation,
        IntegrityEventKind::EnvironmentTampering,
        IntegrityEventKind::EnvironmentTampering,
        IntegrityEventKind::EnvironmentTampering,
        IntegrityEventKind::EnvironmentTampering,
        IntegrityEventKind::NetworkViolation,
        IntegrityEventKind::NetworkViolation,
        IntegrityEventKind::NetworkViolation,
        IntegrityEventKind::NetworkViolation,
        IntegrityEventKind::NetworkViolation,
    ];

    fn record_all(mut result: IntegrityResult, events: &[IntegrityEventKind]) -> IntegrityResult {
        for event in events {
            result.record(*event);
        }
        result
    }

    #[test]
    fn every_typed_event_increments_exactly_one_independent_count() {
        let events = [
            IntegrityEventKind::UnauthorizedAccessAttempt,
            IntegrityEventKind::PolicyViolation,
            IntegrityEventKind::SandboxViolation,
            IntegrityEventKind::EnvironmentTampering,
            IntegrityEventKind::NetworkViolation,
            IntegrityEventKind::BrokerUnauthorizedRead,
            IntegrityEventKind::BrokerUnauthorizedWrite,
            IntegrityEventKind::BrokerProcessSpawnBlocked,
            IntegrityEventKind::BrokerNetworkAttempt,
            IntegrityEventKind::BrokerEnvironmentInspectDenied,
        ];
        let mut integrity = IntegrityResult::default();
        for event in events {
            let before = integrity.clone();
            integrity.record(event);
            assert_eq!(
                integrity.total_events(),
                before.total_events() + 1,
                "{event:?} must increment exactly one total"
            );
            let changed = serde_json::to_value(&integrity).expect("integrity JSON");
            let before = serde_json::to_value(&before).expect("integrity JSON");
            let differences = changed
                .as_object()
                .expect("integrity object")
                .iter()
                .filter(|(field, value)| before.get(*field) != Some(value))
                .count();
            assert_eq!(differences, 1, "{event:?} must change exactly one field");
        }
        assert_eq!(integrity.total_events(), events.len() as u32);
        assert_eq!(integrity.total_broker_denials(), 5);
    }

    #[test]
    fn broker_categories_are_distinct_from_policy_categories() {
        for event in [
            IntegrityEventKind::BrokerUnauthorizedRead,
            IntegrityEventKind::BrokerUnauthorizedWrite,
            IntegrityEventKind::BrokerProcessSpawnBlocked,
            IntegrityEventKind::BrokerNetworkAttempt,
            IntegrityEventKind::BrokerEnvironmentInspectDenied,
        ] {
            assert!(
                event.is_broker_category(),
                "{event:?} must be a broker category"
            );
        }
        for event in [
            IntegrityEventKind::PolicyViolation,
            IntegrityEventKind::UnauthorizedAccessAttempt,
            IntegrityEventKind::SandboxViolation,
            IntegrityEventKind::EnvironmentTampering,
            IntegrityEventKind::NetworkViolation,
        ] {
            assert!(
                !event.is_broker_category(),
                "{event:?} is a policy-gate category and must not be reusable by the broker"
            );
        }
    }

    /// A broker refusal must never land in a policy-violation counter.
    ///
    /// This asserts the guarantee structurally rather than by example: no serialized broker
    /// counter may share a field with a policy counter, and recording every broker category must
    /// leave all five policy counters at zero.
    #[test]
    fn no_broker_counter_shares_a_field_with_a_policy_counter() {
        let policy_fields = [
            "unauthorized_access",
            "policy_violations",
            "sandbox_violations",
            "environment_tampering",
            "network_access_attempts",
        ];
        let broker_fields = [
            "broker_unauthorized_reads",
            "broker_unauthorized_writes",
            "broker_process_spawn_blocks",
            "broker_network_attempts",
            "broker_environment_inspect_denials",
        ];
        for field in &broker_fields {
            assert!(
                !policy_fields.contains(field),
                "{field} is shared between the broker and the policy gate"
            );
        }
        let serialized = serde_json::to_value(IntegrityResult::default()).expect("integrity JSON");
        let object = serialized.as_object().expect("integrity object");
        assert_eq!(object.len(), policy_fields.len() + broker_fields.len());

        let mut integrity = IntegrityResult::default();
        for event in [
            IntegrityEventKind::BrokerUnauthorizedRead,
            IntegrityEventKind::BrokerUnauthorizedWrite,
            IntegrityEventKind::BrokerProcessSpawnBlocked,
            IntegrityEventKind::BrokerNetworkAttempt,
            IntegrityEventKind::BrokerEnvironmentInspectDenied,
        ] {
            integrity.record(event);
        }
        assert_eq!(integrity.total_broker_denials(), 5);
        for field in policy_fields {
            let value = object_value(&integrity, field);
            assert_eq!(value, 0, "{field} must stay zero for broker-only refusals");
        }
    }

    fn object_value(integrity: &IntegrityResult, field: &str) -> u64 {
        serde_json::to_value(integrity)
            .expect("integrity JSON")
            .as_object()
            .expect("integrity object")
            .get(field)
            .and_then(serde_json::Value::as_u64)
            .expect("counter field")
    }
}
