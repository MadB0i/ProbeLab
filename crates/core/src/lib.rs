//! Trusted domain contracts and data types shared by ProbeLab components.

#![deny(missing_docs)]

mod action;
mod capability;
mod error;
mod execution;
mod game;
mod model;
mod observation;
mod outcome;
mod result;
mod security;
mod telemetry;
mod traits;

pub use action::Action;
pub use capability::{
    CapabilityDecision, CapabilityDenial, CapabilityPermission, CapabilityPolicy,
    CapabilityPolicyError, CapabilityRequest,
};
pub use error::DomainError;
pub use execution::{
    ContainmentFailure, ContainmentFailureKind, ContainmentStatus, IntegrityLevel,
    ProcessAuthorization, ProcessBudget, ProcessExecution, ProcessLifecycle, ProcessPlanError,
    ProcessSpawnPlan, ProcessSpawner, StreamCapture, TerminationReason, STREAM_PREVIEW_LIMIT,
};
pub use game::{
    BattleshipCell, ConnectFourCell, GameBoard, GameKind, GameMove, GameRequest, GameSide,
    Position, RpsChoice, TicTacToeCell, GAME_HISTORY_WINDOW,
};
pub use model::{
    HistoryEntry, ModelCallFailure, ModelCallFailureKind, ModelCallInfo, ModelDecision,
    ModelDescriptor, ModelRequest, ModelResponse, RequestedCapability, TokenUsage, HISTORY_WINDOW,
    NO_GRID, NO_GRID_POSITION,
};
pub use observation::{Coordinate, GridSize, Observation, TileKind, VisibleTile};
pub use outcome::{ActionOutcome, ActionResultKind, Episode, EpisodeStatus, StepOutcome};
pub use result::{
    ActionTraceEntry, BenchmarkSummary, CapabilityResult, CompletedRun, CompletionStatus,
    EvaluationInput, IntegrityResult, ProtocolAdherence, ReproducibilityMetadata, RunMetadata,
    TestResult, BUILD_COMMIT, PROBELAB_VERSION, PROTOCOL_FAILURE_THRESHOLD,
};
pub use security::{IntegrityEventKind, PolicyDecision, RuntimeAuthorization};
pub use telemetry::{GameTelemetry, ResourceInfo, TelemetryEvent};
pub use traits::{
    Agent, AsyncModelProvider, Benchmark, Environment, Evaluator, ModelProvider,
    ModelProviderFuture, PolicyGate, RuntimeGuardian, Sandbox, TelemetryRecorder,
};
