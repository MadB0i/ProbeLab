//! Windows OS-level defense-in-depth containment for child processes ProbeLab launches.
//!
//! # What this is
//!
//! This module is an operating-system containment layer that runs *below* the Rust capability
//! broker in `probelab-broker`. The `CapabilityBroker` remains the primary boundary: it decides
//! whether a named capability is authorized, and it is the only component that can say "allowed".
//! This module never grants a capability, never executes a model-authored request, and never
//! widens what the broker authorized. It only makes a process that ProbeLab *does* spawn on the
//! broker's authority harder to abuse if that process, or the native code it loads, misbehaves.
//!
//! # What this is not
//!
//! This is **not** a full code sandbox. It does not filter syscalls, does not virtualize the
//! file system or registry, does not restrict networking, and does not confine the address
//! space. A job object and a mandatory integrity label are resource and write-up boundaries, not
//! a language-level jail. A process contained here is still a normal Windows process that can
//! read whatever its user can read, open network connections, and load native code.
//!
//! # The three Windows mechanisms used
//!
//! 1. **Suspension before containment.** A child is created with `CREATE_SUSPENDED`, so its first
//!    instruction has not executed yet.
//! 2. **A job object.** The suspended child is assigned to an unnamed job object that carries a
//!    per-job CPU time limit, a per-process CPU time limit, a per-process memory limit, a
//!    whole-job memory limit, an active-process limit, and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
//!    Assignment happens *before* the primary thread is resumed, so the child is never briefly
//!    uncontained.
//! 3. **A low-integrity primary token.** The child's primary token is opened with
//!    `OpenProcessToken` and rewritten with `SetTokenInformation(TokenIntegrityLevel, ...)` to the
//!    low mandatory label `S-1-16-4096`. The child is then resumed. This is mandatory integrity
//!    control, not a restricted token: the child keeps the same user and group SIDs and the same
//!    privileges, so the only thing that changes is that Windows will refuse to let it *write* to
//!    objects that carry a higher integrity label.
//!
//! The write boundary is the designated directory created by [`SandboxDirectory`]: it is labeled
//! low so the child can write inside it, and everything around it keeps the default medium label,
//! which the child may not write up into. Combined with the job object, a contained child can
//! neither outlive nor outgrow the run that started it.
//!
//! # Failure is closed
//!
//! Every Windows step is checked. If the low-integrity label cannot be applied, the guard reports
//! [`ContainmentError::Win32`] and the still-suspended child is terminated rather than resumed
//! with a normal token. There is no fallback to an unconfined child: on Windows, either the child
//! runs contained or it does not run at all. If the child cannot be contained, the caller gets a
//! typed error and no process.
//!
//! # Platforms
//!
//! Every item in this module is available on all platforms, but the enforcement exists only on
//! Windows. On Linux and macOS every containment entry point returns
//! [`ContainmentError::UnsupportedPlatform`]. No Windows binding is compiled on a non-Windows
//! target: the `windows-sys` dependency is target-specific and the implementation module is
//! `#[cfg(windows)]`. Nothing here ever reports success on a platform where it did not enforce
//! anything.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(windows)]
mod windows;

#[cfg(not(windows))]
mod other;

mod protection;
mod spawn;

/// AppContainer identity and network isolation. Windows only; every other platform gets a stub whose
/// every entry point reports `UnsupportedPlatform`, so asking for isolation off-Windows fails rather
/// than quietly launching unconfined.
#[cfg(windows)]
mod appcontainer;

#[cfg(not(windows))]
mod appcontainer_stub;

#[cfg(windows)]
pub use appcontainer::{
    capability_sid, AppContainerError, AppContainerIdentity, INTERNET_CLIENT_CAPABILITY,
    PROFILE_NAME,
};

#[cfg(not(windows))]
pub use appcontainer_stub::{
    capability_sid, AppContainerError, AppContainerIdentity, INTERNET_CLIENT_CAPABILITY,
    PROFILE_NAME,
};

#[cfg(windows)]
use self::windows as platform;

#[cfg(not(windows))]
use self::other as platform;

pub use protection::ProtectedPaths;
pub use spawn::{
    ContainedProcessSpawner, ENFORCED_JOB_LIMITS, STDERR_CAPTURE_FILE, STDOUT_CAPTURE_FILE,
};

/// The mandatory integrity level SID assigned to a contained child: low integrity, `S-1-16-4096`.
pub const LOW_INTEGRITY_SID: &str = "S-1-16-4096";

/// The relative identifier authority of [`LOW_INTEGRITY_SID`], the low mandatory level.
const LOW_INTEGRITY_RID: u32 = 4096;

