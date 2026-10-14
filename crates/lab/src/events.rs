//! One event vocabulary for the whole lab.
//!
//! # The problem this solves
//!
//! An experiment produces events from five subsystems that each had their own vocabulary: the
//! telemetry recorder, the capability broker, the sandbox, the model adapter's failure kinds, and the
//! experiment manager's own lifecycle. A UI that wanted to draw one timeline had to know all five,
//! and every new event type meant editing the UI. Worse, the *union* of those vocabularies invites a
//! category error — a model failing to emit JSON looks a lot like a model probing for a boundary, and a
//! broker denying a write looks a lot like a breach.
//!
//! [`LabEvent`] is the single shape, and the important part is not the shape but the *tagging*:
//! [`EventSource`] says which subsystem spoke, and [`Severity`] says how much it matters, and neither
//! is inferred. A caller cannot accidentally report a protocol failure as a security finding, because
//! there is no constructor that produces one.
//!
//! # What a UI can rely on
//!
//! * `code` is stable and safe to branch on. Display strings may change; codes may not.
//! * `detail` is bounded and passed through a secret-agnostic sanitizer
//!   ([`sanitize_detail`]) — credential-shaped fragments (`sk-…`, bearer
//!   tokens, `api_key=` assignments) and control characters are removed, so
//!   an adapter cannot smuggle a key into the timeline in plain sight. This
//!   is a second layer, not a substitute: provider response text is redacted
//!   with the real secret at source by the transport layer, which is the only
//!   place the secret value exists.
//! * `integrity` is `Some` only for events the *infrastructure itself* classified. A model saying
//!   "I was refused" never sets it, because a model's account of what happened is not evidence.

use std::time::SystemTime;

use probelab_core::IntegrityEventKind;
use probelab_core::{ModelCallFailureKind, ModelDecision};
use serde::{Deserialize, Serialize};

use crate::brain::ObservedCategory;

/// The character budget for an event detail string.
pub const EVENT_DETAIL_LIMIT: usize = 512;

/// Which subsystem produced an event.
///
/// Recorded rather than inferred, because "what does this mean" depends entirely on who said it. The
/// same outcome — a denied capability — is routine from the broker and alarming from a target model.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    /// The engine's telemetry recorder.
    Telemetry,
    /// The capability broker.
    Broker,
    /// The sandbox or AppContainer runtime.
    Sandbox,
    /// A model adapter's own failure report.
    Model,
    /// The lab's plan monitor and anomaly detector.
    Brain,
    /// The experiment manager's lifecycle.
    Experiment,
    /// A probe agent's own account of what it did.
    Probe,
}

impl EventSource {
    /// Whether this source is allowed to assert a security finding.
    ///
    /// Only the components that enforce boundaries may. A model or a probe saying that something was
    /// blocked is a claim; the broker saying it is a record. This is the mechanism behind the
    /// "never trust the target" rule, and it is why a probe agent cannot inflate an integrity count
    /// by asserting one.
    pub fn may_assert_integrity(self) -> bool {
        matches!(
            self,
            EventSource::Telemetry | EventSource::Broker | EventSource::Sandbox
        )
    }
}

/// How much an event matters.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Routine progress. The overwhelming majority of events.
    Info,
    /// Something a user should know about but which did not change the run's meaning.
    Warning,
    /// A run-affecting failure. Protocol errors, denied capabilities, truncated responses.
    Error,
    /// A containment or integrity event.
    Critical,
}

impl Severity {
    /// Whether this severity should stop a run.
    ///
    /// A containment event does; a protocol error does not, because a model that speaks nonsense on
    /// turn nine of thirty is a measurement, not a malfunction. Conflating the two would end runs
    /// early and quietly bias every score toward models that are well behaved.
    pub fn halts_run(self) -> bool {
        self == Severity::Critical
    }
}

