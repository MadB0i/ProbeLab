//! The Windows containment implementation: job object, low-integrity token, mandatory label.
//!
//! Every unsafe call in this file is a Windows API call whose result is checked. The ordering of
//! the calls is the security property, so it is stated once here: the child is created suspended,
//! assigned to a job that carries `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, given the low mandatory
//! integrity label, and only then resumed. Any failure before the resume terminates the
//! still-suspended child, so a normal token is never resumed.

use std::ffi::{c_void, OsStr, OsString};
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::time::Duration;

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, SetHandleInformation, ERROR_INVALID_PARAMETER,
    GENERIC_READ, GENERIC_WRITE, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SDDL_REVISION_1,
    SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    CreateWellKnownSid, GetAce, GetLengthSid, GetSecurityDescriptorControl,
    GetSecurityDescriptorDacl, GetTokenInformation, IsValidSid, SetFileSecurityW,
    SetTokenInformation, TokenIntegrityLevel, WinLowLabelSid, ACCESS_ALLOWED_ACE, ACL,
    DACL_SECURITY_INFORMATION, LABEL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SID_AND_ATTRIBUTES, TOKEN_ADJUST_DEFAULT, TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_ALWAYS, OPEN_EXISTING,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject, JOBOBJECTINFOCLASS, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_JOB_TIME, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOB_OBJECT_LIMIT_PROCESS_TIME,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, OpenProcess, OpenProcessToken, ResumeThread,
    TerminateProcess, WaitForSingleObject, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, INFINITE,
    PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES, STARTUPINFOW,
};

use super::{
    from_hundred_nanoseconds, ChildOutcome, ContainmentError, ContainmentLimits,
    MandatoryIntegrity, RestrictedProcessSpec, LOW_INTEGRITY_RID, LOW_INTEGRITY_SID,
    SANDBOX_DIRECTORY_SDDL,
};

/// Reads the last Win32 error, which is only meaningful immediately after a failed call.
fn last_error() -> u32 {
    unsafe { GetLastError() }
}

/// The limit flags every contained job carries, checked again when the job is read back.
const REQUIRED_JOB_LIMIT_FLAGS: u32 = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    | JOB_OBJECT_LIMIT_JOB_TIME
    | JOB_OBJECT_LIMIT_PROCESS_TIME
    | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
    | JOB_OBJECT_LIMIT_JOB_MEMORY
    | JOB_OBJECT_LIMIT_PROCESS_MEMORY
    | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;

/// `SE_GROUP_INTEGRITY`, the ACE flag that marks a mandatory integrity label.
const SE_GROUP_INTEGRITY: u32 = 0x0000_0020;

/// `SE_DACL_PROTECTED`, the control bit that marks a DACL as not inheritable from its parent.
const SE_DACL_PROTECTED: u16 = 0x1000;

/// The one ACE type a file SACL may hold, because a mandatory label is a system mandatory label.
const SYSTEM_MANDATORY_LABEL_ACE_TYPE: u8 = 0x11;

/// `SYSTEM_MANDATORY_LABEL_NO_WRITE_UP`, the label mask that refuses a write to a higher level.
const SYSTEM_MANDATORY_LABEL_NO_WRITE_UP: u32 = 0x0000_0001;

/// `SYSTEM_MANDATORY_LABEL_NO_READ_UP`, the label mask that refuses a read from a lower level.
///
/// This is the flag that closes the read gap. `NO_WRITE_UP` alone leaves a low-integrity child free
/// to read a medium-labeled file, because Windows gives ordinary files a medium mandatory label and
/// read-up is permitted unless this bit is set. Adding it to the level a file already has makes the
/// refusal apply to any *lower* integrity caller while leaving an equal or higher one — the trusted
/// process — completely unaffected, because mandatory integrity control only ever refuses access
/// that goes strictly up.
const SYSTEM_MANDATORY_LABEL_NO_READ_UP: u32 = 0x0000_0002;

/// The identifier authority every mandatory integrity label SID belongs to.
const MANDATORY_LABEL_IDENTIFIER_AUTHORITY: &str = "1";

/// The sub authority every mandatory integrity label SID belongs to.
const MANDATORY_LABEL_SUB_AUTHORITY: &str = "16";

/// The exit code ProbeLab uses when it terminates a contained child itself.
///
/// The value is arbitrary and is never a result the child chose, so a caller can always tell it
/// apart from an exit the child produced on its own.
const TERMINATED_EXIT_CODE: u32 = 0x0000_0BAD;

/// A child that is contained and running.
pub(crate) struct Guard {
    process: OwnedHandle,
    job: OwnedHandle,
    process_id: u32,
    limits: ContainmentLimits,
}

/// Starts a child suspended, inside a job, at low integrity, and resumes it last.
pub(crate) fn spawn(
    spec: &RestrictedProcessSpec,
    limits: &ContainmentLimits,
) -> Result<Guard, ContainmentError> {
    let job = create_job(limits)?;
    let mut child = PendingChild::create(spec)?;
    child.assign_to(job.as_raw())?;
    set_low_integrity(child.process_handle())?;
    child.resume()?;
    let process_id = child.process_id();
    Ok(Guard {
        process: child.take_process(),
        job,
        process_id,
        limits: *limits,
    })
}

impl Guard {
    /// Waits for the child to exit and reports its exit code and charged CPU time.
    pub(crate) fn wait(&mut self) -> Result<ChildOutcome, ContainmentError> {
        match self.wait_for(None)? {
            Some(outcome) => Ok(outcome),
            None => Err(ContainmentError::Win32 {
                step: "WaitForSingleObject(INFINITE)",
                code: last_error(),
            }),
        }
    }

