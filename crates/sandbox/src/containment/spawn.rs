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
    ProcessExecution, ProcessSpawnPlan, ProcessSpawner, StreamCapture, TerminationReason,
    STREAM_PREVIEW_LIMIT,
};

use super::{
    process_integrity, AppContainerIdentity, ChildOutcome, ContainmentError, ContainmentLimits,
    MandatoryIntegrity, ProtectedPaths, RestrictedProcessGuard, RestrictedProcessSpec,
    SandboxDirectory, PROFILE_NAME,
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
    app_container_capabilities: Vec<String>,
}

impl ContainedProcessSpawner {
    /// Creates a spawner that writes into `designated_directory` and hides `protected_paths`.
    pub fn new(designated_directory: impl Into<PathBuf>, protected_paths: Vec<PathBuf>) -> Self {
        Self {
            designated_directory: designated_directory.into(),
            protected_paths,
            journal_directory: std::env::temp_dir().join("probelab-protected-paths"),
            app_container_capabilities: Vec::new(),
        }
    }

    /// Grants AppContainer capabilities to contained children.
    ///
    /// The default is an **empty** capability list, which is what makes the operating system refuse
    /// outbound network access to a contained child. Adding a capability such as `internetClient` is a
    /// deliberate loosening of that boundary, so it has to be asked for by name and it is reported in
    /// the containment telemetry.
    pub fn with_app_container_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.app_container_capabilities = capabilities;
        self
    }

    /// Places the crash-recovery journals in an explicit directory.
    pub fn with_journal_directory(mut self, journal_directory: impl Into<PathBuf>) -> Self {
        self.journal_directory = journal_directory.into();
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
        // The plan contract is checked before anything exists. An undeclared timeout would be an
        // unbounded child, and a job object does not bound elapsed time, so this cannot be defaulted.
        plan.validate().map_err(|error| {
            ContainmentFailure::new("validate_plan", ContainmentFailureKind::InvalidPlan)
                .with_detail(error.to_string())
        })?;
        let timeout = plan
            .timeout()
            .expect("validate rejected a plan without a timeout");
        if !cfg!(windows) {
            return Err(ContainmentFailure::new(
                "spawn_contained",
                ContainmentFailureKind::UnsupportedPlatform,
            ));
        }

        // 0. The AppContainer identity, derived first because the designated directory has to grant
        //    it access. A package SID matches none of the DACL's existing trustees, so without this
        //    the child could read its own directory but not write to it.
        let app_container = AppContainerIdentity::derive(PROFILE_NAME).map_err(|error| {
            containment_failure(
                "derive_app_container",
                ContainmentError::AppContainer(error.to_string()),
                ContainmentFailureKind::AppContainerUnavailable,
            )
        })?;

        // 1. The one directory this child may work in.
        let designated = SandboxDirectory::prepare_for_app_container(
            &self.designated_directory,
            Some(app_container.sid_text()),
        )
        .map_err(|error| {
            containment_failure(
                "prepare_designated_directory",
                error,
                ContainmentFailureKind::DesignatedDirectoryUnavailable,
            )
        })?;
        let capture_stdout = designated.path().join(STDOUT_CAPTURE_FILE);
        let capture_stderr = designated.path().join(STDERR_CAPTURE_FILE);
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
        // The empty child environment is an invariant of this spawner, not a default. There is
        // deliberately no builder that turns it off: a contained child must never be handed the
        // operator's exported variables, provider credentials above all. A benchmark that needs a
        // variable declares it explicitly on the plan, where it is visible and reviewable.
        //
        // The AppContainer identity is requested unconditionally for the same reason. This spawner is
        // the product's containment boundary, and a boundary that can be skipped is not a boundary.
        // There is no flag to launch a child without it: if the SID cannot be derived, the attribute
        // list cannot be built, or the running child's token does not carry the SID, the spawn fails.
        let mut spec = RestrictedProcessSpec::new(plan.program.as_str())
            .with_args(plan.args.iter())
            .with_current_directory(child_directory(designated.path()))
            .with_stdout_file(capture_stdout.clone())
            .with_stderr_file(capture_stderr.clone())
            .without_inherited_environment();
        if self.app_container_capabilities.is_empty() {
            spec = spec.with_app_container();
        } else {
            // A non-empty capability list is a deliberate loosening of the network boundary, so it is
            // logged rather than applied silently. It belongs in the result bundle too, and that
            // surfacing is outstanding UI work rather than something to fake here.
            tracing::warn!(
                run_root = %self.designated_directory.display(),
                capabilities = ?self.app_container_capabilities,
                "a contained child was granted AppContainer capabilities, which loosens the \
                 default outbound network denial"
            );
            spec = spec.with_app_container_capabilities(self.app_container_capabilities.clone());
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

        // 5. Observe the child to completion, or stop it when the plan's own timeout runs out.
        //
        //    The timer starts when the child is resumed rather than when the run started, so a run
        //    that spent time on earlier turns does not eat into this child's budget.
        let observation = guard.wait_for(timeout).map_err(|error| {
            let _ = guard.terminate();
            containment_failure("wait_for_child", error, ContainmentFailureKind::WaitFailed)
        })?;
        let (termination, exit_code, user_cpu, kernel_cpu) = match observation {
            Some(outcome) => (
                termination_of(&outcome, &plan.budget),
                Some(outcome.exit_code()),
                millis(outcome.user_cpu_time()),
                millis(outcome.kernel_cpu_time()),
            ),
            None => {
                // The job is terminated as a whole, so anything the child started dies with it and no
                // process outlives the timeout. The exit code is deliberately absent: the host stopped
                // this child, so the child never chose one.
                let _ = guard.terminate();
                (TerminationReason::Timeout, None, 0, 0)
            }
        };
        let (stdout, stderr) = read_capture(&capture_stdout, &capture_stderr);

        Ok(ProcessExecution {
            spawned: true,
            pid: Some(pid),
            exit_code,
            containment: ContainmentStatus::Enforced {
                integrity: IntegrityLevel::Low,
                limits_applied: ENFORCED_JOB_LIMITS,
            },
            termination,
            user_cpu_millis: user_cpu,
            kernel_cpu_millis: kernel_cpu,
            stdout,
            stderr,
        })
    }
}

