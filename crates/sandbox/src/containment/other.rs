//! The non-Windows containment implementation: there is none, and it says so.
//!
//! Nothing in this file enforces anything. It exists so the public API in
//! [`crate::containment`] has one shape on every platform while reporting, honestly, that
//! containment is Windows-only. Every entry point returns
//! [`ContainmentError::UnsupportedPlatform`]; none of them starts a process, touches a security
//! descriptor, or reports success.

use std::path::Path;
use std::time::Duration;

use super::{
    ChildOutcome, ContainmentError, ContainmentLimits, MandatoryIntegrity, RestrictedProcessSpec,
};

/// The handle a contained child would be held in. This platform has none, so nothing constructs it.
#[allow(dead_code)]
pub(crate) struct Guard(());

impl Guard {
    /// Reports that this platform has no OS-level containment implementation.
    pub(crate) fn wait(&mut self) -> Result<ChildOutcome, ContainmentError> {
        Err(ContainmentError::UnsupportedPlatform)
    }

    /// Reports that this platform has no OS-level containment implementation.
    pub(crate) fn wait_for(
        &mut self,
        _timeout: Option<Duration>,
    ) -> Result<Option<ChildOutcome>, ContainmentError> {
        Err(ContainmentError::UnsupportedPlatform)
    }

    /// Reports that this platform has no OS-level containment implementation.
    pub(crate) fn terminate(&mut self) -> Result<(), ContainmentError> {
        Err(ContainmentError::UnsupportedPlatform)
    }

    /// Reports that this platform has no OS-level containment implementation.
    pub(crate) fn is_running(&self) -> Result<bool, ContainmentError> {
        Err(ContainmentError::UnsupportedPlatform)
    }

    /// Reports that this platform has no OS-level containment implementation.
    pub(crate) fn child_process_id(&self) -> Result<u32, ContainmentError> {
        Err(ContainmentError::UnsupportedPlatform)
    }

    /// Reports that this platform has no OS-level containment implementation.
    pub(crate) fn effective_limits(&self) -> Result<ContainmentLimits, ContainmentError> {
        Err(ContainmentError::UnsupportedPlatform)
    }

    /// Reports that this platform has no OS-level containment implementation.
    pub(crate) fn active_process_count(&self) -> Result<u32, ContainmentError> {
        Err(ContainmentError::UnsupportedPlatform)
    }

    /// Reports that this platform has no OS-level containment implementation.
    pub(crate) fn total_process_count(&self) -> Result<u32, ContainmentError> {
        Err(ContainmentError::UnsupportedPlatform)
    }
}

/// Reports that this platform starts no contained process.
pub(crate) fn spawn(
    _spec: &RestrictedProcessSpec,
    _limits: &ContainmentLimits,
) -> Result<Guard, ContainmentError> {
    Err(ContainmentError::UnsupportedPlatform)
}

/// Reports that this platform cannot create a designated sandbox directory.
pub(crate) fn prepare_designated_directory(_path: &Path) -> Result<(), ContainmentError> {
    Err(ContainmentError::UnsupportedPlatform)
}

/// Reports that this platform cannot verify a designated sandbox directory.
pub(crate) fn verify_designated_directory(_path: &Path) -> Result<(), ContainmentError> {
    Err(ContainmentError::UnsupportedPlatform)
}

/// Reports that this platform cannot report a mandatory integrity level.
pub(crate) fn read_integrity(_path: &Path) -> Result<MandatoryIntegrity, ContainmentError> {
    Err(ContainmentError::UnsupportedPlatform)
}

/// Reports that this platform cannot report a process token's mandatory integrity level.
pub(crate) fn process_integrity(_process_id: u32) -> Result<MandatoryIntegrity, ContainmentError> {
    Err(ContainmentError::UnsupportedPlatform)
}

/// Reports that this platform cannot remove a designated sandbox directory through containment.
pub(crate) fn remove_tree(_path: &Path) -> Result<(), ContainmentError> {
    Err(ContainmentError::UnsupportedPlatform)
}

/// Reports that this platform cannot inspect a process the way Windows containment does.
pub(crate) fn is_process_alive(_process_id: u32) -> Result<bool, ContainmentError> {
    Err(ContainmentError::UnsupportedPlatform)
}

/// Reports that this platform cannot read a path's mandatory label.
pub(crate) fn capture_path_label(_path: &Path) -> Result<Option<String>, ContainmentError> {
    Err(ContainmentError::UnsupportedPlatform)
}

/// Reports that this platform cannot restore a path's mandatory label.
pub(crate) fn restore_path_label(
    _path: &Path,
    _original_label: Option<&str>,
) -> Result<(), ContainmentError> {
    Err(ContainmentError::UnsupportedPlatform)
}

/// Reports that this platform cannot refuse a low-integrity reader.
pub(crate) fn deny_low_integrity_reads(_path: &Path) -> Result<(), ContainmentError> {
    Err(ContainmentError::UnsupportedPlatform)
}

/// Reports that this platform cannot inspect whether a path refuses a low-integrity reader.
pub(crate) fn path_denies_low_integrity_reads(_path: &Path) -> Result<bool, ContainmentError> {
    Err(ContainmentError::UnsupportedPlatform)
}