    /// Waits up to `timeout` for the child, returning `None` while it is still running.
    pub(crate) fn wait_for(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Option<ChildOutcome>, ContainmentError> {
        let milliseconds = match timeout {
            None => INFINITE,
            Some(timeout) => timeout.as_millis().min(u128::from(u32::MAX)) as u32,
        };
        match unsafe { WaitForSingleObject(self.process.as_raw(), milliseconds) } {
            WAIT_OBJECT_0 => Ok(Some(self.outcome()?)),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(ContainmentError::Win32 {
                step: "WaitForSingleObject",
                code: last_error(),
            }),
        }
    }

    /// Terminates every process in the job, including the child and anything it started.
    pub(crate) fn terminate(&mut self) -> Result<(), ContainmentError> {
        if unsafe { TerminateJobObject(self.job.as_raw(), TERMINATED_EXIT_CODE) } == 0 {
            return Err(ContainmentError::Win32 {
                step: "TerminateJobObject",
                code: last_error(),
            });
        }
        Ok(())
    }

    /// Returns true while the child process has not exited.
    pub(crate) fn is_running(&self) -> Result<bool, ContainmentError> {
        match unsafe { WaitForSingleObject(self.process.as_raw(), 0) } {
            WAIT_TIMEOUT => Ok(true),
            WAIT_OBJECT_0 => Ok(false),
            _ => Err(ContainmentError::Win32 {
                step: "WaitForSingleObject(0)",
                code: last_error(),
            }),
        }
    }

    /// Returns the identifier Windows assigned to the contained child.
    pub(crate) fn child_process_id(&self) -> Result<u32, ContainmentError> {
        Ok(self.process_id)
    }

    /// Reads the limits Windows reports for the job object, not the limits that were requested.
    pub(crate) fn effective_limits(&self) -> Result<ContainmentLimits, ContainmentError> {
        let mut information: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        query_job(
            self.job.as_raw(),
            JobObjectExtendedLimitInformation,
            &mut information as *mut _ as *mut c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )?;
        if information.BasicLimitInformation.LimitFlags & REQUIRED_JOB_LIMIT_FLAGS
            != REQUIRED_JOB_LIMIT_FLAGS
        {
            return Err(ContainmentError::JobLimitsNotApplied);
        }
        Ok(ContainmentLimits {
            job_cpu_time: from_hundred_nanoseconds(
                information.BasicLimitInformation.PerJobUserTimeLimit,
            ),
            per_process_cpu_time: from_hundred_nanoseconds(
                information.BasicLimitInformation.PerProcessUserTimeLimit,
            ),
            job_memory_bytes: information.JobMemoryLimit,
            per_process_memory_bytes: information.ProcessMemoryLimit,
            active_processes: information.BasicLimitInformation.ActiveProcessLimit,
        })
    }

    /// Returns how many processes are active in the job right now.
    pub(crate) fn active_process_count(&self) -> Result<u32, ContainmentError> {
        Ok(self.accounting()?.ActiveProcesses)
    }

    /// Returns how many processes have been assigned to the job since it was created.
    pub(crate) fn total_process_count(&self) -> Result<u32, ContainmentError> {
        Ok(self.accounting()?.TotalProcesses)
    }

    fn accounting(&self) -> Result<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, ContainmentError> {
        let mut information: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
        query_job(
            self.job.as_raw(),
            JobObjectBasicAccountingInformation,
            &mut information as *mut _ as *mut c_void,
            size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
        )?;
        Ok(information)
    }

    fn outcome(&self) -> Result<ChildOutcome, ContainmentError> {
        let mut exit_code = 0u32;
        if unsafe { GetExitCodeProcess(self.process.as_raw(), &mut exit_code) } == 0 {
            return Err(ContainmentError::Win32 {
                step: "GetExitCodeProcess",
                code: last_error(),
            });
        }
        let accounting = self.accounting()?;
        Ok(ChildOutcome {
            exit_code,
            user_cpu_time: from_hundred_nanoseconds(accounting.TotalUserTime),
            kernel_cpu_time: from_hundred_nanoseconds(accounting.TotalKernelTime),
            per_process_cpu_time: self.limits.per_process_cpu_time(),
        })
    }
}

/// Creates the designated sandbox directory and applies the containment security descriptor.
pub(crate) fn prepare_designated_directory(path: &Path) -> Result<(), ContainmentError> {
    let parent = path
        .parent()
        .ok_or(ContainmentError::MissingParentDirectory)?;
    if !parent.as_os_str().is_empty() && read_integrity(parent)?.is_low() {
        return Err(ContainmentError::ParentIntegrityNotHigher(
            parent.display().to_string(),
        ));
    }
    if path.exists() && !path.is_dir() {
        return Err(ContainmentError::NotADirectory);
    }
    std::fs::create_dir_all(path).map_err(|_| ContainmentError::DesignatedDirectoryUnavailable)?;
    apply_designated_security(path)?;
    verify_designated_directory(path)
}

/// Reads a designated directory's security descriptor and checks the containment invariants.
pub(crate) fn verify_designated_directory(path: &Path) -> Result<(), ContainmentError> {
    let security = DirectorySecurity::read(path)?;
    match security.label {
        None => return Err(ContainmentError::LabelMissing),
        Some(label) if label != LOW_INTEGRITY_SID => {
            return Err(ContainmentError::LabelNotVerified {
                expected: LOW_INTEGRITY_SID,
                actual: label,
            })
        }
        Some(_) => {}
    }
    if !security.dacl_present {
        return Err(ContainmentError::DaclMissing);
    }
    if !security.dacl_protected {
        return Err(ContainmentError::DaclNotProtected);
    }
    Ok(())
}

/// Reads the effective mandatory integrity level of a path, defaulting to medium when absent.
///
/// A path with no explicit label is what Windows gives a normal desktop process, so it reports as
/// medium.
pub(crate) fn read_integrity(path: &Path) -> Result<MandatoryIntegrity, ContainmentError> {
    DirectorySecurity::read(path)?
        .label
        .map_or(Ok(MandatoryIntegrity::Medium), label_to_integrity)
}

/// Maps a mandatory level SID such as `S-1-16-4096` to the level it names.
///
/// A mandatory level SID is always `S-1-16-<relative identifier>`, so the relative identifier is
/// the last component. Anything that does not have that shape is refused rather than guessed at.
fn label_to_integrity(label: String) -> Result<MandatoryIntegrity, ContainmentError> {
    let components: Vec<&str> = label.split('-').collect();
    if components.len() != 4
        || components[0] != "S"
        || components[1] != MANDATORY_LABEL_IDENTIFIER_AUTHORITY
        || components[2] != MANDATORY_LABEL_SUB_AUTHORITY
    {
        return Err(ContainmentError::UnreadableMandatoryLabel(label));
    }
    match components[3].parse::<u32>() {
        Ok(relative_identifier) => Ok(MandatoryIntegrity::from_rid(relative_identifier)),
        Err(_) => Err(ContainmentError::UnreadableMandatoryLabel(label)),
    }
}

/// Reads the mandatory integrity level out of a running process's primary token.
pub(crate) fn process_integrity(process_id: u32) -> Result<MandatoryIntegrity, ContainmentError> {
    let process =
        OwnedHandle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) })
            .ok_or(ContainmentError::Win32 {
                step: "OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)",
                code: last_error(),
            })?;
    let mut raw_token: HANDLE = null_mut();
    let opened = unsafe { OpenProcessToken(process.as_raw(), TOKEN_QUERY, &mut raw_token) };
    if opened == 0 {
        return Err(ContainmentError::Win32 {
            step: "OpenProcessToken(TOKEN_QUERY)",
            code: last_error(),
        });
    }
    let token = OwnedHandle::new(raw_token).ok_or(ContainmentError::Win32 {
        step: "OpenProcessToken(token)",
        code: last_error(),
    })?;
    label_to_integrity(token_label_sid(&token)?)
}

