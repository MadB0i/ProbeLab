//! The one place an authorized `process.spawn` becomes a running process.
//!
//! This is the component that makes the capability broker's decision mean something. It is
//! deliberately the *only* code in ProbeLab that can start a process on a model's behalf, and it can
//! only be reached after a broker `Allow`:
//!
//! 1. the run's designated writable directory is prepared and verified;
//! 2. the paths the child must not read are given a read-up refusal and verified, so the child
//!    cannot read the benchmark, the evaluator, or any other host path the run declared sensitive;
//! 3. the child is created suspended inside a job object, given a low-integrity token, and only then
//!    resumed;
//! 4. the *running* child's integrity level is read back and the protection is re-verified, so a
//!    child is never observed outside the boundary it was promised;
//! 5. the child is observed to completion or terminated when its budget runs out.
//!
//! Every failure in that sequence returns a typed [`ContainmentFailure`] and leaves no process
//! running. There is no branch that starts a child with weaker containment, and no branch that
//! reports success for a platform that enforced nothing.
//!
//! # What the child still can do
//!
//! A contained child keeps its parent's user, groups, and privileges. It is refused writes outside
//! its designated directory and refused reads of the protected paths, but it can still open network
//! connections, load native code, and read anything else its user can read. This is a real boundary
//! with real limits, and the limits are stated rather than glossed over.

use std::path::{Path, PathBuf};
use std::time::Duration;

use probelab_core::{
    ContainmentFailure, ContainmentFailureKind, ContainmentStatus, IntegrityLevel, ProcessBudget,
    ProcessExecution, ProcessSpawnPlan, ProcessSpawner, TerminationReason,
};

use super::{
    process_integrity, ChildOutcome, ContainmentError, ContainmentLimits, MandatoryIntegrity,
    ProtectedPaths, RestrictedProcessGuard, RestrictedProcessSpec, SandboxDirectory,
};

/// How many distinct limits a contained job object is verified to carry.
///
/// The job is created with seven limit flags and a guard only exists when Windows accepted all of
/// them, so this is a count of enforced boundaries rather than a configuration knob.
pub const ENFORCED_JOB_LIMITS: u32 = 7;

/// Starts broker-authorized children inside Windows containment.
///
/// The designated directory and the protected paths come from trusted run configuration, never from
/// a model response. The same spawner can be reused for every spawn in a run: each call prepares and
/// tears down its own protection, so one child's protection window never overlaps another's.
pub struct ContainedProcessSpawner {
    designated_directory: PathBuf,
    protected_paths: Vec<PathBuf>,
    journal_directory: PathBuf,
    inherit_environment: bool,
}

impl ContainedProcessSpawner {
    /// Creates a spawner that writes into `designated_directory` and hides `protected_paths`.
    pub fn new(designated_directory: impl Into<PathBuf>, protected_paths: Vec<PathBuf>) -> Self {
        Self {
            designated_directory: designated_directory.into(),
            protected_paths,
            journal_directory: std::env::temp_dir().join("probelab-protected-paths"),
            inherit_environment: false,
        }
    }

    /// Places the crash-recovery journals in an explicit directory.
    pub fn with_journal_directory(mut self, journal_directory: impl Into<PathBuf>) -> Self {
        self.journal_directory = journal_directory.into();
        self
    }

    /// Lets the child inherit the host environment.
    ///
    /// This is off by default and exists for benchmarks that genuinely need it. Turning it on hands
    /// the child every variable the operator exported, including provider credentials.
    pub fn with_inherited_environment(mut self) -> Self {
        self.inherit_environment = true;
        self
    }

    /// Returns the only directory a contained child may write in.
    pub fn designated_directory(&self) -> &Path {
        &self.designated_directory
    }

    /// Returns the paths this spawner hides from contained children.
    pub fn protected_paths(&self) -> &[PathBuf] {
        &self.protected_paths
    }
}

