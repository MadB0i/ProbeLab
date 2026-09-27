//! The non-Windows AppContainer implementation: there is none, and it says so.
//!
//! AppContainer is a Windows mechanism with no cross-platform equivalent. Nothing in this file
//! enforces anything, and — importantly — nothing in it *appears* to. Every entry point returns
//! [`AppContainerError::UnsupportedPlatform`], so a spawn that asked for an AppContainer identity on
//! Linux or macOS fails rather than quietly launching unconfined. That is the whole point of this
//! module existing: the fail-closed behaviour has to be the same shape on every platform.

use std::ffi::c_void;

/// A derived AppContainer package identity. This platform has none.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppContainerIdentity {
    text: String,
}

impl AppContainerIdentity {
    /// Reports that this platform cannot derive an AppContainer SID.
    pub fn derive(_profile_name: &str) -> Result<Self, AppContainerError> {
        Err(AppContainerError::UnsupportedPlatform)
    }

    /// Returns the SID text. Unreachable on this platform.
    pub fn sid_text(&self) -> &str {
        &self.text
    }

    /// Returns the SID bytes. Unreachable on this platform.
    pub fn sid_bytes(&self) -> &[u8] {
        &[]
    }
}

/// Turns a capability name into a SID. This platform has no SIDs to resolve.
pub fn capability_sid(_name: &str) -> Result<AppContainerIdentity, AppContainerError> {
    Err(AppContainerError::UnsupportedPlatform)
}

/// A request to run a child inside a derived AppContainer.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppContainerRequest {
    /// Capability SIDs to grant. Empty means no capabilities.
    pub capabilities: Vec<String>,
}

/// The process attribute list carrying the AppContainer capability.
///
/// On this platform it cannot be built at all: both constructors return
/// [`AppContainerError::UnsupportedPlatform`], so a spawn that asked for an AppContainer identity
/// fails instead of quietly running unconfined. The accessors exist only so the call sites read the
/// same on every platform; none of them is reachable here.
pub struct SecurityAttributeList;

impl SecurityAttributeList {
    /// Reports that this platform has no AppContainer support.
    pub fn for_identity(_identity: &AppContainerIdentity) -> Result<Self, AppContainerError> {
        Err(AppContainerError::UnsupportedPlatform)
    }

    /// Reports that this platform has no AppContainer support.
    pub fn with_capabilities(
        _identity: &AppContainerIdentity,
        _capabilities: Vec<AppContainerIdentity>,
    ) -> Result<Self, AppContainerError> {
        Err(AppContainerError::UnsupportedPlatform)
    }

    /// Always null here.
    pub fn pointer(&self) -> *mut c_void {
        std::ptr::null_mut()
    }

    /// Always false here: no identity is ever in force on this platform.
    pub fn is_present(&self) -> bool {
        false
    }

    /// Always empty here.
    pub fn granted_capabilities(&self) -> Vec<String> {
        Vec::new()
    }

    /// Always zero here, because no capability can be granted on a platform with no AppContainer.
    pub fn granted_capability_count(&self) -> u32 {
        0
    }
}

/// A typed AppContainer failure.
#[derive(Debug, thiserror::Error)]
pub enum AppContainerError {
    /// This platform has no AppContainer support.
    #[error("AppContainer isolation is not available on this platform")]
    UnsupportedPlatform,
    /// The profile name could not be turned into a SID.
    #[error("the AppContainer SID could not be derived (HRESULT 0x{status:08X})")]
    DeriveFailed {
        /// The raw `HRESULT` from the derive call.
        status: u32,
    },
    /// The derived SID could not be rendered as text.
    #[error("the derived AppContainer SID could not be rendered: {0}")]
    SidNotRenderable(String),
    /// A capability name was not a valid security identifier.
    #[error("AppContainer capability {name:?} is not a valid SID")]
    CapabilityNotASid {
        /// The rejected capability name.
        name: String,
    },
    /// Capabilities were requested without an identity to attach them to.
    #[error("AppContainer capabilities were requested without an AppContainer identity")]
    CapabilitiesWithoutIdentity,
    /// The process attribute list could not be built.
    #[error(
        "the AppContainer process attribute list could not be built (step {step}, code {code})"
    )]
    AttributeListFailed {
        /// Which step failed.
        step: &'static str,
        /// The Win32 error code.
        code: u32,
    },
    /// The attribute list could not be released.
    #[error("the AppContainer process attribute list could not be released")]
    AttributeListNotReleased,
}

/// The profile name ProbeLab derives for every contained child.
pub const PROFILE_NAME: &str = "ProbeLab.ContainedChild";

/// The AppContainer capability SID that grants outbound internet access. Never granted by default.
pub const INTERNET_CLIENT_CAPABILITY: &str = "S-1-15-3-1";