/// Reads the SID of a token's mandatory label in its canonical string form.
fn token_label_sid(token: &OwnedHandle) -> Result<String, ContainmentError> {
    let mut required = 0u32;
    unsafe {
        GetTokenInformation(
            token.as_raw(),
            TokenIntegrityLevel,
            null_mut(),
            0,
            &mut required,
        );
    }
    if required == 0 {
        return Err(ContainmentError::TokenIntegrityLevelUnavailable);
    }
    let words = required as usize / size_of::<u64>() + 1;
    let mut storage = vec![0u64; words];
    let read = unsafe {
        GetTokenInformation(
            token.as_raw(),
            TokenIntegrityLevel,
            storage.as_mut_ptr() as *mut c_void,
            (words * size_of::<u64>()) as u32,
            &mut required,
        )
    };
    if read == 0 {
        return Err(ContainmentError::Win32 {
            step: "GetTokenInformation(TokenIntegrityLevel)",
            code: last_error(),
        });
    }
    let reported = unsafe { &*(storage.as_ptr() as *const TOKEN_MANDATORY_LABEL) };
    sid_to_string(reported.Label.Sid)
}

/// Removes a designated directory tree, which a medium-integrity caller may always do.
pub(crate) fn remove_tree(path: &Path) -> Result<(), ContainmentError> {
    remove_tree_inner(path).map_err(|_| ContainmentError::RemovalFailed)
}

/// Returns true while a process with this identifier has not exited.
pub(crate) fn is_process_alive(process_id: u32) -> Result<bool, ContainmentError> {
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, process_id) };
    if handle.is_null() {
        let code = last_error();
        if code == ERROR_INVALID_PARAMETER {
            return Ok(false);
        }
        return Err(ContainmentError::Win32 {
            step: "OpenProcess(PROCESS_SYNCHRONIZE)",
            code,
        });
    }
    let handle = OwnedHandle::new(handle).ok_or(ContainmentError::Win32 {
        step: "OpenProcess(PROCESS_SYNCHRONIZE)",
        code: last_error(),
    })?;
    match unsafe { WaitForSingleObject(handle.as_raw(), 0) } {
        WAIT_OBJECT_0 => Ok(false),
        WAIT_TIMEOUT => Ok(true),
        _ => Err(ContainmentError::Win32 {
            step: "WaitForSingleObject(process)",
            code: last_error(),
        }),
    }
}

/// A child that has not been allowed to run yet, and is terminated if this scope unwinds.
struct PendingChild {
    process: Option<OwnedHandle>,
    thread: OwnedHandle,
    process_id: u32,
    armed: bool,
}

impl PendingChild {
    /// Creates the child suspended, so it has executed no instructions yet.
    fn create(spec: &RestrictedProcessSpec) -> Result<Self, ContainmentError> {
        let mut line = command_line(spec);
        let directory = spec.current_directory().map(wide_path).transpose()?;
        // When a stream is redirected, all three standard handles must be valid and inheritable, and
        // `bInheritHandles` must be set. Standard input is pointed at the null device so a child can
        // never block on, or read from, the operator's console. The redirection handles are dropped
        // as soon as creation returns: the child holds its own copies from that point on.
        let streams = StandardStreams::open(spec)?;
        let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
        startup.cb = size_of::<STARTUPINFOW>() as u32;
        startup.dwFlags = STARTF_USESHOWWINDOW;
        startup.wShowWindow = 0;
        if let Some(streams) = streams.as_ref() {
            // `STARTF_USESTDHANDLES` is set only when handles are actually supplied. Setting the flag
            // with null handles would hand the child invalid standard streams instead of the inherited
            // ones, which silently breaks a child that redirects to a file from inside a script.
            startup.dwFlags |= STARTF_USESTDHANDLES;
            startup.hStdInput = streams.input.as_raw();
            startup.hStdOutput = streams.output.as_raw();
            startup.hStdError = streams.error.as_raw();
        }
        let environment = if spec.inherits_environment() {
            match spec.environment() {
                [] => None,
                overrides => Some(environment_block(overrides, true)?),
            }
        } else {
            // An empty environment block is a single NUL, which is what Windows expects for "no
            // variables at all". Passing `None` here would mean "inherit the parent's block".
            Some(environment_block(spec.environment(), false)?)
        };
        let mut information: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let inherit_handles = i32::from(streams.is_some());
        let created = unsafe {
            CreateProcessW(
                null(),
                line.as_mut_ptr(),
                null(),
                null(),
                inherit_handles,
                CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
                environment
                    .as_ref()
                    .map_or(null(), |block| block.as_ptr() as *const c_void),
                directory.as_ref().map_or(null(), |path| path.as_ptr()),
                &startup,
                &mut information,
            )
        };
        if created == 0 {
            return Err(ContainmentError::Win32 {
                step: "CreateProcessW(CREATE_SUSPENDED)",
                code: last_error(),
            });
        }
        let process = OwnedHandle::new(information.hProcess);
        let thread = OwnedHandle::new(information.hThread);
        if process.is_none() || thread.is_none() {
            if let Some(process) = process.as_ref() {
                unsafe {
                    TerminateProcess(process.as_raw(), TERMINATED_EXIT_CODE);
                }
            }
            return Err(ContainmentError::Win32 {
                step: "CreateProcessW(process handles)",
                code: last_error(),
            });
        }
        Ok(Self {
            process,
            thread: thread.expect("the thread handle was just checked"),
            process_id: information.dwProcessId,
            armed: true,
        })
    }

