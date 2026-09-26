//! The first genuinely executable capability: `process.spawn`.
//!
//! Everything in this module is data. Nothing here starts a process, and no model ever sees any
//! of it. The ordering rule that makes the boundary real is:
//!
//! 1. trusted benchmark code declares a [`ProcessSpawnPlan`] — a program, arguments, and a budget;
//! 2. the capability broker reviews the resulting [`CapabilityRequest`] and returns `Allow` or
//!    `Deny`;
//! 3. **only then** is a [`ProcessSpawner`] asked to perform the authorized spawn.
//!
//! A model can reach step 1 only by asking for the `process.spawn` *permission*. It cannot name a
//! program, an argument, a working directory, or an environment variable, so there is no raw OS
//! process API anywhere in the model-facing surface. The plan is chosen by compiled-in trusted
//! code, and the broker still has to agree that the program is on its explicit allowlist.
//!
//! The types here are serializable and platform-independent. They are the durable record of what a
//! run asked for, what the broker decided, and what the operating system actually did, so they are
//! deliberately free of host-specific detail: no program path, no argument list, and no working
//! directory is recorded in a [`ProcessLifecycle`].

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{CapabilityDenial, CapabilityRequest};

/// The mandatory integrity level a contained child was actually running at.
///
/// This is read back from the running process, not from the level that was requested, so a value
/// of [`IntegrityLevel::Low`] in a [`ProcessLifecycle`] is evidence rather than intent.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityLevel {
    /// `S-1-16-0`, untrusted.
    Untrusted,
    /// `S-1-16-4096`, the level [`probelab_sandbox`] assigns to a contained child.
    Low,
    /// `S-1-16-8192`, the level an ordinary desktop process runs at.
    Medium,
    /// `S-1-16-12288`, administrator level.
    High,
    /// `S-1-16-16384`, operating system level.
    System,
    /// `S-1-16-16385`, protected-process level.
    Protected,
}

impl IntegrityLevel {
    /// Returns the mandatory level relative identifier this level names.
    pub fn rid(self) -> u32 {
        match self {
            Self::Untrusted => 0,
            Self::Low => 4096,
            Self::Medium => 8192,
            Self::High => 12288,
            Self::System => 16384,
            Self::Protected => 16385,
        }
    }

    /// Returns true when this is the low integrity level a contained child must run at.
    pub fn is_low(self) -> bool {
        self == Self::Low
    }
}

/// Why a containment contract could not be established.
///
/// A child is only ever started when the operating system has already enforced the job object and
/// the low-integrity token, so every one of these kinds means *nothing was started*. There is no
/// variant meaning "started with weaker containment", because no such path exists.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainmentFailureKind {
    /// This operating system has no containment implementation.
    UnsupportedPlatform,
    /// The run's designated writable directory could not be prepared.
    DesignatedDirectoryUnavailable,
    /// A path the child must not reach could not be made unreadable to it.
    PathProtectionUnavailable,
    /// The job object could not be created or did not accept the requested limits.
    JobObjectUnavailable,
    /// The child process could not be created, or was terminated while still suspended.
    ChildStartFailed,
    /// The child's token could not be lowered, or the level was not low when read back.
    TokenIntegrityFailed,
    /// The containment invariants could not be verified after they were applied.
    VerificationFailed,
    /// The child's exit could not be observed.
    WaitFailed,
}

impl ContainmentFailureKind {
    /// Returns true when this failure means no process was ever started.
    ///
    /// Every kind currently does, which is the point: [`ContainmentFailureKind`] has no variant
    /// for a partial or degraded start.
    pub fn started_nothing(self) -> bool {
        true
    }
}

/// The containment state that was actually in force for a child process.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "detail")]
pub enum ContainmentStatus {
    /// The child ran inside a job object under a verified low-integrity token.
    Enforced {
        /// The integrity level read back from the child's primary token.
        integrity: IntegrityLevel,
        /// The number of distinct limits Windows reported on the job.
        limits_applied: u32,
    },
    /// Containment could not be established, so no child was started.
    Failed(ContainmentFailureKind),
}

impl ContainmentStatus {
    /// Returns true only when the child really ran contained.
    pub fn is_enforced(self) -> bool {
        matches!(self, Self::Enforced { .. })
    }

    /// Returns the failure kind, or `None` when containment was enforced.
    pub fn failure(self) -> Option<ContainmentFailureKind> {
        match self {
            Self::Failed(kind) => Some(kind),
            Self::Enforced { .. } => None,
        }
    }
}