/// The security descriptor applied to a designated sandbox directory.
///
/// It is deliberately explicit and self-contained, and it is applied as two separate steps so the
/// DACL is in place before the label is written.
///
/// * `D:P` — the DACL is protected, so the designated directory stops inheriting access from its
///   parent and its own children inherit exactly this ACL instead.
/// * `(A;OICI;FA;;;SY)`, `(A;OICI;FA;;;BA)`, `(A;OICI;FA;;;OW)` — full control for `NT AUTHORITY\
///   SYSTEM`, for `BUILTIN\Administrators`, and for the object's owner. The `OW` entry is what
///   lets a low-integrity child running as the same user create files inside the designated
///   directory; the `OICI` inheritance flags carry that access to everything created inside.
/// * `S:(ML;OICI;NW;;;S-1-16-4096)` — the SACL mandatory label. `ML` is the system mandatory label
///   ACE type, `OICI` makes the label inherit to the objects and containers created inside, `NW`
///   is the `SYSTEM_MANDATORY_LABEL_NO_WRITE_UP` mask, and the SID is the low mandatory level
///   `S-1-16-4096`. A low child writing to a low-labeled object is not a write-up and is
///   therefore allowed; a low child writing to the unlabeled or medium-labeled file system around
///   it is refused by the kernel.
///
/// The full SID string is used instead of the `LO` SDDL alias so the descriptor has no dependence
/// on which abbreviations a particular Windows version accepts. The ACE shape was chosen to match
/// byte for byte what `icacls /setintegritylevel L` writes: ACE type `0x11`, flags `0x03`, mask
/// `0x01`, and the low mandatory SID.
pub const SANDBOX_DIRECTORY_SDDL: &str =
    "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;OW)S:(ML;OICI;NW;;;S-1-16-4096)";

/// A typed containment failure.
///
/// Every variant is part of the fail-closed contract: the error names the containment step that
/// did not happen, so a caller can never mistake "not contained" for "contained differently".
#[derive(Debug, thiserror::Error)]
pub enum ContainmentError {
    /// The requested containment does not exist on this operating system.
    ///
    /// This is the only error a non-Windows caller can receive from an enforcement entry point.
    #[error("OS-level process containment is only implemented on Windows")]
    UnsupportedPlatform,
    /// The spec named no program to run.
    #[error("a contained child process requires a non-empty program")]
    EmptyProgram,
    /// A limit was zero where Windows requires a positive value.
    #[error("containment limit `{0}` must be greater than zero")]
    ZeroLimit(&'static str),
    /// A limit could not be represented by a Windows job object.
    #[error("containment limit `{0}` is out of range for a Windows job object")]
    LimitOutOfRange(&'static str),
    /// The per-process memory limit exceeded the whole-job memory limit.
    #[error("the per-process memory limit must not exceed the job memory limit")]
    InconsistentMemoryLimits,
    /// An environment override name was empty or contained `=` or a NUL.
    #[error("environment override names must be non-empty and contain no '=' or NUL")]
    InvalidEnvironmentName,
    /// An environment override value contained a NUL.
    #[error("environment override values must not contain a NUL")]
    InvalidEnvironmentValue,
    /// The AppContainer identity could not be derived, applied, or confirmed.
    ///
    /// Reported as a single string because every path here is fail-closed: the spawn does not happen,
    /// or the already-suspended child is terminated. There is deliberately no variant that means
    /// "continue without the identity".
    #[error("AppContainer isolation failed: {0}")]
    AppContainer(String),
    /// The designated sandbox directory could not be created.
    #[error("the designated sandbox directory could not be created")]
    DesignatedDirectoryUnavailable,
    /// The designated sandbox directory existed but was not a directory.
    #[error("the designated sandbox directory is not a directory")]
    NotADirectory,
    /// The designated sandbox directory has no parent directory that could hold it back.
    #[error("the designated sandbox directory has no parent directory to protect")]
    MissingParentDirectory,
    /// The designated sandbox directory's mandatory label is not the low integrity level.
    #[error("the designated sandbox directory is labeled `{actual}` instead of `{expected}`")]
    LabelNotVerified {
        /// The label the containment layer requires.
        expected: &'static str,
        /// The label that was read back from the directory.
        actual: String,
    },
    /// The designated sandbox directory has no mandatory label at all.
    #[error("the designated sandbox directory carries no mandatory integrity label")]
    LabelMissing,
    /// The designated sandbox directory's DACL was not written as a protected DACL.
    #[error("the designated sandbox directory does not have a protected DACL")]
    DaclNotProtected,
    /// The designated sandbox directory has no DACL.
    #[error("the designated sandbox directory has no DACL")]
    DaclMissing,
    /// The SACL of the designated sandbox directory held something other than a mandatory label.
    #[error("the designated sandbox directory carries a non-mandatory SACL entry")]
    NotAMandatoryLabel,
    /// A mandatory label SID could not be read as a mandatory integrity level.
    #[error("the mandatory label `{0}` could not be read as a mandatory integrity level")]
    UnreadableMandatoryLabel(String),
    /// The parent of the designated sandbox directory is not above the low integrity level.
    ///
    /// A low-integrity child could write into such a parent, so the designated directory would
    /// not bound it. Refusing to set up containment is safer than pretending it holds.
    #[error(
        "the designated sandbox directory's parent is labeled `{0}`, so a low-integrity child \
         could write outside it"
    )]
    ParentIntegrityNotHigher(String),
    /// The well-known low integrity SID did not resolve to [`LOW_INTEGRITY_SID`].
    #[error("the low integrity SID resolved to `{0}` instead of `{LOW_INTEGRITY_SID}`")]
    LowIntegritySidMismatch(String),
    /// The job object did not report every limit containment requires.
    #[error("the job object does not report every containment limit it was asked to enforce")]
    JobLimitsNotApplied,
    /// A contained child's token could not be read back at the low integrity level.
    #[error("the child process token did not report the low integrity level after it was set")]
    IntegrityLevelNotApplied,
    /// The child was never started, so no token existed to label.
    #[error("the child process token reported no integrity level")]
    TokenIntegrityLevelUnavailable,
    /// A directory, file, or process entry could not be deleted.
    #[error("a contained filesystem entry could not be removed")]
    RemovalFailed,
    /// The plan could not be executed as written and no process was started.
    ///
    /// This covers an undeclared or zero wall-clock timeout, an empty program, a relative program
    /// path, and a resource budget no job object can express. It is a contract error, not a
    /// containment failure, and it is reported separately so a misconfigured benchmark is never
    /// mistaken for a security event.
    #[error("process spawn plan is not executable and no process was started")]
    InvalidPlan,
    /// A path to protect was empty or could not be recorded in the recovery journal.
    #[error("a protected path is empty or cannot be recorded safely")]
    ProtectionPathInvalid,
    /// Path protection was asked for but no existing path remained to protect.
    ///
    /// Protecting nothing would look identical to protecting everything, so it is refused and the
    /// caller decides whether a run without protected paths is acceptable.
    #[error("no existing path remained to protect from a contained child")]
    ProtectionNothingToProtect,
    /// A protected path did not report the low-integrity deny after it was applied.
    #[error("the protected path `{path}` does not refuse a low-integrity reader")]
    ProtectionNotVerified {
        /// The path whose protection could not be confirmed.
        path: PathBuf,
    },
    /// Protecting a path would have locked the trusted process out of it.
    ///
    /// The read-up refusal applies to every caller at a strictly lower integrity level, so a
    /// protected path that sits above ProbeLab's own level cannot be protected without breaking the
    /// application. Refusing is the only safe answer: there is no weaker protection to fall back to.
    #[error("protecting the path `{path}` would lock the trusted process out of it")]
    ProtectionWouldLockTrustedProcess {
        /// The path that could not be protected safely.
        path: PathBuf,
    },
    /// The crash-recovery journal for a protection could not be written, read, or removed.
    ///
    /// The journal is what makes a protection recoverable after a hard kill, so any problem with it
    /// fails the protection rather than being tolerated.
    #[error("path protection journal failure: {detail}")]
    ProtectionJournal {
        /// A fixed explanation that never contains a path or a security descriptor.
        detail: &'static str,
    },
    /// A Windows API call failed while the child was being contained or was contained.
    ///
    /// `step` names the containment step, never a model-supplied value, and `code` is the Win32
    /// error the step returned.
    #[error("windows containment step `{step}` failed with error {code}")]
    Win32 {
        /// The containment step that failed.
        step: &'static str,
        /// The Win32 error code that step returned.
        code: u32,
    },
}