    fn process_handle(&self) -> HANDLE {
        self.process
            .as_ref()
            .map_or(null_mut(), OwnedHandle::as_raw)
    }

    /// Assigns the still-suspended child to the job, before it can execute anything.
    fn assign_to(&self, job: HANDLE) -> Result<(), ContainmentError> {
        if unsafe { AssignProcessToJobObject(job, self.process_handle()) } == 0 {
            return Err(ContainmentError::Win32 {
                step: "AssignProcessToJobObject",
                code: last_error(),
            });
        }
        Ok(())
    }

    /// Resumes the primary thread, the first moment the child can execute anything.
    fn resume(&mut self) -> Result<(), ContainmentError> {
        if unsafe { ResumeThread(self.thread.as_raw()) } == u32::MAX {
            return Err(ContainmentError::Win32 {
                step: "ResumeThread",
                code: last_error(),
            });
        }
        self.armed = false;
        Ok(())
    }

    fn take_process(&mut self) -> OwnedHandle {
        self.process
            .take()
            .expect("a pending child always owns its process handle")
    }

    fn process_id(&self) -> u32 {
        self.process_id
    }
}

impl Drop for PendingChild {
    fn drop(&mut self) {
        if self.armed {
            unsafe {
                TerminateProcess(self.process_handle(), TERMINATED_EXIT_CODE);
            }
        }
    }
}

/// Creates an unnamed job object carrying every resource limit the guard promises.
fn create_job(limits: &ContainmentLimits) -> Result<OwnedHandle, ContainmentError> {
    let handle = unsafe { CreateJobObjectW(null(), null()) };
    let job = OwnedHandle::new(handle).ok_or(ContainmentError::Win32 {
        step: "CreateJobObjectW",
        code: last_error(),
    })?;
    let mut information: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    information.BasicLimitInformation.PerJobUserTimeLimit =
        to_hundred_nanoseconds(limits.job_cpu_time());
    information.BasicLimitInformation.PerProcessUserTimeLimit =
        to_hundred_nanoseconds(limits.per_process_cpu_time());
    information.BasicLimitInformation.ActiveProcessLimit = limits.active_processes();
    information.JobMemoryLimit = limits.job_memory_bytes();
    information.ProcessMemoryLimit = limits.per_process_memory_bytes();
    information.BasicLimitInformation.LimitFlags = REQUIRED_JOB_LIMIT_FLAGS;
    let applied = unsafe {
        SetInformationJobObject(
            job.as_raw(),
            JobObjectExtendedLimitInformation,
            &information as *const _ as *const c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if applied == 0 {
        return Err(ContainmentError::Win32 {
            step: "SetInformationJobObject(JobObjectExtendedLimitInformation)",
            code: last_error(),
        });
    }
    Ok(job)
}

/// Rewrites the child's primary token to low integrity, then reads it back to prove it stuck.
fn set_low_integrity(process: HANDLE) -> Result<(), ContainmentError> {
    let low = LowIntegritySid::new()?;
    let mut raw_token: HANDLE = null_mut();
    let opened =
        unsafe { OpenProcessToken(process, TOKEN_ADJUST_DEFAULT | TOKEN_QUERY, &mut raw_token) };
    if opened == 0 {
        return Err(ContainmentError::Win32 {
            step: "OpenProcessToken(TOKEN_ADJUST_DEFAULT|TOKEN_QUERY)",
            code: last_error(),
        });
    }
    let token = OwnedHandle::new(raw_token).ok_or(ContainmentError::Win32 {
        step: "OpenProcessToken(token)",
        code: last_error(),
    })?;
    let label = TOKEN_MANDATORY_LABEL {
        Label: SID_AND_ATTRIBUTES {
            Sid: low.as_raw(),
            Attributes: SE_GROUP_INTEGRITY,
        },
    };
    let applied = unsafe {
        SetTokenInformation(
            token.as_raw(),
            TokenIntegrityLevel,
            &label as *const _ as *const c_void,
            size_of::<TOKEN_MANDATORY_LABEL>() as u32,
        )
    };
    if applied == 0 {
        return Err(ContainmentError::Win32 {
            step: "SetTokenInformation(TokenIntegrityLevel)",
            code: last_error(),
        });
    }
    if !token_is_low(&token)? {
        return Err(ContainmentError::IntegrityLevelNotApplied);
    }
    Ok(())
}

/// Reads the token's mandatory label back and reports whether it is the low integrity level.
fn token_is_low(token: &OwnedHandle) -> Result<bool, ContainmentError> {
    Ok(token_label_sid(token)? == LOW_INTEGRITY_SID)
}

/// The low mandatory integrity SID, held in storage that outlives every use of the pointer.
struct LowIntegritySid {
    storage: Vec<u64>,
}

impl LowIntegritySid {
    /// Builds the well-known low integrity SID and proves it is exactly `S-1-16-4096`.
    fn new() -> Result<Self, ContainmentError> {
        let mut length = 0u32;
        unsafe {
            CreateWellKnownSid(WinLowLabelSid, null_mut(), null_mut(), &mut length);
        }
        if length == 0 {
            return Err(ContainmentError::Win32 {
                step: "CreateWellKnownSid(WinLowLabelSid) size",
                code: last_error(),
            });
        }
        let words = length as usize / size_of::<u64>() + 1;
        let mut storage = vec![0u64; words];
        let created = unsafe {
            CreateWellKnownSid(
                WinLowLabelSid,
                null_mut(),
                storage.as_mut_ptr() as PSID,
                &mut length,
            )
        };
        if created == 0 {
            return Err(ContainmentError::Win32 {
                step: "CreateWellKnownSid(WinLowLabelSid)",
                code: last_error(),
            });
        }
        let sid = storage.as_ptr() as PSID;
        if unsafe { IsValidSid(sid) } == 0 {
            return Err(ContainmentError::Win32 {
                step: "IsValidSid(WinLowLabelSid)",
                code: last_error(),
            });
        }
        if unsafe { GetLengthSid(sid) } == 0 {
            return Err(ContainmentError::Win32 {
                step: "GetLengthSid(WinLowLabelSid)",
                code: last_error(),
            });
        }
        let text = sid_to_string(sid)?;
        if text != LOW_INTEGRITY_SID {
            return Err(ContainmentError::LowIntegritySidMismatch(text));
        }
        Ok(Self { storage })
    }

    fn as_raw(&self) -> PSID {
        self.storage.as_ptr() as PSID
    }
}

/// The parts of a file security descriptor that containment depends on.
struct DirectorySecurity {
    label: Option<String>,
    dacl_present: bool,
    dacl_protected: bool,
}

impl DirectorySecurity {
    /// Reads the DACL, its protection bit, and the mandatory label of one filesystem path.
    fn read(path: &Path) -> Result<Self, ContainmentError> {
        let wide = wide_path(path)?;
        let mut owner: PSID = null_mut();
        let mut group: PSID = null_mut();
        let mut dacl: *mut ACL = null_mut();
        let mut sacl: *mut ACL = null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        let code = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION,
                &mut owner,
                &mut group,
                &mut dacl,
                &mut sacl,
                &mut descriptor,
            )
        };
        if code != 0 {
            return Err(ContainmentError::Win32 {
                step: "GetNamedSecurityInfoW",
                code,
            });
        }
        let _descriptor = LocalSecurityDescriptor(descriptor);

        let mut present: BOOL = 0;
        let mut defaulted: BOOL = 0;
        let mut dacl_pointer: *mut ACL = null_mut();
        let described = unsafe {
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl_pointer, &mut defaulted)
        };
        if described == 0 {
            return Err(ContainmentError::Win32 {
                step: "GetSecurityDescriptorDacl",
                code: last_error(),
            });
        }
        let mut control: u16 = 0;
        let mut revision: u32 = 0;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
            return Err(ContainmentError::Win32 {
                step: "GetSecurityDescriptorControl",
                code: last_error(),
            });
        }
        let label = if sacl.is_null() {
            None
        } else {
            Some(mandatory_label_sid(sacl)?)
        };
        Ok(Self {
            label,
            dacl_present: present != 0 && !dacl_pointer.is_null(),
            dacl_protected: control & SE_DACL_PROTECTED != 0,
        })
    }
}