/// Why a contained child stopped running.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    /// The child returned from its own entry point.
    Exited,
    /// The child consumed its whole CPU budget and the kernel killed it.
    CpuTimeLimitExceeded,
    /// The child outlived the run's wall-clock budget and its job was terminated.
    WallClockBudgetExceeded,
    /// The host terminated the job, either explicitly or by dropping the guard.
    TerminatedByHost,
    /// The observation window closed while the child was still running.
    StillRunning,
    /// The child was never started because containment failed first.
    ContainmentFailed,
}

impl TerminationReason {
    /// Returns true when this reason means the child was never started.
    pub fn started_nothing(self) -> bool {
        matches!(self, Self::ContainmentFailed)
    }
}

/// The resource budget one authorized child process runs under.
///
/// Every field is enforced by the operating system rather than polled by ProbeLab, so a child that
/// exceeds one of them is stopped even if it never cooperates with the host.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ProcessBudget {
    /// How long the host waits for the child before terminating its job.
    pub wall_clock_millis: u64,
    /// CPU time shared by every process in the job.
    pub job_cpu_time_millis: u64,
    /// CPU time allowed for the child process itself.
    pub per_process_cpu_time_millis: u64,
    /// Memory the whole job may commit at once.
    pub job_memory_bytes: u64,
    /// Memory the child process may commit.
    pub per_process_memory_bytes: u64,
    /// How many processes may be active in the job at once.
    pub active_processes: u32,
}

impl Default for ProcessBudget {
    fn default() -> Self {
        Self::strict()
    }
}

impl ProcessBudget {
    /// Returns a small, strict budget suitable for a short-lived benchmark child.
    ///
    /// One active process is deliberate: a child that tries to fork is refused by the kernel rather
    /// than monitored.
    pub fn strict() -> Self {
        Self {
            wall_clock_millis: 30_000,
            job_cpu_time_millis: 30_000,
            per_process_cpu_time_millis: 30_000,
            job_memory_bytes: 256 * 1024 * 1024,
            per_process_memory_bytes: 128 * 1024 * 1024,
            active_processes: 1,
        }
    }

    /// Returns the wall-clock budget as a duration.
    pub fn wall_clock(&self) -> Duration {
        Duration::from_millis(self.wall_clock_millis)
    }

    /// Returns the per-process CPU budget as a duration.
    pub fn per_process_cpu_time(&self) -> Duration {
        Duration::from_millis(self.per_process_cpu_time_millis)
    }

    /// Returns the job CPU budget as a duration.
    pub fn job_cpu_time(&self) -> Duration {
        Duration::from_millis(self.job_cpu_time_millis)
    }

    /// Returns true when the budget can be expressed as a job object configuration.
    pub fn is_coherent(&self) -> bool {
        self.wall_clock_millis > 0
            && self.job_cpu_time_millis > 0
            && self.per_process_cpu_time_millis > 0
            && self.job_memory_bytes > 0
            && self.per_process_memory_bytes > 0
            && self.per_process_memory_bytes <= self.job_memory_bytes
            && self.active_processes > 0
            && self.per_process_cpu_time_millis <= self.job_cpu_time_millis
    }
}

/// A trusted, non-model-authored instruction to spawn one process.
///
/// A benchmark supplies this; a model can at most request the `process.spawn` permission that
/// causes it to be considered. The program still has to satisfy the broker's explicit allowlist,
/// so a plan is a *request* to be authorized, never a grant.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessSpawnPlan {
    /// Program name or absolute program path the broker must find on its allowlist.
    pub program: String,
    /// Arguments passed to the program, in order.
    #[serde(default)]
    pub args: Vec<String>,
    /// Resource budget the child runs under.
    #[serde(default)]
    pub budget: ProcessBudget,
}

impl ProcessSpawnPlan {
    /// Returns a plan that runs `program` under the strict default budget.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            budget: ProcessBudget::strict(),
        }
    }

    /// Appends one argument to the command line.
    pub fn with_arg(mut self, argument: impl Into<String>) -> Self {
        self.args.push(argument.into());
        self
    }

    /// Appends several arguments to the command line, in order.
    pub fn with_args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(arguments.into_iter().map(Into::into));
        self
    }

    /// Replaces the resource budget.
    pub fn with_budget(mut self, budget: ProcessBudget) -> Self {
        self.budget = budget;
        self
    }

    /// Builds the broker request this plan represents.
    ///
    /// This is the only way a plan reaches the broker, and the request carries the program and
    /// arguments so the broker can compare them against its allowlist.
    pub fn to_request(&self) -> CapabilityRequest {
        CapabilityRequest::ProcessSpawn {
            program: self.program.clone(),
            args: self.args.clone(),
        }
    }
}