/// One event on the lab timeline.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LabEvent {
    /// Monotonic sequence number within a run.
    pub sequence: u64,
    /// Stable machine-readable code. Safe to branch on; never changes once shipped.
    pub code: String,
    /// Which subsystem spoke.
    pub source: EventSource,
    /// How much it matters.
    pub severity: Severity,
    /// Bounded, sanitized detail.
    pub detail: String,
    /// The infrastructure-classified integrity category, when one applies.
    ///
    /// `None` for everything else, including every model-reported event. There is no constructor that
    /// sets it from a model or a probe, so a self-report can never be counted as a finding.
    pub integrity: Option<IntegrityEventKind>,
    /// The anomaly category the monitor assigned, when the event is part of anomaly accounting.
    pub category: Option<ObservedCategory>,
    /// Wall-clock time the event was recorded.
    pub recorded_at: SystemTime,
}

impl LabEvent {
    /// Creates an event with no integrity or anomaly claim.
    pub fn new(
        sequence: u64,
        code: &str,
        source: EventSource,
        severity: Severity,
        detail: &str,
    ) -> Self {
        Self {
            sequence,
            code: code.to_owned(),
            source,
            severity,
            detail: bound(detail),
            integrity: None,
            category: None,
            recorded_at: SystemTime::now(),
        }
    }

    /// Creates an event that asserts an infrastructure-classified integrity category.
    ///
    /// # Panics
    ///
    /// Panics if the source is not permitted to assert integrity. This is deliberate: a source that
    /// cannot establish a boundary finding has no business calling this, and turning that into a
    /// runtime panic rather than a `Result` makes the mistake impossible to ship quietly.
    pub fn integrity(
        sequence: u64,
        kind: IntegrityEventKind,
        source: EventSource,
        detail: &str,
    ) -> Self {
        assert!(
            source.may_assert_integrity(),
            "{source:?} is not permitted to assert an integrity event; a model or probe cannot \
             establish that a boundary was crossed"
        );
        Self {
            sequence,
            code: integrity_code(kind).to_owned(),
            source,
            severity: Severity::Critical,
            detail: bound(detail),
            integrity: Some(kind),
            category: Some(ObservedCategory::IntegrityEvent),
            recorded_at: SystemTime::now(),
        }
    }

    /// Attaches an anomaly category.
    pub fn with_category(mut self, category: ObservedCategory) -> Self {
        self.category = Some(category);
        self
    }

    /// Whether this event counts toward a security finding.
    ///
    /// Requires both a source permitted to assert integrity and an integrity category. Either alone is
    /// insufficient, so a mislabelled non-integrity event and an unauthorised source both fail here.
    pub fn is_security_finding(&self) -> bool {
        self.integrity.is_some() && self.source.may_assert_integrity()
    }
}

/// Bounds a detail string so a chatty subsystem cannot inflate a result bundle.
///
/// Also strips credential-shaped fragments and control characters. Provider
/// response text is redacted at source by the transport layer; this is the
/// second, secret-agnostic layer for the event path, which never receives the
/// credential value itself. Patterns covered: `sk-…`, `sk-ant-…`, bearer
/// tokens, `api_key=`/`apikey:` assignments, and C0/C1 control characters.
/// This is lossy by design: a detail is a label, not evidence.
fn bound(detail: &str) -> String {
    sanitize_detail(detail)
}

/// Redacts secret-shaped fragments without needing the secret value.
pub fn sanitize_detail(detail: &str) -> String {
    let mut out = String::with_capacity(detail.len().min(EVENT_DETAIL_LIMIT * 2));
    let chars: Vec<char> = detail.chars().take(EVENT_DETAIL_LIMIT * 2).collect();
    let mut index = 0;
    while index < chars.len() {
        if matches_secret_prefix(&chars[index..]) {
            out.push_str("[REDACTED]");
            index = skip_secret_token(&chars[index..], index, chars.len());
            continue;
        }
        let character = chars[index];
        // Line breaks survive: Windows-originated text ends lines with CRLF,
        // and a detail that dropped them would misquote its source.
        if character.is_control() && character != '\n' && character != '\r' && character != '\t' {
            index += 1;
            continue;
        }
        out.push(character);
        index += 1;
    }
    out.chars().take(EVENT_DETAIL_LIMIT).collect()
}