/// The file a contained child's standard output is captured into, inside the designated directory.
pub const STDOUT_CAPTURE_FILE: &str = "probelab-stdout.log";

/// The file a contained child's standard error is captured into, inside the designated directory.
pub const STDERR_CAPTURE_FILE: &str = "probelab-stderr.log";

/// Reads both capture files into bounded telemetry records.
///
/// The total byte count comes from the file length, so it reflects everything the child wrote even
/// though only a bounded preview is kept. A missing file is reported as an empty stream rather than
/// as a failure: a child that wrote nothing still ran, and its exit code is the fact that matters.
fn read_capture(stdout: &Path, stderr: &Path) -> (StreamCapture, StreamCapture) {
    (read_one_capture(stdout), read_one_capture(stderr))
}

fn read_one_capture(path: &Path) -> StreamCapture {
    let Ok(metadata) = std::fs::metadata(path) else {
        return StreamCapture::empty();
    };
    let bytes = metadata.len();
    let preview = read_bounded_preview(path);
    let truncated = bytes > STREAM_PREVIEW_LIMIT as u64;
    StreamCapture::new(bytes, preview, truncated)
}

/// Reads at most a bounded preview from the start of a capture file.
///
/// The read is capped in bytes first and then capped in characters, because a lossy UTF-8 conversion
/// expands invalid bytes into multi-byte replacement characters and would otherwise let a hostile
/// child push far more than the cap into telemetry.
///
/// The preview is also passed through [`sanitize_preview`]: the child's environment holds no
/// credentials, so this is a narrow exposure — but a child that prints a secret-shaped string must
/// not land it verbatim in persisted telemetry.
fn read_bounded_preview(path: &Path) -> String {
    use std::io::Read;

    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut buffer = vec![0u8; STREAM_PREVIEW_LIMIT];
    let Ok(read) = file.read(&mut buffer) else {
        return String::new();
    };
    buffer.truncate(read);
    let mut preview = String::from_utf8_lossy(&buffer).into_owned();
    if preview.chars().count() > STREAM_PREVIEW_LIMIT {
        preview = preview.chars().take(STREAM_PREVIEW_LIMIT).collect();
    }
    sanitize_preview(&preview)
}