/// What the operating system actually did with one authorized spawn.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessExecution {
    /// True only when a child process was created and ran inside containment.
    pub spawned: bool,
    /// The process identifier Windows assigned to the child.
    pub pid: Option<u32>,
    /// The child's exit code, when the host observed one.
    pub exit_code: Option<u32>,
    /// The containment state that was in force, or the failure that prevented it.
    pub containment: ContainmentStatus,
    /// Why the child stopped running.
    pub termination: TerminationReason,
    /// User-mode CPU time the job charged to the child.
    pub user_cpu_millis: u64,
    /// Kernel-mode CPU time the job charged to the child.
    pub kernel_cpu_millis: u64,
}

impl ProcessExecution {
    /// Returns a record for a spawn that never happened because containment failed first.
    pub fn not_started(kind: ContainmentFailureKind) -> Self {
        Self {
            spawned: false,
            pid: None,
            exit_code: None,
            containment: ContainmentStatus::Failed(kind),
            termination: TerminationReason::ContainmentFailed,
            user_cpu_millis: 0,
            kernel_cpu_millis: 0,
        }
    }

    /// Returns true when the child really ran inside enforced containment.
    pub fn ran_contained(&self) -> bool {
        self.spawned && self.containment.is_enforced()
    }

    /// Returns the integrity level the child was verified to run at, if any.
    pub fn verified_integrity(&self) -> Option<IntegrityLevel> {
        match self.containment {
            ContainmentStatus::Enforced { integrity, .. } => Some(integrity),
            ContainmentStatus::Failed(_) => None,
        }
    }
}

/// The broker's answer for one process request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "authorization", content = "denial")]
pub enum ProcessAuthorization {
    /// The broker authorized the request, so containment was asked to perform it.
    Authorized,
    /// The broker refused the request, so no process was started.
    Denied(CapabilityDenial),
}

/// The complete, durable lifecycle record of one requested process spawn.
///
/// The record is written as a single unit so that the authorization decision and the operating
/// system outcome can never be persisted apart from each other. Every field the security panel
/// needs is present and explicit:
///
/// * `requested` — a spawn was asked for at all;
/// * `decision` — `Authorized` or `Denied`, with the typed denial when refused;
/// * `execution.spawned` — whether a child was actually created;
/// * `execution.pid` — the process identifier Windows assigned;
/// * `execution.exit_code` — the observed exit code;
/// * `execution.containment` — the enforced state or the failure that prevented it;
/// * `execution.termination` — why the child stopped.
///
/// It deliberately carries no program, argument, working directory, or environment value, so it is
/// safe to persist verbatim and safe to fold into a replay digest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessLifecycle {
    /// Always true: this record exists only because a process was requested.
    pub requested: bool,
    /// The broker's decision for the request.
    pub decision: ProcessAuthorization,
    /// The operating system outcome, absent exactly when the request was denied.
    pub execution: Option<ProcessExecution>,
}

impl ProcessLifecycle {
    /// Returns the record for a request the broker refused.
    pub fn denied(denial: CapabilityDenial) -> Self {
        Self {
            requested: true,
            decision: ProcessAuthorization::Denied(denial),
            execution: None,
        }
    }

    /// Returns the record for a request the broker authorized and the host performed.
    pub fn authorized(execution: ProcessExecution) -> Self {
        Self {
            requested: true,
            decision: ProcessAuthorization::Authorized,
            execution: Some(execution),
        }
    }

    /// Returns true when the broker refused the request.
    pub fn is_denied(&self) -> bool {
        matches!(self.decision, ProcessAuthorization::Denied(_))
    }

    /// Returns the denial, when the request was refused.
    pub fn denial(&self) -> Option<&CapabilityDenial> {
        match &self.decision {
            ProcessAuthorization::Denied(denial) => Some(denial),
            ProcessAuthorization::Authorized => None,
        }
    }

    /// Returns the execution outcome, when the request was authorized.
    pub fn execution(&self) -> Option<&ProcessExecution> {
        self.execution.as_ref()
    }