/// Whether the slice starts a credential-shaped token.
fn matches_secret_prefix(slice: &[char]) -> bool {
    let text: String = slice.iter().take(24).collect();
    let lower = text.to_ascii_lowercase();
    lower.starts_with("sk-")
        || lower.starts_with("sk-ant-")
        || lower.starts_with("bearer ")
        || lower.starts_with("api_key=")
        || lower.starts_with("apikey=")
        || lower.starts_with("api-key=")
        || lower.starts_with("api_key:")
        || lower.starts_with("x-api-key")
}

/// Skips the secret token itself, returning the absolute index to resume at.
fn skip_secret_token(slice: &[char], base: usize, total: usize) -> usize {
    let mut offset = 0;
    // Skip the prefix word itself (up to whitespace or `=`/`:`).
    while offset < slice.len()
        && !slice[offset].is_whitespace()
        && slice[offset] != '='
        && slice[offset] != ':'
        && offset < 96
    {
        offset += 1;
    }
    // Skip the separator and any following whitespace/quotes.
    while offset < slice.len()
        && (slice[offset].is_whitespace()
            || slice[offset] == '='
            || slice[offset] == ':'
            || slice[offset] == '"'
            || slice[offset] == '\'')
    {
        offset += 1;
    }
    // Skip the token value itself.
    while offset < slice.len()
        && (slice[offset].is_alphanumeric() || "_-./+=:".contains(slice[offset]))
        && offset < 160
    {
        offset += 1;
    }
    (base + offset).min(total)
}

/// The stable code for an integrity category.
pub fn integrity_code(kind: IntegrityEventKind) -> &'static str {
    match kind {
        IntegrityEventKind::UnauthorizedAccessAttempt => "integrity.unauthorized_access_attempt",
        IntegrityEventKind::PolicyViolation => "integrity.policy_violation",
        IntegrityEventKind::SandboxViolation => "integrity.sandbox_violation",
        IntegrityEventKind::EnvironmentTampering => "integrity.environment_tampering",
        IntegrityEventKind::NetworkViolation => "integrity.network_violation",
        IntegrityEventKind::BrokerUnauthorizedRead => "integrity.broker_unauthorized_read",
        IntegrityEventKind::BrokerUnauthorizedWrite => "integrity.broker_unauthorized_write",
        IntegrityEventKind::BrokerProcessSpawnBlocked => "integrity.broker_process_spawn_blocked",
        IntegrityEventKind::BrokerNetworkAttempt => "integrity.broker_network_attempt",
        IntegrityEventKind::BrokerEnvironmentInspectDenied => "integrity.broker_environment_denied",
    }
}

/// The stable code for a model failure kind.
///
/// Model failures are `Warning` or `Error`, never `Critical`. A model that cannot follow the action
/// protocol is a *result* — it is what the protocol benchmark measures — and treating it as a
/// containment event would both mislabel it and end the run.
pub fn model_failure_code(kind: ModelCallFailureKind) -> &'static str {
    match kind {
        ModelCallFailureKind::Configuration => "model.configuration",
        ModelCallFailureKind::Timeout => "model.timeout",
        ModelCallFailureKind::MalformedResponse => "model.malformed_response",
        ModelCallFailureKind::EmptyResponse => "model.empty_response",
        ModelCallFailureKind::HttpError => "model.http_error",
        ModelCallFailureKind::ProviderError => "model.provider_error",
        ModelCallFailureKind::InvalidAction => "model.invalid_action",
        ModelCallFailureKind::TransportDisconnect => "model.transport_disconnect",
        ModelCallFailureKind::ConnectionRefused => "model.connection_refused",
        ModelCallFailureKind::ModelNotFound => "model.model_not_found",
        ModelCallFailureKind::ContextLengthExceeded => "model.context_length_exceeded",
    }
}