impl ProcessSpawner for ContainedProcessSpawner {
    fn spawn_contained(
        &self,
        plan: &ProcessSpawnPlan,
    ) -> Result<ProcessExecution, ContainmentFailure> {
        if plan.program.trim().is_empty() {
            return Err(ContainmentFailure::new(
                "validate_program",
                ContainmentFailureKind::ChildStartFailed,
            ));
        }
        if !plan.budget.is_coherent() {
            return Err(ContainmentFailure::new(
                "validate_budget",
                ContainmentFailureKind::JobObjectUnavailable,
            ));
        }

        // 1. The one directory this child may write in.
        let designated =
            SandboxDirectory::prepare(&self.designated_directory).map_err(|error| {
                containment_failure(
                    "prepare_designated_directory",
                    error,
                    ContainmentFailureKind::DesignatedDirectoryUnavailable,
                )
            })?;
        designated.verify().map_err(|error| {
            containment_failure(
                "verify_designated_directory",
                error,
                ContainmentFailureKind::DesignatedDirectoryUnavailable,
            )
        })?;

        // 2. The paths this child must not read. This is held for the child's whole life, so the
        //    refusal cannot lapse while the child is still running.
        //
        //    An empty declared set is a refusal, not a pass. A benchmark that can start a child and
        //    declares nothing the child must not read has left the evaluator readable, and quietly
        //    allowing that would make this protection look stronger than it is.
        let protection = ProtectedPaths::apply_in(&self.journal_directory, &self.protected_paths)
            .map_err(|error| {
            containment_failure(
                "protect_paths",
                error,
                ContainmentFailureKind::PathProtectionUnavailable,
            )
        })?;

        let limits = limits_from(&plan.budget);
        let mut spec = RestrictedProcessSpec::new(plan.program.as_str())
            .with_args(plan.args.iter())
            .with_current_directory(child_directory(designated.path()));
        if !self.inherit_environment {
            spec = spec.without_inherited_environment();
        }

        // 3. Suspended, in a job, at low integrity, resumed last.
        let mut guard = RestrictedProcessGuard::spawn(spec, &limits).map_err(|error| {
            let kind = classify(&error);
            containment_failure("spawn_contained", error, kind)
        })?;

        // 4. Nothing is trusted until the running child is inspected.
        let pid = guard.child_process_id().map_err(|error| {
            let _ = guard.terminate();
            containment_failure(
                "child_process_id",
                error,
                ContainmentFailureKind::VerificationFailed,
            )
        })?;
        let integrity = match process_integrity(pid) {
            Ok(integrity) => integrity,
            Err(error) => {
                let _ = guard.terminate();
                return Err(containment_failure(
                    "read_child_integrity",
                    error,
                    ContainmentFailureKind::VerificationFailed,
                ));
            }
        };
        if !integrity.is_low() {
            let _ = guard.terminate();
            return Err(ContainmentFailure::new(
                "verify_child_integrity",
                ContainmentFailureKind::TokenIntegrityFailed,
            ));
        }
        if protection.verify().is_err() {
            let _ = guard.terminate();
            return Err(ContainmentFailure::new(
                "verify_protection",
                ContainmentFailureKind::VerificationFailed,
            ));
        }

        // 5. Observe the child to completion, or stop it when its budget runs out.
        let observation = guard.wait_for(plan.budget.wall_clock()).map_err(|error| {
            let _ = guard.terminate();
            containment_failure("wait_for_child", error, ContainmentFailureKind::WaitFailed)
        })?;
        let outcome = match observation {
            Some(outcome) => outcome,
            None => {
                let _ = guard.terminate();
                return Ok(ProcessExecution {
                    spawned: true,
                    pid: Some(pid),
                    exit_code: None,
                    containment: ContainmentStatus::Enforced {
                        integrity: IntegrityLevel::Low,
                        limits_applied: ENFORCED_JOB_LIMITS,
                    },
                    termination: TerminationReason::WallClockBudgetExceeded,
                    user_cpu_millis: 0,
                    kernel_cpu_millis: 0,
                });
            }
        };

        Ok(ProcessExecution {
            spawned: true,
            pid: Some(pid),
            exit_code: Some(outcome.exit_code()),
            containment: ContainmentStatus::Enforced {
                integrity: IntegrityLevel::Low,
                limits_applied: ENFORCED_JOB_LIMITS,
            },
            termination: termination_of(&outcome, &plan.budget),
            user_cpu_millis: millis(outcome.user_cpu_time()),
            kernel_cpu_millis: millis(outcome.kernel_cpu_time()),
        })
    }
}