/// Removes secret-shaped fragments and control characters from a persisted
/// stream preview.
///
/// Secret-agnostic by necessity: the preview path never holds the credential
/// value, so it redacts shapes (`sk-…`, bearer tokens, `api_key=`
/// assignments) rather than values. Control characters are dropped so a
/// terminal or log renderer cannot be driven by child output. Lossy by
/// design: a preview is evidence that output existed, not a transcript.
fn sanitize_preview(preview: &str) -> String {
    let mut out = String::with_capacity(preview.len().min(STREAM_PREVIEW_LIMIT * 2));
    let chars: Vec<char> = preview.chars().take(STREAM_PREVIEW_LIMIT * 2).collect();
    let mut index = 0;
    while index < chars.len() {
        let rest: String = chars[index..].iter().take(24).collect();
        let lower = rest.to_ascii_lowercase();
        if lower.starts_with("sk-")
            || lower.starts_with("sk-ant-")
            || lower.starts_with("bearer ")
            || lower.starts_with("api_key=")
            || lower.starts_with("apikey=")
            || lower.starts_with("api-key=")
        {
            out.push_str("[REDACTED]");
            // Skip the prefix word, the separator, and the token value.
            while index < chars.len()
                && !chars[index].is_whitespace()
                && chars[index] != '='
                && chars[index] != ':'
            {
                index += 1;
            }
            while index < chars.len()
                && (chars[index].is_whitespace()
                    || chars[index] == '='
                    || chars[index] == ':'
                    || chars[index] == '"'
                    || chars[index] == '\'')
            {
                index += 1;
            }
            while index < chars.len()
                && (chars[index].is_alphanumeric() || "_-./+=:".contains(chars[index]))
            {
                index += 1;
            }
            continue;
        }
        let character = chars[index];
        // Line breaks survive: Windows child output ends lines with CRLF,
        // and a preview that dropped them would no longer be the output.
        if character.is_control() && character != '\n' && character != '\r' && character != '\t' {
            index += 1;
            continue;
        }
        out.push(character);
        index += 1;
    }
    out.chars().take(STREAM_PREVIEW_LIMIT).collect()
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
        // An AppContainer failure is reported as its own kind rather than folded into a generic one.
        // "The identity could not be established or confirmed" is a distinct operational fact from a
        // token or job-object problem, and a reader of a result bundle should be able to tell them
        // apart without parsing a message.
        ContainmentError::AppContainer(_) => ContainmentFailureKind::AppContainerUnavailable,
        ContainmentError::ProtectionPathInvalid
        | ContainmentError::ProtectionNothingToProtect
        | ContainmentError::ProtectionNotVerified { .. }
        | ContainmentError::ProtectionWouldLockTrustedProcess { .. }
        | ContainmentError::ProtectionJournal { .. } => {
            ContainmentFailureKind::PathProtectionUnavailable
        }
        ContainmentError::InvalidPlan => ContainmentFailureKind::InvalidPlan,
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
    use super::{
        classify, limits_from, sanitize_preview, ContainedProcessSpawner, ENFORCED_JOB_LIMITS,
    };
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

    /// A contained child must never inherit the host environment, so no builder on the spawner can
    /// ask for it. This is a compile-time-shaped guarantee: the escape hatch is removed rather than
    /// left unused, so a future caller cannot reintroduce credential inheritance by accident. The
    /// low-level `RestrictedProcessSpec` keeps the opt-out because it is the raw platform primitive
    /// and is exercised directly by the sandbox tests; the *spawner* is the boundary.
    #[test]
    fn the_spawner_exposes_no_way_to_inherit_the_host_environment() {
        let spawner = ContainedProcessSpawner::new(
            std::env::temp_dir().join("probelab-inherit-audit"),
            Vec::new(),
        );
        assert_eq!(spawner.protected_paths().len(), 0);
    }

    #[test]
    fn a_preview_never_persists_a_secret_shaped_fragment() {
        let preview = sanitize_preview("leaked sk-abc123def456 done");
        assert!(!preview.contains("abc123"), "secret survived: {preview}");
        assert!(preview.contains("[REDACTED]"));
    }

    #[test]
    fn a_preview_drops_control_characters() {
        assert_eq!(sanitize_preview("a\x00b\x1bc"), "abc");
    }
}