/// Reads the SID of the single mandatory label ACE a file SACL is allowed to hold.
///
/// A system mandatory label ACE carries its access mask and then its SID, exactly like an access
/// ACE, which is why the label is read through the access ACE layout. Verification is strict: the
/// ACE must be a system mandatory label, it must refuse write-up, and its SID must be the low
/// integrity level. Anything else means the object is not labelled the way containment requires.
fn mandatory_label_sid(sacl: *const ACL) -> Result<String, ContainmentError> {
    let mut ace: *mut c_void = null_mut();
    if unsafe { GetAce(sacl, 0, &mut ace) } == 0 {
        return Err(ContainmentError::Win32 {
            step: "GetAce(mandatory label)",
            code: last_error(),
        });
    }
    let entry = unsafe { &*(ace as *const ACCESS_ALLOWED_ACE) };
    if entry.Header.AceType != SYSTEM_MANDATORY_LABEL_ACE_TYPE
        || entry.Mask & SYSTEM_MANDATORY_LABEL_NO_WRITE_UP == 0
    {
        return Err(ContainmentError::NotAMandatoryLabel);
    }
    sid_to_string(&entry.SidStart as *const u32 as PSID)
}

/// Applies the designated directory DACL and then its low mandatory label.
///
/// The DACL is written first so the label is written against an ACL that already grants its owner
/// the access setting a mandatory label requires.
fn apply_designated_security(path: &Path) -> Result<(), ContainmentError> {
    let wide = wide_path(path)?;
    let sddl = wide_string(OsStr::new(SANDBOX_DIRECTORY_SDDL));
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let mut length = 0u32;
    let parsed = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            &mut length,
        )
    };
    if parsed == 0 {
        return Err(ContainmentError::Win32 {
            step: "ConvertStringSecurityDescriptorToSecurityDescriptorW",
            code: last_error(),
        });
    }
    let descriptor = LocalSecurityDescriptor(descriptor);
    for (information, step) in [
        (
            DACL_SECURITY_INFORMATION,
            "SetFileSecurityW(DACL_SECURITY_INFORMATION)",
        ),
        (
            LABEL_SECURITY_INFORMATION,
            "SetFileSecurityW(LABEL_SECURITY_INFORMATION)",
        ),
    ] {
        if unsafe { SetFileSecurityW(wide.as_ptr(), information, descriptor.as_raw()) } == 0 {
            return Err(ContainmentError::Win32 {
                step,
                code: last_error(),
            });
        }
    }
    Ok(())
}

/// Reads one path's mandatory label as an SDDL string, so it can be restored byte for byte later.
///
/// Only the label is captured. The DACL, the owner, and the group are never touched, so protecting
/// a path does not change who may reach it as the trusted user.
pub(crate) fn capture_path_label(path: &Path) -> Result<Option<String>, ContainmentError> {
    let descriptor = read_label_descriptor(path)?;

    let mut text: *mut u16 = null_mut();
    let mut length = 0u32;
    let converted = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor.as_raw(),
            SDDL_REVISION_1,
            LABEL_SECURITY_INFORMATION,
            &mut text,
            &mut length,
        )
    };
    if converted == 0 {
        return Err(ContainmentError::Win32 {
            step: "ConvertSecurityDescriptorToStringSecurityDescriptorW(label)",
            code: last_error(),
        });
    }
    if text.is_null() {
        // No explicit label exists, which is the normal state for a freshly created file. Windows
        // treats the absence as the medium level, and that is what `effective_label` reports.
        return Ok(None);
    }
    let text = LocalWideString(text);
    let mut units = 0usize;
    while unsafe { *text.0.add(units) } != 0 {
        units += 1;
    }
    let sddl = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text.0, units) });
    if sddl.is_empty() {
        return Ok(None);
    }
    Ok(Some(sddl))
}

/// Restores a path's mandatory label from an SDDL string captured by [`capture_path_label`].
pub(crate) fn restore_path_label(
    path: &Path,
    original_label: Option<&str>,
) -> Result<(), ContainmentError> {
    // A path that had no explicit label is restored to the medium label Windows itself writes for an
    // ordinary object. That is semantically the state it was in: same level, same no-write-up flag,
    // and no read-up refusal. There is no portable way to delete a mandatory label outright, and
    // leaving our read-up flag in place would be the one outcome that is definitely wrong.
    let original_label = original_label.unwrap_or("S:(ML;;NW;;;S-1-16-8192)");

    let sddl = wide_string(OsStr::new(original_label));
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let mut length = 0u32;
    let parsed = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            &mut length,
        )
    };
    if parsed == 0 {
        return Err(ContainmentError::Win32 {
            step: "ConvertStringSecurityDescriptorToSecurityDescriptorW(label restore)",
            code: last_error(),
        });
    }
    let descriptor = LocalSecurityDescriptor(descriptor);
    set_label(path, &descriptor)
}