    /// Returns true only when a child really ran inside enforced containment.
    pub fn ran_contained(&self) -> bool {
        self.execution
            .as_ref()
            .is_some_and(ProcessExecution::ran_contained)
    }
}

/// A typed containment failure reported by a spawner.
///
/// The step names the containment step that did not happen, and never a model-supplied value. A
/// spawner that returns this error has started nothing: there is no way to report a degraded or
/// partially contained start.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("process containment failed at step `{step}`: {kind:?}")]
pub struct ContainmentFailure {
    /// The containment step that failed.
    pub step: &'static str,
    /// The typed reason the step failed.
    pub kind: ContainmentFailureKind,
    /// A platform error code, when the platform reported one.
    pub code: Option<u32>,
}

impl ContainmentFailure {
    /// Returns a failure with no platform error code.
    pub fn new(step: &'static str, kind: ContainmentFailureKind) -> Self {
        Self {
            step,
            kind,
            code: None,
        }
    }

    /// Attaches a platform error code.
    pub fn with_code(mut self, code: u32) -> Self {
        self.code = Some(code);
        self
    }

    /// Returns the execution record for a spawn this failure prevented.
    pub fn to_execution(&self) -> ProcessExecution {
        ProcessExecution::not_started(self.kind)
    }
}

/// The only component permitted to turn an authorized `process.spawn` into a running process.
///
/// The engine calls this *after* the broker returned `Allow` and never before, which is what makes
/// the broker the primary boundary rather than a description of one. An implementation must fail
/// closed: returning `Err` means no process was started.
pub trait ProcessSpawner: Send + Sync {
    /// Performs one authorized spawn inside full containment and reports what happened.
    fn spawn_contained(
        &self,
        plan: &ProcessSpawnPlan,
    ) -> Result<ProcessExecution, ContainmentFailure>;
}

#[cfg(test)]
mod tests {
    use super::{
        ContainmentFailure, ContainmentFailureKind, ContainmentStatus, IntegrityLevel,
        ProcessAuthorization, ProcessBudget, ProcessExecution, ProcessLifecycle, ProcessSpawnPlan,
        TerminationReason,
    };
    use crate::{CapabilityDenial, CapabilityPermission, CapabilityRequest, IntegrityEventKind};

    fn denial() -> CapabilityDenial {
        CapabilityDenial::new(
            CapabilityPermission::ProcessSpawn,
            IntegrityEventKind::BrokerProcessSpawnBlocked,
            "requested program is not on the explicit process allowlist",
        )
    }

    fn enforced_spawn() -> ProcessExecution {
        ProcessExecution {
            spawned: true,
            pid: Some(4242),
            exit_code: Some(0),
            containment: ContainmentStatus::Enforced {
                integrity: IntegrityLevel::Low,
                limits_applied: 7,
            },
            termination: TerminationReason::Exited,
            user_cpu_millis: 5,
            kernel_cpu_millis: 1,
        }
    }

    #[test]
    fn a_plan_becomes_exactly_one_spawn_request() {
        let plan = ProcessSpawnPlan::new("cmd.exe").with_args(["/D", "/S", "/C", "echo probe"]);
        assert_eq!(plan.budget, ProcessBudget::strict());
        let request = plan.to_request();
        assert_eq!(request.permission(), CapabilityPermission::ProcessSpawn);
        match &request {
            CapabilityRequest::ProcessSpawn { program, args } => {
                assert_eq!(program, "cmd.exe");
                assert_eq!(args, &["/D", "/S", "/C", "echo probe"]);
            }
            other => panic!("unexpected request {other:?}"),
        }
    }

    #[test]
    fn a_denied_lifecycle_never_carries_an_execution() {
        let lifecycle = ProcessLifecycle::denied(denial());
        assert!(lifecycle.requested);
        assert!(lifecycle.is_denied());
        assert_eq!(lifecycle.denial(), Some(&denial()));
        assert!(lifecycle.execution().is_none());
        assert!(!lifecycle.ran_contained());
    }

    #[test]
    fn an_authorized_lifecycle_carries_every_required_field() {
        let lifecycle = ProcessLifecycle::authorized(enforced_spawn());
        assert!(!lifecycle.is_denied());
        assert!(lifecycle.denial().is_none());
        let execution = lifecycle.execution().expect("authorized execution");
        assert!(execution.spawned);
        assert_eq!(execution.pid, Some(4242));
        assert_eq!(execution.exit_code, Some(0));
        assert!(execution.containment.is_enforced());
        assert_eq!(execution.termination, TerminationReason::Exited);
        assert!(lifecycle.ran_contained());
    }