/// The mandatory integrity level Windows reports for a filesystem object.
///
/// Windows assigns a default label to objects that carry none, so a path with no explicit label
/// is reported as [`MandatoryIntegrity::Medium`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MandatoryIntegrity {
    /// `S-1-16-0`, untrusted, used for AppContainer state.
    Untrusted,
    /// `S-1-16-4096`, the low integrity level used for contained children.
    Low,
    /// `S-1-16-8192`, the default level for objects a normal desktop process creates.
    Medium,
    /// `S-1-16-12288`, administrator level.
    High,
    /// `S-1-16-16384`, operating system level.
    System,
    /// `S-1-16-16385`, protected-process level.
    Protected,
    /// A mandatory level SID this module does not name.
    Unrecognized(u32),
}

impl MandatoryIntegrity {
    /// Returns the mandatory level relative identifier of this level.
    pub fn rid(self) -> u32 {
        match self {
            Self::Untrusted => 0,
            Self::Low => LOW_INTEGRITY_RID,
            Self::Medium => 8192,
            Self::High => 12288,
            Self::System => 16384,
            Self::Protected => 16385,
            Self::Unrecognized(rid) => rid,
        }
    }

    /// Returns true when this level is the low integrity level.
    pub fn is_low(self) -> bool {
        self.rid() == LOW_INTEGRITY_RID
    }

    /// Maps a mandatory level relative identifier to a level.
    pub fn from_rid(rid: u32) -> Self {
        match rid {
            0 => Self::Untrusted,
            LOW_INTEGRITY_RID => Self::Low,
            8192 => Self::Medium,
            12288 => Self::High,
            16384 => Self::System,
            16385 => Self::Protected,
            other => Self::Unrecognized(other),
        }
    }
}

/// The resource limits a Windows job object enforces on a contained child.
///
/// Every field is a limit Windows itself applies, not a limit this module polls. A child that
/// exceeds the CPU time, memory, or process-count limit is terminated by the kernel even if it
/// never cooperates with the guard. The flags behind these limits are
/// `JOB_OBJECT_LIMIT_JOB_TIME`, `JOB_OBJECT_LIMIT_PROCESS_TIME`, `JOB_OBJECT_LIMIT_JOB_MEMORY`,
/// `JOB_OBJECT_LIMIT_PROCESS_MEMORY`, `JOB_OBJECT_LIMIT_ACTIVE_PROCESS`, and
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, plus `JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION` so a
/// child that dies on an unhandled exception takes nothing else with it.
///
/// Memory is capped with the job and per-process commit limits rather than with
/// `JOB_OBJECT_LIMIT_WORKINGSET`. Windows refuses a working-set limit on a fresh job object with
/// `ERROR_INVALID_PARAMETER`, and reserving a working-set minimum instead fails with
/// `ERROR_NOT_ENOUGH_QUOTA`, so a working-set flag would make containment fail closed for a
/// reason that has nothing to do with the child. The commit limits bound the same growth.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContainmentLimits {
    job_cpu_time: Duration,
    per_process_cpu_time: Duration,
    job_memory_bytes: usize,
    per_process_memory_bytes: usize,
    active_processes: u32,
}