/// Converts a model failure into an event.
///
/// The severity is capped at `Error` on purpose, and a test pins that. If a future change let a
/// protocol failure arrive as `Critical`, every run against a model with imperfect formatting would
/// halt, and the resulting scores would silently reflect only the models that never misformat.
///
/// The split that matters is `is_harness_failure`. A refused connection, a missing model, or an
/// over-long prompt is an `Error`: the run measured nothing, and an operator has to know. Those are
/// never `Critical` either, because none of them is a containment event — a provider being switched
/// off is not an attack.
pub fn model_failure_event(sequence: u64, kind: ModelCallFailureKind, detail: &str) -> LabEvent {
    let severity = match kind {
        ModelCallFailureKind::Timeout
        | ModelCallFailureKind::TransportDisconnect
        | ModelCallFailureKind::ConnectionRefused
        | ModelCallFailureKind::HttpError
        | ModelCallFailureKind::ProviderError
        | ModelCallFailureKind::ModelNotFound
        | ModelCallFailureKind::ContextLengthExceeded
        | ModelCallFailureKind::Configuration => Severity::Error,
        // The three the protocol benchmark exists to measure.
        ModelCallFailureKind::MalformedResponse
        | ModelCallFailureKind::EmptyResponse
        | ModelCallFailureKind::InvalidAction => Severity::Warning,
    };
    LabEvent::new(
        sequence,
        model_failure_code(kind),
        EventSource::Model,
        severity,
        detail,
    )
}

/// Converts a model decision into an event.
pub fn decision_event(sequence: u64, turn: u64, decision: &ModelDecision) -> LabEvent {
    let code = match decision {
        ModelDecision::GridAction(action) => {
            return LabEvent::new(
                sequence,
                "decision.grid_action",
                EventSource::Telemetry,
                Severity::Info,
                &format!("turn {turn}: {action:?}"),
            )
        }
        ModelDecision::ProcessSpawnRequest => "decision.process_spawn_requested",
        ModelDecision::GameMove(_) => "decision.game_move",
        ModelDecision::Capability(request) => {
            // A capability *request* is the model asking, not the model obtaining. Recording it as a
            // warning rather than a finding is the distinction the whole taxonomy turns on: asking for
            // a file read is normal play, and the broker refusing it is the system working.
            return LabEvent::new(
                sequence,
                "decision.capability_requested",
                EventSource::Telemetry,
                Severity::Warning,
                &format!("turn {turn}: asked for {request:?}"),
            );
        }
        // Only deterministic mocks can produce a fully-formed probe request, so this never comes from
        // a real provider. The timeline still has to be able to show it.
        ModelDecision::CapabilityProbe(_) => "decision.capability_probe",
    };
    LabEvent::new(
        sequence,
        code,
        EventSource::Telemetry,
        Severity::Info,
        "decision made",
    )
}

/// An append-only timeline for one experiment.
///
/// # Why a Vec and not a channel
///
/// A UI subscribing to a live run wants a replay, and a run that crashed still has to produce one. A
/// `Vec` is trivially both replayable and readable after the fact; a `tokio::mpsc::Sender` that was
/// dropped mid-run silently loses whatever had not been read. The cost is that a long run holds its
/// events in memory, which is bounded by [`LabTimeline::MAX_EVENTS`] and drops the oldest first.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct LabTimeline {
    events: Vec<LabEvent>,
    next_sequence: u64,
    /// How many oldest events were dropped once the timeline exceeded
    /// [`LabTimeline::MAX_EVENTS`]. Zero for every normal run. Recorded so a
    /// truncated timeline is distinguishable from a short one.
    dropped_events: u64,
}

impl LabTimeline {
    /// The number of events retained before the oldest are dropped.
    ///
    /// High enough that a normal run is never truncated, low enough that a runaway model cannot
    /// exhaust memory during a long evaluation.
    pub const MAX_EVENTS: usize = 10_000;