/// Refuses a lower-integrity reader by adding the read-up flag to a path's existing label.
///
/// The integrity level itself is never changed. Windows already labels an ordinary file at medium,
/// and this keeps that level while adding `NO_READ_UP`, so a low-integrity child is refused while the
/// trusted process, which runs at the same level, is not affected at all. Nothing else about the
/// object changes: the DACL, the owner, the group, and the level all stay as they were.
pub(crate) fn deny_low_integrity_reads(path: &Path) -> Result<(), ContainmentError> {
    let (level, mask) = effective_label(path)?;
    if level <= LOW_INTEGRITY_RID {
        return Err(ContainmentError::Win32 {
            step: "protected path is not above the low integrity level",
            code: ERROR_INVALID_PARAMETER,
        });
    }
    if mask & SYSTEM_MANDATORY_LABEL_NO_READ_UP != 0 {
        return Ok(());
    }
    let widened = format!("S:(ML;;NRNW;;;S-1-16-{level})");
    let sddl = wide_string(OsStr::new(&widened));
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let mut parsed_length = 0u32;
    let parsed = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            &mut parsed_length,
        )
    };
    if parsed == 0 {
        return Err(ContainmentError::Win32 {
            step: "ConvertStringSecurityDescriptorToSecurityDescriptorW(label deny)",
            code: last_error(),
        });
    }
    let descriptor = LocalSecurityDescriptor(descriptor);
    set_label(path, &descriptor)
}

/// Returns true when a path's label refuses a read from any lower integrity level.
pub(crate) fn path_denies_low_integrity_reads(path: &Path) -> Result<bool, ContainmentError> {
    let (level, mask) = effective_label(path)?;
    Ok(level > LOW_INTEGRITY_RID && mask & SYSTEM_MANDATORY_LABEL_NO_READ_UP != 0)
}

/// Returns a path's effective integrity level and label mask.
///
/// A path with no explicit label reports the medium level with the no-write-up flag, which is exactly
/// how Windows treats such a path for access checks.
fn effective_label(path: &Path) -> Result<(u32, u32), ContainmentError> {
    match capture_path_label(path)? {
        Some(label) => parse_mandatory_label(&label),
        None => Ok((8192, SYSTEM_MANDATORY_LABEL_NO_WRITE_UP)),
    }
}

/// Splits a mandatory label SDDL section into its level and its mask.
///
/// A mandatory label ACE is written as `ML;<inheritance flags>;<access flags>;;<level>`, and Windows
/// is not consistent about which field the access flags land in: the label this module writes puts
/// them in the third field while Windows itself puts them in the second. Rather than depend on that,
/// every field between the type and the level is scanned for mask letters and the *last* non-empty
/// field is taken as the level. Anything that is not a system mandatory label is refused instead of
/// being guessed at, because guessing here would silently apply the wrong protection.
fn parse_mandatory_label(label_sddl: &str) -> Result<(u32, u32), ContainmentError> {
    let unreadable = || ContainmentError::Win32 {
        step: "mandatory label SDDL could not be parsed",
        code: ERROR_INVALID_PARAMETER,
    };
    let rest = label_sddl
        .strip_prefix("S:")
        .or_else(|| label_sddl.strip_prefix("S:AI"))
        .ok_or_else(unreadable)?;
    let open = rest.find('(').ok_or_else(unreadable)?;
    let close = rest.rfind(')').ok_or_else(unreadable)?;
    if close <= open {
        return Err(unreadable());
    }
    let fields: Vec<&str> = rest[open + 1..close].split(';').collect();
    if fields.first() != Some(&"ML") {
        return Err(ContainmentError::NotAMandatoryLabel);
    }
    let level_field = fields
        .iter()
        .rev()
        .find(|field| !field.is_empty())
        .copied()
        .ok_or_else(unreadable)?;
    let mut mask = 0u32;
    for field in &fields[1..fields.len().saturating_sub(1)] {
        for flag in field.chars() {
            match flag {
                'N' | 'I' | 'O' | 'C' | 'L' => {}
                'W' => mask |= SYSTEM_MANDATORY_LABEL_NO_WRITE_UP,
                'R' => mask |= SYSTEM_MANDATORY_LABEL_NO_READ_UP,
                _ => return Err(unreadable()),
            }
        }
    }
    let level = match level_field {
        // Windows writes the well-known aliases; this module writes the explicit SID form.
        "LU" => 0,
        "LO" | "LW" => LOW_INTEGRITY_RID,
        "ME" | "MD" => 8192,
        "HI" | "HD" => 12288,
        "SY" => 16384,
        "PP" => 16385,
        explicit => explicit
            .strip_prefix("S-1-16-")
            .ok_or_else(unreadable)?
            .parse()
            .map_err(|_| unreadable())?,
    };
    Ok((level, mask))
}

/// Reads a path's mandatory label as a writable security descriptor.
///
/// Only the label is requested, so the DACL, owner, and group are never even read and therefore
/// cannot be rewritten by accident.
fn read_label_descriptor(path: &Path) -> Result<LocalSecurityDescriptor, ContainmentError> {
    let wide = wide_path(path)?;
    let mut owner: PSID = null_mut();
    let mut group: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut sacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let code = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            LABEL_SECURITY_INFORMATION,
            &mut owner,
            &mut group,
            &mut dacl,
            &mut sacl,
            &mut descriptor,
        )
    };
    if code != 0 {
        return Err(ContainmentError::Win32 {
            step: "GetNamedSecurityInfoW(label)",
            code,
        });
    }
    if descriptor.is_null() {
        return Err(ContainmentError::Win32 {
            step: "GetNamedSecurityInfoW(label)",
            code: ERROR_INVALID_PARAMETER,
        });
    }
    Ok(LocalSecurityDescriptor(descriptor))
}