/// Maps a core budget onto the job object limits the containment layer understands.
fn limits_from(budget: &ProcessBudget) -> ContainmentLimits {
    ContainmentLimits::strict()
        .with_job_cpu_time(budget.job_cpu_time())
        .with_per_process_cpu_time(budget.per_process_cpu_time())
        .with_job_memory_bytes(clamp(budget.job_memory_bytes))
        .with_per_process_memory_bytes(clamp(budget.per_process_memory_bytes))
        .with_active_processes(budget.active_processes)
}

/// Narrows a `u64` byte count to the `usize` the containment layer takes.
fn clamp(bytes: u64) -> usize {
    usize::try_from(bytes).unwrap_or(usize::MAX)
}

/// Returns a path a child process can actually use as its working directory.
///
/// Windows canonicalizes a real path to the verbatim `\\?\` form, which is a device path rather than
/// a drive path. `CreateProcessW` accepts it, but `cmd.exe` does not: it reports that UNC paths are
/// unsupported and silently falls back to its own directory, which would quietly run the child
/// somewhere other than the designated sandbox. The prefix is therefore stripped before the path
/// reaches the child.
fn child_directory(path: &Path) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(drive_path) if !drive_path.starts_with("UNC\\") => PathBuf::from(drive_path),
        _ => path.to_path_buf(),
    }
}

/// Converts a CPU duration to whole milliseconds, saturating rather than wrapping.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Decides why a child stopped, distinguishing a kernel CPU kill from an ordinary exit.
///
/// Windows does not document a distinct exit code for a job-object termination, so the charged CPU
/// time compared against the configured budget is the documented way to tell the two apart.
fn termination_of(outcome: &ChildOutcome, budget: &ProcessBudget) -> TerminationReason {
    if outcome.total_cpu_time() >= budget.per_process_cpu_time() {
        TerminationReason::CpuTimeLimitExceeded
    } else {
        TerminationReason::Exited
    }
}

/// Wraps a containment error as a typed failure, keeping the Win32 code when there was one.
fn containment_failure(
    step: &'static str,
    error: ContainmentError,
    kind: ContainmentFailureKind,
) -> ContainmentFailure {
    match error {
        ContainmentError::Win32 {
            step: win_step,
            code,
        } => {
            if kind == ContainmentFailureKind::ChildStartFailed {
                ContainmentFailure::new(win_step, kind).with_code(code)
            } else {
                ContainmentFailure::new(step, kind).with_code(code)
            }
        }
        other => {
            let _ = other;
            ContainmentFailure::new(step, kind)
        }
    }
}

/// Maps a containment error onto the failure kind that describes which boundary did not hold.
fn classify(error: &ContainmentError) -> ContainmentFailureKind {
    match error {
        ContainmentError::UnsupportedPlatform => ContainmentFailureKind::UnsupportedPlatform,
        ContainmentError::DesignatedDirectoryUnavailable
        | ContainmentError::NotADirectory
        | ContainmentError::MissingParentDirectory
        | ContainmentError::LabelNotVerified { .. }
        | ContainmentError::LabelMissing
        | ContainmentError::DaclNotProtected
        | ContainmentError::DaclMissing
        | ContainmentError::NotAMandatoryLabel
        | ContainmentError::UnreadableMandatoryLabel(_)
        | ContainmentError::ParentIntegrityNotHigher(_)
        | ContainmentError::LowIntegritySidMismatch(_)
        | ContainmentError::RemovalFailed => ContainmentFailureKind::DesignatedDirectoryUnavailable,
        ContainmentError::JobLimitsNotApplied
        | ContainmentError::ZeroLimit(_)
        | ContainmentError::LimitOutOfRange(_)
        | ContainmentError::InconsistentMemoryLimits => {
            ContainmentFailureKind::JobObjectUnavailable
        }
        ContainmentError::IntegrityLevelNotApplied
        | ContainmentError::TokenIntegrityLevelUnavailable => {
            ContainmentFailureKind::TokenIntegrityFailed
        }
        ContainmentError::ProtectionPathInvalid
        | ContainmentError::ProtectionNothingToProtect
        | ContainmentError::ProtectionNotVerified { .. }
        | ContainmentError::ProtectionWouldLockTrustedProcess { .. }
        | ContainmentError::ProtectionJournal { .. } => {
            ContainmentFailureKind::PathProtectionUnavailable
        }
        ContainmentError::Win32 { step, .. } => {
            if step.contains("job") || step.contains("Job") {
                ContainmentFailureKind::JobObjectUnavailable
            } else if step.contains("ntegrity") || step.contains("oken") {
                ContainmentFailureKind::TokenIntegrityFailed
            } else {
                ContainmentFailureKind::ChildStartFailed
            }
        }
        ContainmentError::EmptyProgram
        | ContainmentError::InvalidEnvironmentName
        | ContainmentError::InvalidEnvironmentValue => ContainmentFailureKind::ChildStartFailed,
    }
}