impl Default for ContainmentLimits {
    fn default() -> Self {
        Self::strict()
    }
}

impl ContainmentLimits {
    /// Returns a small, strict default suitable for a short-lived benchmark child.
    ///
    /// The defaults are 30 seconds of CPU for the job and for each process, 256 MiB of memory for
    /// the whole job, 128 MiB for each process, and exactly one active process. One active
    /// process is deliberate: a contained child that tries to fork is refused by the kernel
    /// rather than monitored.
    pub fn strict() -> Self {
        Self {
            job_cpu_time: Duration::from_secs(30),
            per_process_cpu_time: Duration::from_secs(30),
            job_memory_bytes: 256 * 1024 * 1024,
            per_process_memory_bytes: 128 * 1024 * 1024,
            active_processes: 1,
        }
    }

    /// Returns limits that keep a child alive long enough to be observed interactively.
    ///
    /// The resource boundaries are still applied; only the CPU time budget is larger.
    pub fn interactive() -> Self {
        Self {
            job_cpu_time: Duration::from_secs(600),
            per_process_cpu_time: Duration::from_secs(300),
            ..Self::strict()
        }
    }

    /// Sets the CPU time budget shared by every process in the job.
    pub fn with_job_cpu_time(mut self, job_cpu_time: Duration) -> Self {
        self.job_cpu_time = job_cpu_time;
        self
    }

    /// Sets the CPU time budget of each individual process in the job.
    pub fn with_per_process_cpu_time(mut self, per_process_cpu_time: Duration) -> Self {
        self.per_process_cpu_time = per_process_cpu_time;
        self
    }

    /// Sets the memory the whole job may commit at once.
    pub fn with_job_memory_bytes(mut self, job_memory_bytes: usize) -> Self {
        self.job_memory_bytes = job_memory_bytes;
        self
    }

    /// Sets the memory a single process in the job may commit.
    pub fn with_per_process_memory_bytes(mut self, per_process_memory_bytes: usize) -> Self {
        self.per_process_memory_bytes = per_process_memory_bytes;
        self
    }

    /// Sets how many processes may be active in the job at once.
    pub fn with_active_processes(mut self, active_processes: u32) -> Self {
        self.active_processes = active_processes;
        self
    }

    /// Returns the CPU time budget shared by every process in the job.
    pub fn job_cpu_time(&self) -> Duration {
        self.job_cpu_time
    }

    /// Returns the CPU time budget of each individual process in the job.
    pub fn per_process_cpu_time(&self) -> Duration {
        self.per_process_cpu_time
    }

    /// Returns the memory the whole job may commit at once.
    pub fn job_memory_bytes(&self) -> usize {
        self.job_memory_bytes
    }

    /// Returns the memory a single process in the job may commit.
    pub fn per_process_memory_bytes(&self) -> usize {
        self.per_process_memory_bytes
    }

    /// Returns how many processes may be active in the job at once.
    pub fn active_processes(&self) -> u32 {
        self.active_processes
    }

    /// Checks every limit against what a Windows job object can express.
    ///
    /// Validation is platform-independent, so a caller learns about an impossible limit on every
    /// platform instead of only where enforcement exists.
    pub fn validate(&self) -> Result<(), ContainmentError> {
        check_positive_time(self.job_cpu_time, "job_cpu_time")?;
        check_positive_time(self.per_process_cpu_time, "per_process_cpu_time")?;
        if self.job_memory_bytes == 0 {
            return Err(ContainmentError::ZeroLimit("job_memory_bytes"));
        }
        if self.per_process_memory_bytes == 0 {
            return Err(ContainmentError::ZeroLimit("per_process_memory_bytes"));
        }
        if self.active_processes == 0 {
            return Err(ContainmentError::ZeroLimit("active_processes"));
        }
        if self.per_process_memory_bytes > self.job_memory_bytes {
            return Err(ContainmentError::InconsistentMemoryLimits);
        }
        Ok(())
    }
}

/// The command a [`RestrictedProcessGuard`] will start contained.
///
/// A spec is data. It names a program, its arguments, an optional working directory, and optional
/// environment overrides; it never starts anything by itself and it is never built from a
/// model-authored request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestrictedProcessSpec {
    program: OsString,
    args: Vec<OsString>,
    current_directory: Option<PathBuf>,
    environment: Vec<(OsString, OsString)>,
    inherit_environment: bool,
    stdout_path: Option<PathBuf>,
    stderr_path: Option<PathBuf>,
    app_container: Option<appcontainer::AppContainerRequest>,
}