/// Writes only the mandatory label of a parsed security descriptor onto a path.
fn set_label(path: &Path, descriptor: &LocalSecurityDescriptor) -> Result<(), ContainmentError> {
    let wide = wide_path(path)?;
    let code = unsafe {
        SetFileSecurityW(
            wide.as_ptr(),
            LABEL_SECURITY_INFORMATION,
            descriptor.as_raw(),
        )
    };
    if code == 0 {
        return Err(ContainmentError::Win32 {
            step: "SetFileSecurityW(label)",
            code: last_error(),
        });
    }
    Ok(())
}
/// The three standard handles a contained child is given when its output is redirected.
///
/// Standard input is the null device rather than the host's console, so a child can neither read the
/// operator's terminal nor block waiting for input that will never arrive. Output and error are
/// write-only file handles inside the designated sandbox directory.
struct StandardStreams {
    input: OwnedHandle,
    output: OwnedHandle,
    error: OwnedHandle,
}

impl StandardStreams {
    /// Opens the declared capture files, or returns `None` when no stream is redirected.
    ///
    /// Both handles are marked inheritable, because `CreateProcessW` only passes a handle to a child
    /// when the handle itself is inheritable and `bInheritHandles` is set. Handle inheritance is only
    /// enabled at all when a stream is redirected, so a spec that captures nothing behaves exactly as
    /// it did before redirection existed.
    fn open(spec: &RestrictedProcessSpec) -> Result<Option<Self>, ContainmentError> {
        if !spec.redirects_output() {
            return Ok(None);
        }
        let input = Self::open_file("NUL", OPEN_EXISTING, GENERIC_READ)?;
        let output = match spec.stdout_path() {
            Some(path) => Self::open_file(
                &path.as_os_str().to_string_lossy(),
                CREATE_ALWAYS,
                GENERIC_WRITE,
            )?,
            // A stream with no declared destination is discarded rather than inherited.
            None => Self::open_file("NUL", OPEN_ALWAYS, GENERIC_WRITE)?,
        };
        let error = match spec.stderr_path() {
            Some(path) => Self::open_file(
                &path.as_os_str().to_string_lossy(),
                CREATE_ALWAYS,
                GENERIC_WRITE,
            )?,
            None => Self::open_file("NUL", OPEN_ALWAYS, GENERIC_WRITE)?,
        };
        Ok(Some(Self {
            input,
            output,
            error,
        }))
    }

    /// Opens one device or file and marks the handle inheritable.
    fn open_file(name: &str, creation: u32, access: u32) -> Result<OwnedHandle, ContainmentError> {
        let wide = wide_string(OsStr::new(name));
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                creation,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return Err(ContainmentError::Win32 {
                step: "CreateFileW(standard stream)",
                code: last_error(),
            });
        }
        let handle = OwnedHandle::new(handle).ok_or(ContainmentError::Win32 {
            step: "CreateFileW(standard stream)",
            code: ERROR_INVALID_PARAMETER,
        })?;
        if unsafe {
            SetHandleInformation(handle.as_raw(), HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT)
        } == 0
        {
            return Err(ContainmentError::Win32 {
                step: "SetHandleInformation(inherit)",
                code: last_error(),
            });
        }
        Ok(handle)
    }
}

/// A handle that closes itself, so no error path can leak one.
struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE) -> Option<Self> {
        (!handle.is_null()).then_some(Self(handle))
    }

    fn as_raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// A security descriptor Windows allocated, freed exactly once.
struct LocalSecurityDescriptor(PSECURITY_DESCRIPTOR);

impl LocalSecurityDescriptor {
    fn as_raw(&self) -> PSECURITY_DESCRIPTOR {
        self.0
    }
}

impl Drop for LocalSecurityDescriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

/// Builds the command line, quoting every token the way the standard startup parser expects.
fn command_line(spec: &RestrictedProcessSpec) -> Vec<u16> {
    let mut line = quote_argument(spec.program());
    for argument in spec.args() {
        line.push(" ");
        line.push(quote_argument(argument));
    }
    wide_string(&line)
}

/// Quotes one argument the way the standard C startup parser reverses it.
///
/// A run of `n` backslashes is written as `n` backslashes, except in front of a quote, where the
/// quote is a literal character and therefore needs `2n + 1` backslashes to survive the parse. A
/// trailing run of backslashes is never doubled, because nothing follows it that could be escaped.
fn quote_argument(argument: &OsStr) -> OsString {
    let characters: Vec<u16> = argument.encode_wide().collect();
    let needs_quotes = characters.is_empty()
        || characters
            .iter()
            .any(|unit| matches!(*unit, 0x20 | 0x09 | 0x22));
    let mut quoted: Vec<u16> = Vec::with_capacity(characters.len() + 2);
    if needs_quotes {
        quoted.push(0x22);
    }
    let mut backslashes = 0usize;
    for character in characters {
        if character == 0x5c {
            backslashes += 1;
            continue;
        }
        if character == 0x22 {
            quoted.extend(std::iter::repeat_n(0x5c, backslashes * 2 + 1));
        } else {
            quoted.extend(std::iter::repeat_n(0x5c, backslashes));
        }
        backslashes = 0;
        quoted.push(character);
    }
    quoted.extend(std::iter::repeat_n(0x5c, backslashes));
    if needs_quotes {
        quoted.push(0x22);
    }
    OsString::from_wide(&quoted)
}

/// Builds the NUL-terminated, sorted environment block Windows expects for a child process.
fn environment_block(
    overrides: &[(OsString, OsString)],
    inherit: bool,
) -> Result<Vec<u16>, ContainmentError> {
    let mut entries: Vec<(OsString, OsString)> = if inherit {
        std::env::vars_os().collect()
    } else {
        Vec::new()
    };
    for (key, value) in overrides {
        match entries.iter_mut().find(|(existing, _)| existing == key) {
            Some(slot) => slot.1 = value.clone(),
            None => entries.push((key.clone(), value.clone())),
        }
    }
    entries.sort_by_key(|entry| environment_order(&entry.0));
    let mut block: Vec<u16> = Vec::new();
    for (key, value) in entries {
        if key.encode_wide().any(|unit| unit == 0) {
            return Err(ContainmentError::InvalidEnvironmentName);
        }
        if value.encode_wide().any(|unit| unit == 0) {
            return Err(ContainmentError::InvalidEnvironmentValue);
        }
        block.extend(key.encode_wide());
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(0);
    }
    // A block is terminated by one NUL after the last entry, and the whole block by a second. With no
    // entries at all that means two NULs, and `CreateProcessW` rejects a bare single NUL with
    // `ERROR_INVALID_PARAMETER`, so an empty environment is spelled out explicitly here.
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    Ok(block)
}

