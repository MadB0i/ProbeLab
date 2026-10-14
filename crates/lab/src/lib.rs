//! # PROBE//LAB — orchestration layer
//!
//! This crate is the layer *above* the engine: the registries that describe what can be run, the Lab
//! Brain that decides what to run, the probe agents that challenge a target, and the experiment
//! manager that owns a run's lifecycle and its cleanup.
//!
//! # The one direction that matters
//!
//! This crate depends on the broker and the sandbox. Neither depends on this crate. That is not a
//! layering convenience, it is the mechanism by which the master prompt's first security rule is
//! enforced: nothing above the boundary can reach below it except through the same
//! [`probelab_core::CapabilityBroker`] and [`probelab_sandbox`] entry points an evaluated model uses.
//!
//! There is deliberately no `Command::new` anywhere in this crate, and no path by which the Lab Brain
//! can construct a capability request of its own. The Brain plans *experiments*; the broker decides
//! *actions*. A future change that gives the Brain a direct execution path would turn the
//! orchestrator into a universal privileged layer, and `tests::brain_cannot_bypass_the_broker` is
//! written to fail loudly if it happens.

#![deny(missing_docs)]

mod agents;
mod arena;
pub mod arena_stats;
pub mod arena_store;
mod brain;
pub mod comparison;
mod events;
mod experiment;
mod probes;
mod replay;
pub mod runner;
mod security;
pub mod settings;
pub mod settings_store;
pub mod settings_view;
mod view;

pub use agents::{
    AgentBuildError, AgentCategory, AgentExecution, AgentRegistrationError, AgentRegistry,
    AgentSpec,
};
pub use arena::{
    available_games, provenance_digest, ArenaGame, ArenaMatch, ArenaMatchSpec, ArenaMatchView,
    ArenaOutcome, ArenaRunError, ArenaRunner, IllegalMoveView, PlyView, TurnTelemetry,
};
pub use arena_stats::{
    arena_statistics, ArenaStatistics, CostStatistics, DecisionStatistics, MatchStatistics,
    OutcomeCounts, PlyStatistics, DEFAULT_CONFIDENCE, SMALL_SAMPLE_MATCHES,
};
pub use arena_store::{ArenaMatchStoreError, ArenaMatchSummary, SqliteArenaMatchStore};
pub use brain::{
    category_label, label_for, Anomaly, AnomalyDetector, BrainEvent, BrainPanel, BrainState,
    ExperimentPlan, ExperimentPlanError, LabBrain, ObservedCategory, PlanRequest, MAX_EPISODES,
    MAX_PROBES, MAX_WALL_CLOCK_SECONDS,
};
pub use comparison::{
    compare, ComparableRun, Comparison, ComparisonOutcome, Incomparable, IncomparableDimension,
    Standing,
};
pub use events::{
    decision_event, integrity_code, model_failure_code, model_failure_event, EventSource, LabEvent,
    LabTimeline, Severity,
};
pub use experiment::{
    ExperimentArchive, ExperimentManager, ExperimentRecord, ExperimentState, ExperimentSummary,
    ExperimentSummaryRow, ExperimentTransitionError, TerminalReason,
};
pub use probes::{
    probe_request, ProbeAgent, ProbeCategory, ProbeContext, ProbeObservation, ProbeOutcome,
    ProbeRegistry, ProbeReport, ProbeSpec,
};
pub use replay::{
    frame_at, replay_experiment, replay_match, Replay, ReplayCapability, ReplayFrame, ReplayKind,
    ReplaySource,
};
pub use runner::{
    EpisodeFailure, ExperimentAggregate, ExperimentObserver, ExperimentRun, LabRunner, RunnerError,
    SilentObserver, WilsonInterval,
};
pub use security::{
    all_probes, probes_for, run_probe, sweep, BoundaryOutcome, SecurityLabError, SecurityLabReport,
    SecurityProbeReport, SecurityProbeSpec,
};
pub use settings::{
    ConnectedProvider, DefaultRunMode, ExperimentDefaults, LabSettings, ProviderDefaults,
    ReadOnlyFact, SeedBehaviour, SettingScope, SettingsError, MAX_DEFAULT_EPISODES,
    MAX_TIMEOUT_SECONDS, MIN_TIMEOUT_SECONDS, SETTINGS_SCHEMA_VERSION,
};
pub use settings_store::SqliteSettingsStore;
pub use settings_view::{
    ExperimentDefaultsView, ProviderDefaultsView, ProviderState, SettingEntry, SettingsView,
};
pub use view::{
    BrainReadModel, DecisionView, EpisodeFailureView, EpisodeView, ExperimentView, SecuritySummary,
    SignalInterpretation, SignalView, TimelineEntryView,
};