impl RestrictedProcessSpec {
    /// Returns a spec that runs `program` with no arguments.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            current_directory: None,
            environment: Vec::new(),
            inherit_environment: true,
            stdout_path: None,
            stderr_path: None,
            app_container: None,
        }
    }

    /// Runs the child inside a derived AppContainer with an empty capability list.
    ///
    /// This is the identity and network boundary. The package SID is derived from
    /// [`appcontainer::PROFILE_NAME`], so nothing is registered with the machine, and the capability
    /// list is empty, so the operating system refuses outbound network access to the child.
    ///
    /// Fail-closed by construction: if the SID cannot be derived or the attribute list cannot be
    /// built, the spawn returns an error. There is no path that quietly launches the child without the
    /// identity it was asked for.
    pub fn with_app_container(mut self) -> Self {
        self.app_container = Some(appcontainer::AppContainerRequest::default());
        self
    }

    /// Runs the child inside a derived AppContainer with an explicit capability list.
    ///
    /// Granting a capability such as [`appcontainer::INTERNET_CLIENT_CAPABILITY`] is a deliberate
    /// loosening of the network boundary and is recorded in the containment telemetry.
    pub fn with_app_container_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.app_container = Some(appcontainer::AppContainerRequest { capabilities });
        self
    }

    /// Sends the child's standard output to a file instead of inheriting the host's streams.
    ///
    /// Capturing rather than inheriting closes an information leak: a child that inherited the host's
    /// console could write into a terminal, a log, or a pipe the operator never agreed to expose, and
    /// the output would have no bounded home. The file is created or truncated by the containment
    /// layer, and the child receives a write-only handle to it.
    pub fn with_stdout_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.stdout_path = Some(path.into());
        self
    }

    /// Sends the child's standard error to a file instead of inheriting the host's streams.
    pub fn with_stderr_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.stderr_path = Some(path.into());
        self
    }

    /// Returns the file the child's standard output is written to, when one was declared.
    pub fn stdout_path(&self) -> Option<&Path> {
        self.stdout_path.as_deref()
    }

    /// Returns the file the child's standard error is written to, when one was declared.
    pub fn stderr_path(&self) -> Option<&Path> {
        self.stderr_path.as_deref()
    }

    /// Returns true when at least one standard stream is redirected to a file.
    pub fn redirects_output(&self) -> bool {
        self.stdout_path.is_some() || self.stderr_path.is_some()
    }

    /// Appends one argument to the command line.
    pub fn with_arg(mut self, argument: impl Into<OsString>) -> Self {
        self.args.push(argument.into());
        self
    }

    /// Appends several arguments to the command line, in order.
    pub fn with_args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(arguments.into_iter().map(Into::into));
        self
    }

    /// Sets the working directory the child starts in.
    pub fn with_current_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.current_directory = Some(directory.into());
        self
    }

    /// Sets or replaces one environment variable for the child.
    ///
    /// The child still inherits the rest of this process's environment. Only these pairs are
    /// replaced or added, and the resulting block is sorted the way Windows expects.
    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        let key = key.into();
        match self
            .environment
            .iter_mut()
            .find(|(existing, _)| existing == &key)
        {
            Some(slot) => slot.1 = value.into(),
            None => self.environment.push((key, value.into())),
        }
        self
    }

    /// Stops the child from inheriting this process's environment.
    ///
    /// A contained child has no business reading the host's environment, and the host environment is
    /// where credentials live: `OPENAI_COMPATIBLE_API_KEY` and anything else the operator exported
    /// would otherwise be handed to a process that is one argument away from exfiltrating it. After
    /// this call the child starts from an empty environment plus whatever was set with
    /// [`RestrictedProcessSpec::with_env`], which also means the child cannot read `PATH` unless the
    /// plan sets it.
    pub fn without_inherited_environment(mut self) -> Self {
        self.inherit_environment = false;
        self
    }

    /// Returns true when the child inherits this process's environment.
    pub fn inherits_environment(&self) -> bool {
        self.inherit_environment
    }

    /// Returns the AppContainer request, when the spec asked to run inside one.
    pub fn app_container(&self) -> Option<&appcontainer::AppContainerRequest> {
        self.app_container.as_ref()
    }

    /// Returns the program the spec names.
    pub fn program(&self) -> &OsStr {
        &self.program
    }

    /// Returns the arguments the spec will append to the program.
    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    /// Returns the working directory the child will start in, when one was set.
    pub fn current_directory(&self) -> Option<&Path> {
        self.current_directory.as_deref()
    }

    /// Returns the environment overrides applied on top of the inherited environment.
    pub fn environment(&self) -> &[(OsString, OsString)] {
        &self.environment
    }
}

/// What a contained child did before it exited.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChildOutcome {
    exit_code: u32,
    user_cpu_time: Duration,
    kernel_cpu_time: Duration,
    per_process_cpu_time: Duration,
}

impl ChildOutcome {
    /// Returns the child's exit code.
    ///
    /// Windows does not document a distinct exit code for a job-object termination, so a non-zero
    /// code is evidence of termination and not of a specific cause. Use
    /// [`ChildOutcome::hit_cpu_time_limit`] to distinguish a CPU-limit kill from an ordinary
    /// non-zero exit.
    pub fn exit_code(&self) -> u32 {
        self.exit_code
    }