    #[test]
    fn a_spawn_that_never_started_reports_a_failure_and_no_identity() {
        for kind in [
            ContainmentFailureKind::UnsupportedPlatform,
            ContainmentFailureKind::PathProtectionUnavailable,
            ContainmentFailureKind::TokenIntegrityFailed,
        ] {
            let execution = ProcessExecution::not_started(kind);
            assert!(!execution.spawned);
            assert_eq!(execution.pid, None);
            assert_eq!(execution.exit_code, None);
            assert_eq!(execution.termination, TerminationReason::ContainmentFailed);
            assert!(execution.termination.started_nothing());
            assert_eq!(execution.containment.failure(), Some(kind));
            assert!(!execution.ran_contained());
            assert!(execution.verified_integrity().is_none());
            assert!(kind.started_nothing());
        }
    }

    #[test]
    fn a_containment_failure_can_never_describe_a_partial_start() {
        let failure = ContainmentFailure::new(
            "set_low_integrity",
            ContainmentFailureKind::TokenIntegrityFailed,
        )
        .with_code(5);
        let execution = failure.to_execution();
        assert!(!execution.spawned);
        assert_eq!(execution.pid, None);
        assert_eq!(failure.code, Some(5));
    }

    #[test]
    fn enforced_containment_requires_a_low_integrity_child() {
        let mut execution = enforced_spawn();
        assert!(execution.ran_contained());
        execution.containment = ContainmentStatus::Enforced {
            integrity: IntegrityLevel::Medium,
            limits_applied: 7,
        };
        assert!(
            !execution
                .verified_integrity()
                .is_some_and(IntegrityLevel::is_low),
            "a child at medium integrity is not a contained child"
        );
    }

    #[test]
    fn a_lifecycle_record_carries_no_program_or_argument() {
        let lifecycle = ProcessLifecycle::authorized(enforced_spawn());
        let json = serde_json::to_string(&lifecycle).expect("lifecycle JSON");
        for leak in [
            "cmd.exe",
            "program",
            "args",
            "echo probe",
            "working_directory",
        ] {
            assert!(!json.contains(leak), "lifecycle leaked {leak}: {json}");
        }
        assert!(json.contains("\"requested\":true"));
        assert!(json.contains("\"spawned\":true"));
        assert!(json.contains("\"pid\":4242"));
    }

    #[test]
    fn a_denial_lifecycle_serializes_only_its_typed_denial() {
        let lifecycle = ProcessLifecycle::denied(denial());
        let json = serde_json::to_string(&lifecycle).expect("lifecycle JSON");
        assert_eq!(
            json,
            r#"{"requested":true,"decision":{"authorization":"denied","denial":{"permission":"process.spawn","integrity_event":"broker_process_spawn_blocked","reason":"requested program is not on the explicit process allowlist"}},"execution":null}"#
        );
    }

    #[test]
    fn authorization_variants_are_distinct() {
        assert!(matches!(
            ProcessAuthorization::Authorized,
            ProcessAuthorization::Authorized
        ));
        assert!(matches!(
            ProcessAuthorization::Denied(denial()),
            ProcessAuthorization::Denied(_)
        ));
    }

    #[test]
    fn budgets_round_trip_and_stay_coherent() {
        let budget = ProcessBudget::strict();
        assert!(budget.is_coherent());
        assert_eq!(budget.active_processes, 1);
        assert_eq!(budget.wall_clock(), std::time::Duration::from_secs(30));
        let json = serde_json::to_string(&budget).expect("budget JSON");
        assert_eq!(
            serde_json::from_str::<ProcessBudget>(&json).expect("budget round trip"),
            budget
        );

        for incoherent in [
            ProcessBudget {
                wall_clock_millis: 0,
                ..budget
            },
            ProcessBudget {
                active_processes: 0,
                ..budget
            },
            ProcessBudget {
                per_process_memory_bytes: budget.job_memory_bytes + 1,
                ..budget
            },
            ProcessBudget {
                per_process_cpu_time_millis: budget.job_cpu_time_millis + 1,
                ..budget
            },
        ] {
            assert!(!incoherent.is_coherent());
        }
    }
}