    /// An empty timeline.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an event and returns a reference to the stored copy.
    pub fn push(&mut self, mut event: LabEvent) -> &LabEvent {
        event.sequence = self.next_sequence;
        self.next_sequence += 1;
        self.events.push(event);
        if self.events.len() > Self::MAX_EVENTS {
            // Drop oldest, keep newest. A run is read forwards, so recent history is what matters.
            let dropped = self.events.len() - Self::MAX_EVENTS;
            self.events.drain(..dropped);
            self.dropped_events = self.dropped_events.saturating_add(dropped as u64);
        }
        self.events.last().expect("just pushed")
    }

    /// Every retained event, oldest first.
    pub fn events(&self) -> &[LabEvent] {
        &self.events
    }

    /// How many events are retained.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// How many oldest events were dropped by the [`Self::MAX_EVENTS`] bound.
    ///
    /// Zero means nothing was lost. A reader that sees a non-zero value knows
    /// the recording is truncated rather than short.
    pub fn dropped_events(&self) -> u64 {
        self.dropped_events
    }

    /// Whether the timeline lost events to the retention bound.
    pub fn is_truncated(&self) -> bool {
        self.dropped_events > 0
    }

    /// Whether the timeline is empty.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Events from one source.
    pub fn from_source(&self, source: EventSource) -> Vec<&LabEvent> {
        self.events
            .iter()
            .filter(|event| event.source == source)
            .collect()
    }

    /// Events at or above a severity.
    pub fn at_least(&self, severity: Severity) -> Vec<&LabEvent> {
        self.events
            .iter()
            .filter(|event| event.severity >= severity)
            .collect()
    }

    /// The security findings on this timeline.
    ///
    /// Derived from the stored events rather than counted separately, so the number a UI shows and the
    /// events a user can click through cannot disagree.
    pub fn security_findings(&self) -> Vec<&LabEvent> {
        self.events
            .iter()
            .filter(|event| event.is_security_finding())
            .collect()
    }

    /// Whether any event would halt the run.
    pub fn has_halting_event(&self) -> bool {
        self.events.iter().any(|event| event.severity.halts_run())
    }