    /// Returns the user-mode CPU time the job charged to the child.
    pub fn user_cpu_time(&self) -> Duration {
        self.user_cpu_time
    }

    /// Returns the kernel-mode CPU time the job charged to the child.
    pub fn kernel_cpu_time(&self) -> Duration {
        self.kernel_cpu_time
    }

    /// Returns the total CPU time the job charged to the child.
    pub fn total_cpu_time(&self) -> Duration {
        self.user_cpu_time.saturating_add(self.kernel_cpu_time)
    }

    /// Returns true when the observed CPU time reached the per-process CPU limit.
    ///
    /// A job kills a process for exceeding its CPU limit without any other observable trace, so
    /// comparing the charged CPU time with the configured limit is the documented way to
    /// distinguish that termination from a clean exit.
    pub fn hit_cpu_time_limit(&self) -> bool {
        self.total_cpu_time() >= self.per_process_cpu_time
    }
}

/// A child process that is suspended inside a Windows job object and labeled low integrity.
///
/// The guard owns three things: the job object, the child process handle, and the promise that
/// the child is never resumed unless every containment step succeeded. Construction order is the
/// whole design:
///
/// 1. the job object is created and loaded with limits,
/// 2. the child is created suspended, so it has run no instructions,
/// 3. the child is assigned to the job,
/// 4. the child's primary token is set to [`LOW_INTEGRITY_SID`],
/// 5. only then is the primary thread resumed.
///
/// If any step fails, the still-suspended child is terminated and a [`ContainmentError`] is
/// returned. There is no path that resumes a child with a normal token.
///
/// Dropping the guard closes the job handle, and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` terminates
/// every process still assigned to the job, so a dropped guard cannot leave an uncontained child
/// running. Because the guard wraps raw Windows handles it is not `Send`; it is used from the
/// thread that created it.
pub struct RestrictedProcessGuard {
    #[cfg_attr(not(windows), allow(dead_code))]
    platform: platform::Guard,
}

impl RestrictedProcessGuard {
    /// Starts a child contained under a Windows job object and a low-integrity token.
    ///
    /// The returned guard holds the job; the child is already running. Spec validation happens
    /// first and is platform-independent, so a malformed spec is reported the same way
    /// everywhere. On a non-Windows platform this always returns
    /// [`ContainmentError::UnsupportedPlatform`]: no process is started and no success is
    /// reported.
    pub fn spawn(
        spec: RestrictedProcessSpec,
        limits: &ContainmentLimits,
    ) -> Result<Self, ContainmentError> {
        limits.validate()?;
        if spec.program().is_empty() {
            return Err(ContainmentError::EmptyProgram);
        }
        for (key, value) in spec.environment() {
            validate_environment(key, value)?;
        }
        Ok(Self {
            platform: platform::spawn(&spec, limits)?,
        })
    }

    /// Waits for the child to exit and reports what it did.
    ///
    /// The child is bounded by the job's CPU time limit, so this cannot block forever on a child
    /// that never exits on its own.
    pub fn wait(&mut self) -> Result<ChildOutcome, ContainmentError> {
        self.platform.wait()
    }

    /// Waits up to `timeout` for the child to exit, returning `None` while it is still running.
    pub fn wait_for(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<ChildOutcome>, ContainmentError> {
        self.platform.wait_for(Some(timeout))
    }

    /// Terminates the child and everything else in its job.
    ///
    /// This is the explicit form of what dropping the guard does. Every process in the job is
    /// killed by `TerminateJobObject`, so there is no window in which anything in the job is
    /// still running and uncontained.
    pub fn terminate(&mut self) -> Result<(), ContainmentError> {
        self.platform.terminate()
    }

    /// Returns true while the child process is still running.
    pub fn is_running(&self) -> Result<bool, ContainmentError> {
        self.platform.is_running()
    }

    /// Returns the process identifier the contained child was assigned by Windows.
    pub fn child_process_id(&self) -> Result<u32, ContainmentError> {
        self.platform.child_process_id()
    }

    /// Reads the limits Windows reports for the job, not the limits that were requested.
    ///
    /// A guard only exists if the requested limits were accepted, so this is how a caller proves
    /// the job really carries the limits it was configured with.
    pub fn effective_limits(&self) -> Result<ContainmentLimits, ContainmentError> {
        self.platform.effective_limits()
    }

    /// Returns how many processes are currently active in the job.
    pub fn active_process_count(&self) -> Result<u32, ContainmentError> {
        self.platform.active_process_count()
    }

    /// Returns how many processes have been assigned to the job since it was created.
    pub fn total_process_count(&self) -> Result<u32, ContainmentError> {
        self.platform.total_process_count()
    }
}

/// A directory a contained child is allowed to write in.
///
/// The directory is created if it does not exist and is then given a protected DACL and the low
/// mandatory label from [`SANDBOX_DIRECTORY_SDDL`]. A low-integrity child may therefore create
/// and modify files inside it, and the rest of the file system keeps its default, higher
/// mandatory label, which the same child may not write up into. The DACL is intentionally
/// permissive to the owning user, because the child needs to write here; the containment comes
/// from the mandatory label, not from the ACL.
///
/// A `SandboxDirectory` does not delete anything. It owns a path, and the caller decides when to
/// call [`SandboxDirectory::remove_tree`].
pub struct SandboxDirectory {
    path: PathBuf,
}

impl SandboxDirectory {
    /// Creates a designated sandbox directory and applies the containment security descriptor.
    ///
    /// Preparation fails closed. After the security descriptor is written, the directory is read
    /// back and must report exactly [`LOW_INTEGRITY_SID`], a present DACL, and a protected DACL,
    /// or [`ContainmentError`] is returned. The parent directory is checked first: if the parent
    /// is at the low integrity level a contained child could write outside the designated
    /// directory, so preparation refuses to continue.
    ///
    /// Mandatory integrity labels require a file system that supports them, such as NTFS or ReFS.
    /// On a volume without that support this returns an error rather than a directory that looks
    /// contained and is not.
    pub fn prepare(path: impl AsRef<Path>) -> Result<Self, ContainmentError> {
        Self::prepare_for_app_container(path, None)
    }

