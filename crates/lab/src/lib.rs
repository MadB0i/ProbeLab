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
mod brain;
mod experiment;
mod probes;

pub use agents::{
    AgentBuildError, AgentCategory, AgentExecution, AgentRegistrationError, AgentRegistry,
    AgentSpec,
};
pub use brain::{
    category_label, label_for, Anomaly, AnomalyDetector, BrainEvent, BrainPanel, BrainState,
    ExperimentPlan, ExperimentPlanError, LabBrain, ObservedCategory, PlanRequest, MAX_EPISODES,
    MAX_PROBES, MAX_WALL_CLOCK_SECONDS,
};
pub use experiment::{
    ExperimentArchive, ExperimentManager, ExperimentRecord, ExperimentState, ExperimentSummary,
    ExperimentSummaryRow, ExperimentTransitionError, TerminalReason,
};
pub use probes::{
    probe_request, ProbeAgent, ProbeCategory, ProbeContext, ProbeObservation, ProbeOutcome,
    ProbeRegistry, ProbeReport, ProbeSpec,
};