/// The core integrity level for a containment level, for the execution record.
impl From<MandatoryIntegrity> for IntegrityLevel {
    fn from(level: MandatoryIntegrity) -> Self {
        match level {
            MandatoryIntegrity::Untrusted => Self::Untrusted,
            MandatoryIntegrity::Low => Self::Low,
            MandatoryIntegrity::Medium => Self::Medium,
            MandatoryIntegrity::High => Self::High,
            MandatoryIntegrity::System => Self::System,
            MandatoryIntegrity::Protected => Self::Protected,
            MandatoryIntegrity::Unrecognized(_) => Self::Medium,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{classify, limits_from, ENFORCED_JOB_LIMITS};
    use crate::containment::ContainmentError;
    use probelab_core::{ContainmentFailureKind, ProcessBudget};

    #[test]
    fn a_budget_becomes_job_object_limits() {
        let budget = ProcessBudget::strict();
        let limits = limits_from(&budget);
        assert!(limits.validate().is_ok());
        assert_eq!(limits.job_cpu_time(), budget.job_cpu_time());
        assert_eq!(limits.per_process_cpu_time(), budget.per_process_cpu_time());
        assert_eq!(limits.active_processes(), budget.active_processes);
    }

    #[test]
    fn a_very_large_memory_budget_is_narrowed_instead_of_wrapping() {
        let budget = ProcessBudget {
            job_memory_bytes: u64::MAX,
            per_process_memory_bytes: u64::MAX,
            ..ProcessBudget::strict()
        };
        let limits = limits_from(&budget);
        assert_eq!(limits.job_memory_bytes(), usize::MAX);
        assert!(limits.validate().is_ok());
    }

    #[test]
    fn every_containment_error_maps_to_a_boundary_that_did_not_hold() {
        let cases = [
            (
                ContainmentError::UnsupportedPlatform,
                ContainmentFailureKind::UnsupportedPlatform,
            ),
            (
                ContainmentError::DaclNotProtected,
                ContainmentFailureKind::DesignatedDirectoryUnavailable,
            ),
            (
                ContainmentError::JobLimitsNotApplied,
                ContainmentFailureKind::JobObjectUnavailable,
            ),
            (
                ContainmentError::IntegrityLevelNotApplied,
                ContainmentFailureKind::TokenIntegrityFailed,
            ),
            (
                ContainmentError::ProtectionNothingToProtect,
                ContainmentFailureKind::PathProtectionUnavailable,
            ),
            (
                ContainmentError::EmptyProgram,
                ContainmentFailureKind::ChildStartFailed,
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(classify(&error), expected, "{error:?} was misclassified");
        }
        assert_eq!(
            classify(&ContainmentError::Win32 {
                step: "CreateJobObjectW",
                code: 5
            }),
            ContainmentFailureKind::JobObjectUnavailable
        );
        assert_eq!(
            classify(&ContainmentError::Win32 {
                step: "CreateProcessW",
                code: 2
            }),
            ContainmentFailureKind::ChildStartFailed
        );
    }

    #[test]
    fn the_enforced_limit_count_is_not_zero() {
        assert_eq!(ENFORCED_JOB_LIMITS, 7);
    }
}