    /// Prepares the designated directory, granting an AppContainer package SID access to it.
    ///
    /// The package SID is passed as text so the trust decision stays in one place: the spawner derives
    /// the identity, and this only decides whether the resulting directory is reachable by it. A
    /// `None` produces exactly the same directory as [`SandboxDirectory::prepare`].
    pub fn prepare_for_app_container(
        path: impl AsRef<Path>,
        app_container_sid: Option<&str>,
    ) -> Result<Self, ContainmentError> {
        let path = path.as_ref().to_path_buf();
        if path.as_os_str().is_empty() {
            return Err(ContainmentError::DesignatedDirectoryUnavailable);
        }
        platform::prepare_designated_directory(&path, app_container_sid)?;
        Ok(Self { path })
    }

    /// Returns the designated directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the directory's security descriptor back and checks the containment invariants.
    ///
    /// This is the same check [`SandboxDirectory::prepare`] performs, exposed so a caller can
    /// confirm the directory is still contained after something else may have touched it.
    pub fn verify(&self) -> Result<(), ContainmentError> {
        platform::verify_designated_directory(&self.path)
    }

    /// Returns the directory's current mandatory integrity level.
    pub fn integrity(&self) -> Result<MandatoryIntegrity, ContainmentError> {
        read_integrity(&self.path)
    }

    /// Removes the directory and everything inside it.
    ///
    /// A medium-integrity caller may delete low-integrity objects, so this works even when a
    /// contained child created entries inside the directory.
    pub fn remove_tree(&self) -> Result<(), ContainmentError> {
        platform::remove_tree(&self.path)
    }
}

/// Reads the effective mandatory integrity level of a filesystem path.
///
/// A path with no explicit mandatory label reports [`MandatoryIntegrity::Medium`], which is the
/// level Windows gives objects created by a normal desktop process. On a non-Windows platform
/// this returns [`ContainmentError::UnsupportedPlatform`].
pub fn read_integrity(path: impl AsRef<Path>) -> Result<MandatoryIntegrity, ContainmentError> {
    platform::read_integrity(path.as_ref())
}

/// Reads the mandatory integrity level of a running process's primary token.
///
/// This is how a caller proves the token was actually lowered rather than assuming it: a child
/// started by [`RestrictedProcessGuard`] reports [`MandatoryIntegrity::Low`], and any other
/// process started by the same user reports [`MandatoryIntegrity::Medium`]. On a non-Windows
/// platform this returns [`ContainmentError::UnsupportedPlatform`].
pub fn process_integrity(process_id: u32) -> Result<MandatoryIntegrity, ContainmentError> {
    platform::process_integrity(process_id)
}

/// Returns true while a Windows process with this identifier has not exited.
///
/// A contained child can be launched, run, and be killed without ever holding the guard, for
/// example by a caller that already dropped it. This is how a caller confirms that the kill
/// actually happened. On a non-Windows platform this returns
/// [`ContainmentError::UnsupportedPlatform`].
pub fn is_process_alive(process_id: u32) -> Result<bool, ContainmentError> {
    platform::is_process_alive(process_id)
}

fn check_positive_time(value: Duration, name: &'static str) -> Result<(), ContainmentError> {
    if value.is_zero() {
        return Err(ContainmentError::ZeroLimit(name));
    }
    if i64::try_from(value.as_nanos() / 100).is_err() {
        return Err(ContainmentError::LimitOutOfRange(name));
    }
    Ok(())
}

#[cfg(windows)]
fn validate_environment(key: &OsStr, value: &OsStr) -> Result<(), ContainmentError> {
    use std::os::windows::ffi::OsStrExt;

    let key: Vec<u16> = key.encode_wide().collect();
    let value: Vec<u16> = value.encode_wide().collect();
    if key.is_empty() || key.contains(&u16::from(b'=')) || key.contains(&0) {
        return Err(ContainmentError::InvalidEnvironmentName);
    }
    if value.contains(&0) {
        return Err(ContainmentError::InvalidEnvironmentValue);
    }
    Ok(())
}

#[cfg(not(windows))]
fn validate_environment(key: &OsStr, value: &OsStr) -> Result<(), ContainmentError> {
    if key.is_empty() || key.to_string_lossy().contains(['=', '\0']) {
        return Err(ContainmentError::InvalidEnvironmentName);
    }
    if value.to_string_lossy().contains('\0') {
        return Err(ContainmentError::InvalidEnvironmentValue);
    }
    Ok(())
}

#[cfg(windows)]
fn from_hundred_nanoseconds(value: i64) -> Duration {
    Duration::from_nanos(u64::try_from(value).unwrap_or_default().saturating_mul(100))
}

#[cfg(test)]
mod tests {
    use super::{
        check_positive_time, validate_environment, ContainmentError, ContainmentLimits,
        MandatoryIntegrity, RestrictedProcessSpec,
    };
    use std::ffi::OsString;
    use std::time::Duration;