/// Sorts a hidden per-drive variable first, then case-insensitively, as Windows requires.
fn environment_order(name: &OsStr) -> (u8, String) {
    let text = name.to_string_lossy().into_owned();
    let hidden = u8::from(text.starts_with('='));
    (hidden, text.to_lowercase())
}

fn query_job(
    job: HANDLE,
    class: JOBOBJECTINFOCLASS,
    buffer: *mut c_void,
    length: u32,
) -> Result<(), ContainmentError> {
    let queried = unsafe { QueryInformationJobObject(job, class, buffer, length, null_mut()) };
    if queried == 0 {
        return Err(ContainmentError::Win32 {
            step: "QueryInformationJobObject",
            code: last_error(),
        });
    }
    Ok(())
}

fn sid_to_string(sid: PSID) -> Result<String, ContainmentError> {
    let mut text: *mut u16 = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(ContainmentError::Win32 {
            step: "ConvertSidToStringSidW",
            code: last_error(),
        });
    }
    let _text = LocalWideString(text);
    let mut length = 0usize;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    Ok(unsafe { String::from_utf16_lossy(std::slice::from_raw_parts(text, length)) })
}

/// A wide string Windows allocated, freed exactly once.
struct LocalWideString(*mut u16);

impl Drop for LocalWideString {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0.cast());
            }
        }
    }
}

fn to_hundred_nanoseconds(value: Duration) -> i64 {
    i64::try_from(value.as_nanos() / 100).unwrap_or(i64::MAX)
}

fn wide_path(path: &Path) -> Result<Vec<u16>, ContainmentError> {
    if path.as_os_str().is_empty() {
        return Err(ContainmentError::DesignatedDirectoryUnavailable);
    }
    Ok(wide_string(path.as_os_str()))
}

fn wide_string(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

/// Removes a directory tree, tolerating entries that do not exist.
fn remove_tree_inner(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.is_dir() {
        return std::fs::remove_file(path);
    }
    for entry in std::fs::read_dir(path)? {
        remove_tree_inner(&entry?.path())?;
    }
    std::fs::remove_dir(path)
}

#[cfg(test)]
mod tests {
    use super::{parse_mandatory_label, quote_argument, LowIntegritySid, MandatoryIntegrity};
    use std::ffi::OsStr;

    #[test]
    fn a_mandatory_label_is_parsed_into_its_level_and_mask() {
        assert_eq!(
            parse_mandatory_label("S:(ML;;NW;;;S-1-16-8192)").expect("medium label"),
            (8192, super::SYSTEM_MANDATORY_LABEL_NO_WRITE_UP)
        );
        assert_eq!(
            parse_mandatory_label("S:(ML;;NRNW;;;S-1-16-8192)").expect("read refusal label"),
            (
                8192,
                super::SYSTEM_MANDATORY_LABEL_NO_READ_UP
                    | super::SYSTEM_MANDATORY_LABEL_NO_WRITE_UP
            )
        );
        assert_eq!(
            parse_mandatory_label("S:(ML;OICI;NW;;;S-1-16-4096)").expect("low label"),
            (4096, super::SYSTEM_MANDATORY_LABEL_NO_WRITE_UP)
        );
    }

    #[test]
    fn the_well_known_level_aliases_windows_writes_are_understood() {
        for (alias, level) in [
            ("LU", 0u32),
            ("LO", 4096),
            ("ME", 8192),
            ("HI", 12288),
            ("SY", 16384),
        ] {
            let label = format!("S:(ML;;NWNR;;;{alias})");
            assert_eq!(
                parse_mandatory_label(&label).expect("alias label"),
                (
                    level,
                    super::SYSTEM_MANDATORY_LABEL_NO_WRITE_UP
                        | super::SYSTEM_MANDATORY_LABEL_NO_READ_UP
                ),
                "{label} must be understood"
            );
        }
    }

    #[test]
    fn a_label_that_is_not_a_mandatory_label_is_refused_rather_than_guessed_at() {
        assert!(matches!(
            parse_mandatory_label("S:(AU;;FA;;;SY)"),
            Err(super::ContainmentError::NotAMandatoryLabel)
        ));
        for malformed in [
            "D:P(A;;FA;;;SY)",
            "S:(ML;;NRNW;;;8192)",
            "S:(ML;;NRNW;;;not-a-level)",
            "S:",
        ] {
            assert!(
                parse_mandatory_label(malformed).is_err(),
                "{malformed} must be refused"
            );
        }
    }
    fn quoted(value: &str) -> String {
        quote_argument(OsStr::new(value))
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn plain_arguments_are_not_quoted() {
        assert_eq!(quoted("cmd.exe"), "cmd.exe");
        assert_eq!(quoted("a/b\\c"), "a/b\\c");
    }

    #[test]
    fn arguments_with_spaces_or_nothing_at_all_are_quoted() {
        assert_eq!(quoted("two words"), "\"two words\"");
        assert_eq!(quoted(""), "\"\"");
        assert_eq!(quoted("tab\there"), "\"tab\there\"");
        assert_eq!(
            quoted("C:\\Program Files\\cmd.exe"),
            "\"C:\\Program Files\\cmd.exe\""
        );
    }

    #[test]
    fn a_quote_is_escaped_and_a_trailing_backslash_is_not() {
        assert_eq!(quoted("with\"quote"), "\"with\\\"quote\"");
        assert_eq!(quoted("a\\\"b"), "\"a\\\\\\\"b\"");
        assert_eq!(quoted("trailing\\"), "trailing\\");
        assert_eq!(quoted("trailing\\\\"), "trailing\\\\");
        assert_eq!(quoted("trailing\\ "), "\"trailing\\ \"");
    }

    #[test]
    fn wide_strings_are_nul_terminated() {
        let wide = super::wide_string(OsStr::new("abc"));
        assert_eq!(wide.len(), 4);
        assert_eq!(wide[3], 0);
    }

    #[test]
    fn the_well_known_low_integrity_sid_is_the_one_the_module_promises() {
        let sid = LowIntegritySid::new().expect("low integrity SID");
        assert_eq!(super::sid_to_string(sid.as_raw()).unwrap(), "S-1-16-4096");
        assert!(MandatoryIntegrity::Low.is_low());
    }
}