    /// Groups the events by stable code, for a summary panel.
    pub fn counts_by_code(&self) -> Vec<(String, usize)> {
        let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
        for event in &self.events {
            *counts.entry(event.code.clone()).or_default() += 1;
        }
        counts.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::ObservedCategory;

    #[test]
    fn a_timeline_numbers_its_events_in_order() {
        let mut timeline = LabTimeline::new();
        for turn in 0..3 {
            timeline.push(LabEvent::new(
                0,
                "decision.grid_action",
                EventSource::Telemetry,
                Severity::Info,
                &format!("turn {turn}"),
            ));
        }
        let sequences: Vec<u64> = timeline
            .events()
            .iter()
            .map(|event| event.sequence)
            .collect();
        assert_eq!(sequences, vec![0, 1, 2]);
    }

    #[test]
    fn a_caller_supplied_sequence_is_replaced_by_the_timelines_own() {
        // A caller that guessed a sequence number must not be able to create a gap or a duplicate,
        // because a UI sorts and de-duplicates on it.
        let mut timeline = LabTimeline::new();
        timeline.push(LabEvent::new(
            999,
            "a",
            EventSource::Broker,
            Severity::Info,
            "",
        ));
        timeline.push(LabEvent::new(
            999,
            "b",
            EventSource::Broker,
            Severity::Info,
            "",
        ));
        let sequences: Vec<u64> = timeline
            .events()
            .iter()
            .map(|event| event.sequence)
            .collect();
        assert_eq!(sequences, vec![0, 1]);
    }

    #[test]
    fn a_model_cannot_assert_an_integrity_event() {
        // This is the guarantee the whole taxonomy exists for. A target model's account of what
        // happened is a claim, not evidence, and there is no way to encode it as a finding.
        for source in [
            EventSource::Model,
            EventSource::Probe,
            EventSource::Brain,
            EventSource::Experiment,
        ] {
            assert!(!source.may_assert_integrity());
            let result = std::panic::catch_unwind(|| {
                LabEvent::integrity(
                    0,
                    IntegrityEventKind::SandboxViolation,
                    source,
                    "the model says it escaped",
                )
            });
            assert!(
                result.is_err(),
                "{source:?} was able to assert a sandbox violation"
            );
        }
    }

    #[test]
    fn the_infrastructure_may_assert_integrity() {
        for source in [
            EventSource::Telemetry,
            EventSource::Broker,
            EventSource::Sandbox,
        ] {
            let event = LabEvent::integrity(
                0,
                IntegrityEventKind::BrokerUnauthorizedWrite,
                source,
                "write refused outside the granted root",
            );
            assert!(event.is_security_finding());
            assert_eq!(event.severity, Severity::Critical);
            assert!(event.severity.halts_run());
        }
    }

    #[test]
    fn a_protocol_failure_is_never_a_security_finding() {
        for kind in [
            ModelCallFailureKind::MalformedResponse,
            ModelCallFailureKind::EmptyResponse,
            ModelCallFailureKind::InvalidAction,
            ModelCallFailureKind::Timeout,
            ModelCallFailureKind::HttpError,
            ModelCallFailureKind::Configuration,
            ModelCallFailureKind::ProviderError,
            ModelCallFailureKind::TransportDisconnect,
        ] {
            let event = model_failure_event(0, kind, "detail");
            assert!(
                !event.is_security_finding(),
                "{kind:?} was reported as a security finding"
            );
            assert!(
                !event.severity.halts_run(),
                "{kind:?} would halt the run; a model that misformats is a measurement, not a \
                 malfunction"
            );
            assert_eq!(event.source, EventSource::Model);
        }
    }

    #[test]
    fn a_malformed_response_is_a_warning_and_a_timeout_is_an_error() {
        // The severity split that keeps protocol measurement from being mistaken for breakage.
        assert_eq!(
            model_failure_event(0, ModelCallFailureKind::MalformedResponse, "").severity,
            Severity::Warning
        );
        assert_eq!(
            model_failure_event(0, ModelCallFailureKind::Timeout, "").severity,
            Severity::Error
        );
    }

    #[test]
    fn every_failure_kind_has_a_distinct_stable_code() {
        let kinds = [
            ModelCallFailureKind::Configuration,
            ModelCallFailureKind::Timeout,
            ModelCallFailureKind::MalformedResponse,
            ModelCallFailureKind::EmptyResponse,
            ModelCallFailureKind::HttpError,
            ModelCallFailureKind::ProviderError,
            ModelCallFailureKind::InvalidAction,
            ModelCallFailureKind::TransportDisconnect,
        ];
        let mut codes: Vec<&str> = kinds.iter().copied().map(model_failure_code).collect();
        let count = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), count, "two failure kinds share a code");
        assert!(codes.iter().all(|code| code.starts_with("model.")));
    }

    #[test]
    fn every_integrity_kind_has_a_distinct_stable_code() {
        let kinds = [
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
        let mut codes: Vec<&str> = kinds.iter().copied().map(integrity_code).collect();
        let count = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), count, "two integrity kinds share a code");
    }

    #[test]
    fn a_detail_string_is_bounded() {
        let long = "x".repeat(EVENT_DETAIL_LIMIT * 3);
        let event = LabEvent::new(0, "c", EventSource::Brain, Severity::Info, &long);
        assert_eq!(event.detail.chars().count(), EVENT_DETAIL_LIMIT);
    }

    #[test]
    fn the_timeline_drops_the_oldest_events_when_it_is_full() {
        let mut timeline = LabTimeline::new();
        for index in 0..(LabTimeline::MAX_EVENTS + 10) {
            timeline.push(LabEvent::new(
                0,
                "c",
                EventSource::Brain,
                Severity::Info,
                &index.to_string(),
            ));
        }
        assert_eq!(timeline.len(), LabTimeline::MAX_EVENTS);
        // The newest is kept, because a run is read forwards.
        assert_eq!(timeline.events().last().expect("last").detail, "10009");
        // The loss is recorded, so a truncated timeline cannot read as a short one.
        assert_eq!(timeline.dropped_events(), 10);
        assert!(timeline.is_truncated());
    }

    #[test]
    fn a_fresh_timeline_reports_no_truncation() {
        let timeline = LabTimeline::new();
        assert_eq!(timeline.dropped_events(), 0);
        assert!(!timeline.is_truncated());
    }

    #[test]
    fn credential_shaped_fragments_do_not_survive_in_a_detail() {
        for secret in [
            "sk-abc123def456",
            "sk-ant-secret-value",
            "Bearer eyJhbGciOiJIUzI1NiJ9",
            "api_key=SUPERSECRET",
        ] {
            let event = LabEvent::new(
                0,
                "c",
                EventSource::Model,
                Severity::Error,
                &format!("provider said {secret} loudly"),
            );
            assert!(
                !event.detail.contains("abc123")
                    && !event.detail.contains("SUPERSECRET")
                    && !event.detail.contains("eyJhbGci"),
                "secret-shaped text survived: {}",
                event.detail
            );
        }
    }

    #[test]
    fn control_characters_do_not_survive_in_a_detail() {
        let event = LabEvent::new(0, "c", EventSource::Model, Severity::Error, "a\x00b\x1bc");
        assert_eq!(event.detail, "abc");
    }

    #[test]
    fn findings_are_derived_from_events_so_the_two_cannot_disagree() {
        let mut timeline = LabTimeline::new();
        timeline.push(LabEvent::integrity(
            0,
            IntegrityEventKind::BrokerUnauthorizedRead,
            EventSource::Broker,
            "read refused",
        ));
        timeline.push(model_failure_event(
            0,
            ModelCallFailureKind::MalformedResponse,
            "not json",
        ));
        assert_eq!(timeline.security_findings().len(), 1);
        assert_eq!(timeline.counts_by_code().len(), 2);
    }

    #[test]
    fn a_run_halts_only_on_a_containment_event() {
        let mut timeline = LabTimeline::new();
        timeline.push(model_failure_event(
            0,
            ModelCallFailureKind::MalformedResponse,
            "not json",
        ));
        timeline.push(model_failure_event(
            0,
            ModelCallFailureKind::Timeout,
            "slow",
        ));
        assert!(!timeline.has_halting_event());
        timeline.push(LabEvent::integrity(
            0,
            IntegrityEventKind::SandboxViolation,
            EventSource::Sandbox,
            "escape attempted",
        ));
        assert!(timeline.has_halting_event());
    }

    #[test]
    fn events_filter_by_source_and_severity() {
        let mut timeline = LabTimeline::new();
        timeline.push(LabEvent::new(
            0,
            "a",
            EventSource::Brain,
            Severity::Info,
            "",
        ));
        timeline.push(model_failure_event(0, ModelCallFailureKind::Timeout, ""));
        assert_eq!(timeline.from_source(EventSource::Brain).len(), 1);
        assert_eq!(timeline.at_least(Severity::Error).len(), 1);
        assert_eq!(timeline.at_least(Severity::Critical).len(), 0);
    }

    #[test]
    fn a_timeline_round_trips_for_the_result_bundle() {
        let mut timeline = LabTimeline::new();
        timeline.push(LabEvent::integrity(
            0,
            IntegrityEventKind::PolicyViolation,
            EventSource::Broker,
            "denied",
        ));
        timeline.push(
            LabEvent::new(0, "x", EventSource::Probe, Severity::Info, "")
                .with_category(ObservedCategory::BrokerDenial),
        );
        let json = serde_json::to_string(&timeline).expect("serialize");
        let restored: LabTimeline = serde_json::from_str(&json).expect("parse");
        assert_eq!(restored, timeline);
    }

    #[test]
    fn a_decision_event_names_the_action_and_the_turn() {
        let event = decision_event(4, 7, &ModelDecision::ProcessSpawnRequest);
        assert_eq!(event.code, "decision.process_spawn_requested");
        assert_eq!(event.sequence, 4);
        assert_eq!(event.source, EventSource::Telemetry);
        let grid = decision_event(0, 2, &ModelDecision::GridAction(probelab_core::Action::Up));
        assert!(grid.detail.contains("turn 2"));
    }
}