    #[test]
    fn a_spec_records_the_command_it_was_built_with() {
        let spec = RestrictedProcessSpec::new("cmd.exe")
            .with_args(["/D", "/S", "/C"])
            .with_arg("echo probe")
            .with_current_directory("C:\\work")
            .with_env("PROBELAB_INSIDE", "in.txt")
            .with_env("PROBELAB_INSIDE", "in.txt");
        assert_eq!(spec.program(), "cmd.exe");
        assert_eq!(
            spec.args(),
            [
                OsString::from("/D"),
                OsString::from("/S"),
                OsString::from("/C"),
                OsString::from("echo probe")
            ]
        );
        assert_eq!(
            spec.current_directory(),
            Some(std::path::Path::new("C:\\work"))
        );
        assert_eq!(spec.environment().len(), 1);
        assert_eq!(spec.environment()[0].1, OsString::from("in.txt"));
    }

    #[test]
    fn strict_limits_are_internally_consistent() {
        let limits = ContainmentLimits::strict();
        assert_eq!(limits, ContainmentLimits::default());
        assert_eq!(limits.active_processes(), 1);
        assert!(limits.validate().is_ok());
        assert!(limits.per_process_cpu_time() <= limits.job_cpu_time());
        assert!(limits.per_process_memory_bytes() <= limits.job_memory_bytes());
    }

    #[test]
    fn impossible_limits_are_refused_before_anything_is_started() {
        assert!(matches!(
            ContainmentLimits::strict()
                .with_job_cpu_time(Duration::ZERO)
                .validate(),
            Err(ContainmentError::ZeroLimit("job_cpu_time"))
        ));
        assert!(matches!(
            ContainmentLimits::strict()
                .with_per_process_cpu_time(Duration::ZERO)
                .validate(),
            Err(ContainmentError::ZeroLimit("per_process_cpu_time"))
        ));
        assert!(matches!(
            ContainmentLimits::strict()
                .with_active_processes(0)
                .validate(),
            Err(ContainmentError::ZeroLimit("active_processes"))
        ));
        assert!(matches!(
            ContainmentLimits::strict()
                .with_job_memory_bytes(0)
                .validate(),
            Err(ContainmentError::ZeroLimit("job_memory_bytes"))
        ));
        assert!(matches!(
            ContainmentLimits::strict()
                .with_per_process_memory_bytes(0)
                .validate(),
            Err(ContainmentError::ZeroLimit("per_process_memory_bytes"))
        ));
        assert!(matches!(
            ContainmentLimits::strict()
                .with_per_process_memory_bytes(usize::MAX)
                .validate(),
            Err(ContainmentError::InconsistentMemoryLimits)
        ));
    }

    #[test]
    fn a_cpu_budget_larger_than_a_job_object_can_express_is_refused() {
        assert!(matches!(
            check_positive_time(Duration::MAX, "job_cpu_time"),
            Err(ContainmentError::LimitOutOfRange("job_cpu_time"))
        ));
        assert!(check_positive_time(Duration::from_secs(1), "job_cpu_time").is_ok());
    }

    #[test]
    fn environment_overrides_are_validated() {
        use std::ffi::OsStr;
        let name = OsStr::new;
        assert!(validate_environment(name("NAME"), name("value")).is_ok());
        assert!(matches!(
            validate_environment(name(""), name("value")),
            Err(ContainmentError::InvalidEnvironmentName)
        ));
        assert!(matches!(
            validate_environment(name("NA=ME"), name("value")),
            Err(ContainmentError::InvalidEnvironmentName)
        ));
        assert!(matches!(
            validate_environment(name("NA\0ME"), name("value")),
            Err(ContainmentError::InvalidEnvironmentName)
        ));
        assert!(matches!(
            validate_environment(name("NAME"), name("val\0ue")),
            Err(ContainmentError::InvalidEnvironmentValue)
        ));
    }

    #[test]
    fn mandatory_levels_map_both_ways() {
        for level in [
            MandatoryIntegrity::Untrusted,
            MandatoryIntegrity::Low,
            MandatoryIntegrity::Medium,
            MandatoryIntegrity::High,
            MandatoryIntegrity::System,
            MandatoryIntegrity::Protected,
        ] {
            assert_eq!(MandatoryIntegrity::from_rid(level.rid()), level);
        }
        assert!(MandatoryIntegrity::Low.is_low());
        assert!(!MandatoryIntegrity::Medium.is_low());
        assert_eq!(
            MandatoryIntegrity::from_rid(7777),
            MandatoryIntegrity::Unrecognized(7777)
        );
    }
}
