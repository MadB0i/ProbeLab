//! AppContainer: the child's identity, derived rather than registered.
//!
//! # Why derive instead of create
//!
//! The obvious call is `CreateAppContainerProfile`, and on this host it fails with
//! `HRESULT 0x80070017` (`ERROR_INVALID_NAME`) — see `docs/SECURITY-STATUS.md`. That failure blocked
//! the whole v0.5 identity milestone and left a branch of half-integrated code behind.
//!
//! `DeriveAppContainerSidFromAppContainerName` computes the same package SID **arithmetically from the
//! profile name** and never touches profile registration, so it does not depend on the call that
//! fails. It takes no capability list at all, which is exactly the default this project wants: a
//! derived profile has no capabilities, and the capability list handed to the child is empty unless a
//! benchmark explicitly asks otherwise.
//!
//! One consequence turned out to be wrong, and the measurement is recorded here because the original
//! assumption was reasonable and would otherwise be re-derived. The plan was: because the profile is
//! never registered, the per-user `%LOCALAPPDATA%\Packages\<name>` folder does not exist, so point
//! `TEMP` and `TMP` at a directory inside the run's own designated directory.
//!
//! Measured, a child launched with a derived AppContainer identity gets `%LOCALAPPDATA%\Packages\
//! <name>\AC\Temp`, created during launch, and the values the parent asked for are discarded. So
//! `LOCALAPPDATA` is the part that is genuinely required in the child environment — without it the
//! launch fails outright with `ERROR_ENVVAR_NOT_FOUND` (203), which is the fail-closed behaviour this
//! project wants — while forcing `TEMP`/`TMP` would be theatre. The test asserts what the child
//! actually observes.
//!
//! This is a Windows behaviour ProbeLab does not control, not a guarantee ProbeLab provides. The
//! package temp directory is a real location on disk and is not itself contained beyond the package
//! identity that owns it.
//!
//! # What this does and does not buy
//!
//! * **Identity.** The child's file-system access is bounded by the package identity rather than by
//!   the invoking user's group membership. Objects carry no ACE for a package identity ProbeLab
//!   invented, so the child cannot read the invoking user's files.
//! * **Network.** An AppContainer token is refused outbound network access by the operating system
//!   unless a capability such as `internetClient` appears in its capability list. With an empty list
//!   the OS is the enforcement, independent of the broker.
//!
//! It is **not** a syscall filter. A child can still load native code, and anything the package
//! identity is granted on the machine remains reachable. What is closed is the broad "anything this
//! user can read" surface, plus outbound network.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;

/// The AppContainer profile name ProbeLab derives for every contained child.
///
/// One name for the whole installation is deliberate. The package SID is what the OS grants
/// system-image read access to and what an ACE must name, so a per-run profile would mean rewriting
/// ACLs on the run's sandbox directory with a SID that changes every run, for no added isolation.
pub const PROFILE_NAME: &str = "ProbeLab.ContainedChild";

/// The AppContainer capability SID that grants outbound internet access.
///
/// Never granted by default. A benchmark would have to name this explicitly, and doing so is a
/// deliberate loosening rather than a default.
pub const INTERNET_CLIENT_CAPABILITY: &str = "S-1-15-3-1";

/// A typed AppContainer failure.
#[derive(Debug, thiserror::Error)]
pub enum AppContainerError {
    /// The profile name could not be turned into a SID.
    #[error("the AppContainer SID could not be derived (HRESULT 0x{status:08X})")]
    DeriveFailed {
        /// The raw `HRESULT` from the derive call, kept verbatim.
        status: u32,
    },
    /// The derived SID could not be converted to text for logging.
    #[error("the derived AppContainer SID could not be rendered: {0}")]
    SidNotRenderable(String),
    /// A capability name was not a valid security identifier.
    #[error("AppContainer capability {name:?} is not a valid SID")]
    CapabilityNotASid {
        /// The rejected capability name.
        name: String,
    },
    /// Capabilities were requested without an AppContainer identity to attach them to.
    ///
    /// Refused rather than dropped, because the alternative is reporting a capability as granted when
    /// no token ever carried it.
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
    /// This platform has no AppContainer support at all.
    #[error("AppContainer isolation is not available on this platform")]
    UnsupportedPlatform,
}

/// The `SE_GROUP_ENABLED` attribute bit for `SID_AND_ATTRIBUTES::Attributes`.
///
/// Spelled out because `windows-sys` 0.61 does not export the constant. The value is the documented
/// `0x00000001` from `winnt.h`, which is what makes a capability group enabled on the token.
#[cfg(windows)]
const SE_GROUP_ENABLED: u32 = 0x0000_0001;

/// A derived AppContainer package identity.
///
/// Holds the SID's bytes rather than the pointer the Win32 call hands back, so the identity is owned
/// by Rust and cannot outlive or double-free the OS allocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppContainerIdentity {
    /// The raw SID bytes.
    bytes: Vec<u8>,
    /// The SID in text form, for telemetry and log lines.
    text: String,
}

impl AppContainerIdentity {
    /// Derives the package SID for a profile name.
    ///
    /// This performs no registration and writes nothing to the machine. It is a pure computation from
    /// the name, which is what makes it usable on a host where `CreateAppContainerProfile` fails.
    pub fn derive(profile_name: &str) -> Result<Self, AppContainerError> {
        derive_sid(profile_name)
    }

    /// Returns the SID bytes, for use in a `SECURITY_CAPABILITIES` structure.
    pub fn sid_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the SID in text form, such as `S-1-15-2-...`.
    pub fn sid_text(&self) -> &str {
        &self.text
    }
}

/// Turns a capability name into a SID, so a benchmark can name one without knowing its bytes.
///
/// A capability is an existing security identifier, not a package name, so this **parses** the string
/// rather than deriving a package SID from it. The two are not interchangeable: deriving would turn
/// the well-known internet-client capability `S-1-15-3-1` into an unrelated AppContainer package SID
/// under `S-1-15-2-*`, which would grant nothing while appearing to have granted something.
pub fn capability_sid(name: &str) -> Result<AppContainerIdentity, AppContainerError> {
    #[cfg(windows)]
    {
        parse_sid(name)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Err(AppContainerError::UnsupportedPlatform)
    }
}

#[cfg(windows)]
fn derive_sid(name: &str) -> Result<AppContainerIdentity, AppContainerError> {
    use std::ptr;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName;
    use windows_sys::Win32::Security::PSID;

    let wide = wide_null(name);
    let mut sid: PSID = ptr::null_mut();
    // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the call, and `sid` is a valid
    // out-pointer. On success the OS allocates the SID and hands back ownership.
    let status = unsafe { DeriveAppContainerSidFromAppContainerName(wide.as_ptr(), &mut sid) };
    if status != 0 || sid.is_null() {
        return Err(AppContainerError::DeriveFailed {
            status: status as u32,
        });
    }

    // Everything the OS allocated is copied out and released *before* any `?`, so an early return can
    // never leak the SID.
    // SAFETY: `sid` is a valid SID pointer returned by the derive call above, and both helpers only
    // read from it.
    let (bytes, rendered) = unsafe { (copy_sid(sid), render_sid(sid)) };
    // SAFETY: `sid` came from the derive call above and is released exactly once, here.
    //
    // `LocalFree`, not `FreeSid`. A derived AppContainer SID is allocated with `LocalAlloc` and MSDN
    // states the caller releases it with `LocalFree`. `FreeSid` is for SIDs the security subsystem
    // allocated from its own pool, and passing it one of these is a mismatched free of a live heap
    // block, which corrupts rather than fails loudly.
    unsafe {
        LocalFree(sid.cast::<c_void>());
    }
    let text = rendered.ok_or_else(|| {
        AppContainerError::SidNotRenderable(
            "ConvertSidToStringSidW rejected the derived SID".to_owned(),
        )
    })?;
    Ok(AppContainerIdentity { bytes, text })
}

/// Parses a SID from its text form, for an already-existing capability such as `S-1-15-3-1`.
///
/// # Safety-free by construction
/// The buffer the OS allocates is copied out and released before any `?`, so a failure part-way
/// through cannot leak it.
#[cfg(windows)]
fn parse_sid(text: &str) -> Result<AppContainerIdentity, AppContainerError> {
    use std::ptr;

    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
    use windows_sys::Win32::Security::PSID;

    let wide = wide_null(text);
    let mut sid: PSID = ptr::null_mut();
    // SAFETY: `wide` is a NUL-terminated UTF-16 buffer that outlives the call, and `sid` is a valid
    // out-pointer. On success the OS allocates a SID and transfers ownership to the caller.
    let ok = unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut sid) };
    if let 0 = ok {
        return Err(AppContainerError::CapabilityNotASid {
            name: text.to_owned(),
        });
    }

    // SAFETY: `sid` is a valid SID pointer returned by the parse above, and both helpers only read it.
    let (bytes, rendered) = unsafe { (copy_sid(sid), render_sid(sid)) };
    // SAFETY: `sid` came from `ConvertStringSidToSidW`, which allocates with `LocalAlloc`; MSDN
    // specifies `LocalFree` for the release. Freed exactly once, here.
    unsafe {
        LocalFree(sid.cast::<std::ffi::c_void>());
    }
    let rendered = rendered.ok_or_else(|| {
        AppContainerError::SidNotRenderable(format!("{text:?} could not be rendered back to text"))
    })?;
    Ok(AppContainerIdentity {
        bytes,
        text: rendered,
    })
}

#[cfg(windows)]
fn wide_null(text: &str) -> Vec<u16> {
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Copies a SID's bytes so Rust owns them instead of the OS allocation.
///
/// # Safety
/// `sid` must be a valid SID pointer.
#[cfg(windows)]
pub(crate) unsafe fn copy_sid(sid: *mut std::ffi::c_void) -> Vec<u8> {
    use windows_sys::Win32::Security::GetLengthSid;

    // SAFETY: the caller guarantees `sid` is valid. `GetLengthSid` reports the total size of the
    // structure including its variable-length subauthority array, so a single copy of that many bytes
    // captures the whole SID. A SID is a one-byte revision, a one-byte subauthority count, a six-byte
    // identifier authority, and then one DWORD per subauthority.
    unsafe {
        let length = GetLengthSid(sid) as usize;
        if length == 0 {
            return Vec::new();
        }
        let mut bytes = vec![0u8; length];
        std::ptr::copy_nonoverlapping(sid.cast::<u8>(), bytes.as_mut_ptr(), length);
        bytes
    }
}

/// Renders a SID as text, releasing the OS-allocated string.
///
/// `ConvertSidToStringSidW` returns a **UTF-16** buffer. Reading it as bytes would stop at the first
/// NUL byte, which for an ASCII-looking string like `S-1-15-2-...` arrives immediately and yields a
/// one-character "SID". It has to be decoded as UTF-16 up to the wide terminator.
///
/// # Safety
/// `sid` must be a valid SID pointer.
#[cfg(windows)]
pub(crate) unsafe fn render_sid(sid: *mut std::ffi::c_void) -> Option<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;

    // SAFETY: `sid` is valid; the out-pointer receives an OS-allocated wide string this function frees.
    unsafe {
        let mut text: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(sid, &mut text) == 0 || text.is_null() {
            return None;
        }
        let mut units = Vec::new();
        let mut cursor = text;
        while *cursor != 0 {
            units.push(*cursor);
            cursor = cursor.add(1);
        }
        LocalFree(text.cast::<c_void>());
        Some(String::from_utf16_lossy(&units))
    }
}

/// A process attribute list carrying the AppContainer capability, or nothing.
///
/// The identity is **owned** here rather than borrowed. `SECURITY_CAPABILITIES` holds a raw pointer
/// to the AppContainer SID, and `UpdateProcThreadAttribute` reads through it, so the bytes the pointer
/// refers to must outlive the call. Owning a `Box<AppContainerIdentity>` gives a heap-stable address
/// that stays valid for exactly as long as this list exists; a borrowed or stack identity would leave
/// that pointer dangling the moment `build` returned.
pub struct SecurityAttributeList {
    #[cfg(windows)]
    list: *mut c_void,
    #[cfg(windows)]
    security: *mut windows_sys::Win32::Security::SECURITY_CAPABILITIES,
    /// Kept alive so the pointer inside `security` stays valid.
    #[cfg(windows)]
    identity: Box<AppContainerIdentity>,
    /// The attribute-list allocation itself, owned so `Drop` can release it at its real size.
    ///
    /// Storing the box rather than only a leaked pointer is what keeps the free well-formed: the
    /// allocation is a `Box<[u8]>`, so releasing it through a bare pointer with a one-byte layout
    /// would hand the allocator a size that does not match the request.
    #[cfg(windows)]
    buffer: Option<Box<[u8]>>,
    /// The `SID_AND_ATTRIBUTES` array named by `SECURITY_CAPABILITIES::Capabilities`. It is empty
    /// unless a benchmark explicitly asked for a capability, which is the default.
    #[cfg(windows)]
    capability_array: Box<[windows_sys::Win32::Security::SID_AND_ATTRIBUTES]>,
    capabilities: Vec<AppContainerIdentity>,
}

impl SecurityAttributeList {
    /// Builds the list for a derived identity with no capabilities.
    pub fn for_identity(identity: &AppContainerIdentity) -> Result<Self, AppContainerError> {
        Self::build(Some(identity.clone()), Vec::new())
    }

    /// Builds the list for a derived identity with an explicit capability list.
    ///
    /// An empty capability list is the safe default and is what every current caller uses. Passing a
    /// non-empty list is a deliberate loosening: the caller is choosing to grant, for example,
    /// `internetClient`, and the grant then shows up in the containment telemetry.
    pub fn with_capabilities(
        identity: &AppContainerIdentity,
        capabilities: Vec<AppContainerIdentity>,
    ) -> Result<Self, AppContainerError> {
        Self::build(Some(identity.clone()), capabilities)
    }

    #[cfg(windows)]
    fn build(
        identity: Option<AppContainerIdentity>,
        capabilities: Vec<AppContainerIdentity>,
    ) -> Result<Self, AppContainerError> {
        use std::mem::size_of;
        use std::ptr;

        use windows_sys::Win32::Security::SECURITY_CAPABILITIES;
        use windows_sys::Win32::Security::SID_AND_ATTRIBUTES;
        use windows_sys::Win32::System::Threading::{
            DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
            UpdateProcThreadAttribute, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
        };

        let Some(identity) = identity.map(Box::new) else {
            // Capabilities without an AppContainer identity would be silently discarded, and a
            // caller that asked for `internetClient` would be told it was granted. Refuse instead:
            // the caller asked for something this list cannot deliver.
            if !capabilities.is_empty() {
                return Err(AppContainerError::CapabilitiesWithoutIdentity);
            }
            return Ok(Self {
                list: ptr::null_mut(),
                security: ptr::null_mut(),
                identity: Box::new(AppContainerIdentity {
                    bytes: Vec::new(),
                    text: String::new(),
                }),
                buffer: None,
                capability_array: Box::new([]),
                capabilities,
            });
        };

        let mut bytes = 0usize;
        // SAFETY: the probing call is specified to take null and report the required size.
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(AppContainerError::AttributeListFailed {
                step: "InitializeProcThreadAttributeList-size",
                code: 0,
            });
        }
        let mut list = vec![0u8; bytes];
        // SAFETY: `list` is at least `bytes` long, which is what the probing call just reported.
        let initialised = unsafe {
            InitializeProcThreadAttributeList(list.as_mut_ptr().cast::<c_void>(), 1, 0, &mut bytes)
        };
        if initialised == 0 {
            return Err(AppContainerError::AttributeListFailed {
                step: "InitializeProcThreadAttributeList",
                code: std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32,
            });
        }

        // The SECURITY_CAPABILITIES block points at the boxed identity's SID bytes. The box is
        // heap-stable and owned by the returned struct, so the pointer stays valid for as long as the
        // attribute list does — which is the whole lifetime of the `CreateProcessW` call that uses it.
        //
        // Capabilities are wired through properly, because a grant that is reported but not passed to
        // the OS is the worst possible outcome: the telemetry would claim a capability the child does
        // not have. The `SID_AND_ATTRIBUTES` array is boxed so its address is stable for the same
        // reason, and each entry points into a boxed capability whose bytes outlive the call.
        let mut capability_array: Vec<SID_AND_ATTRIBUTES> = capabilities
            .iter()
            .map(|capability| SID_AND_ATTRIBUTES {
                Sid: capability.sid_bytes().as_ptr() as *mut c_void,
                Attributes: SE_GROUP_ENABLED,
            })
            .collect();
        let mut capability_array: Box<[SID_AND_ATTRIBUTES]> = capability_array.drain(..).collect();
        let mut security = Box::new(SECURITY_CAPABILITIES {
            AppContainerSid: identity.sid_bytes().as_ptr() as *mut c_void,
            Capabilities: if capability_array.is_empty() {
                ptr::null_mut()
            } else {
                capability_array.as_mut_ptr()
            },
            CapabilityCount: capability_array.len() as u32,
            Reserved: 0,
        });
        let security_ptr: *mut SECURITY_CAPABILITIES = &mut *security;

        // SAFETY: the list was initialised for exactly one attribute above, and
        // `security_ptr` points at a live SECURITY_CAPABILITIES for the duration of the call.
        let updated = unsafe {
            UpdateProcThreadAttribute(
                list.as_mut_ptr().cast::<c_void>(),
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                security_ptr.cast::<c_void>(),
                size_of::<SECURITY_CAPABILITIES>(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if updated == 0 {
            // SAFETY: the list was initialised, so deleting it is valid.
            unsafe {
                DeleteProcThreadAttributeList(list.as_mut_ptr().cast::<c_void>());
            }
            return Err(AppContainerError::AttributeListFailed {
                step: "UpdateProcThreadAttribute",
                code: std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32,
            });
        }

        // The buffer must not move while the OS holds a pointer to it, so it is kept as an owned
        // `Box<[u8]>` and only a raw pointer is handed out. Holding the box is both the leak-free and
        // the layout-safe option: `Drop` frees the original allocation with its original length,
        // rather than reconstructing a differently-sized `Box` from the bare pointer.
        let mut buffer: Box<[u8]> = list.into_boxed_slice();
        let list_ptr = buffer.as_mut_ptr().cast::<c_void>();
        Ok(Self {
            list: list_ptr,
            buffer: Some(buffer),
            security: Box::into_raw(security).cast::<SECURITY_CAPABILITIES>(),
            identity,
            capability_array,
            capabilities,
        })
    }

    #[cfg(not(windows))]
    fn build(
        _identity: Option<AppContainerIdentity>,
        _capabilities: Vec<AppContainerIdentity>,
    ) -> Result<Self, AppContainerError> {
        Err(AppContainerError::UnsupportedPlatform)
    }

    /// Returns the attribute list pointer for `STARTUPINFOEXW`, or null when there is none.
    pub fn pointer(&self) -> *mut c_void {
        #[cfg(windows)]
        {
            self.list
        }
        #[cfg(not(windows))]
        {
            std::ptr::null_mut()
        }
    }

    /// Returns true when this list actually carries an AppContainer identity.
    pub fn is_present(&self) -> bool {
        !self.pointer().is_null()
    }

    /// Returns the AppContainer SID this list applies.
    pub fn identity(&self) -> &AppContainerIdentity {
        &self.identity
    }

    /// Returns the capabilities granted, for telemetry.
    pub fn granted_capabilities(&self) -> Vec<String> {
        self.capabilities
            .iter()
            .map(|capability| capability.sid_text().to_owned())
            .collect()
    }

    /// Returns the capability count actually handed to the OS.
    ///
    /// Read back from the array that `SECURITY_CAPABILITIES::Capabilities` points at, so this is the
    /// grant the token will really carry rather than the request that was made. It exists to be
    /// logged, because a capability that is requested but not passed to the OS is exactly the silent
    /// degradation worth catching.
    pub fn granted_capability_count(&self) -> u32 {
        #[cfg(windows)]
        {
            self.capability_array.len() as u32
        }
        #[cfg(not(windows))]
        {
            0
        }
    }
}

impl Drop for SecurityAttributeList {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use std::ptr;

            use windows_sys::Win32::System::Threading::DeleteProcThreadAttributeList;
            if !self.list.is_null() {
                // SAFETY: the list was initialised in `build` and is deleted exactly once.
                unsafe {
                    DeleteProcThreadAttributeList(self.list);
                }
                // The allocation is released by dropping the owned box below, which knows its own
                // length. `UpdateProcThreadAttribute` is the last reader of it.
                self.buffer = None;
                self.list = ptr::null_mut();
            }
            if !self.security.is_null() {
                // SAFETY: the boxed SECURITY_CAPABILITIES is released exactly once, here.
                unsafe {
                    drop(Box::from_raw(
                        self.security
                            .cast::<windows_sys::Win32::Security::SECURITY_CAPABILITIES>(),
                    ));
                }
                self.security = ptr::null_mut();
            }
        }
    }
}

/// A request to run a child inside a derived AppContainer.
///
/// Capabilities are names rather than resolved SIDs at the request layer, so a caller cannot smuggle
/// an arbitrary SID past the intent-reading code without it appearing in plain text in the benchmark
/// definition and in the telemetry.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppContainerRequest {
    /// Capability SIDs to grant. Empty means no capabilities, which is the default and means no
    /// outbound network access.
    pub capabilities: Vec<String>,
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// The capability list handed to the OS is the one that was asked for, and the default is empty.
    ///
    /// **Scope, stated honestly.** This proves ProbeLab's own wiring: that a requested capability is
    /// parsed into the right SID, and that the count and names reported back are the ones passed to
    /// `SECURITY_CAPABILITIES`. It does **not** prove the OS honours a granted capability at connect
    /// time, and the first attempt at that was instructive — `internetClient` grants outbound internet
    /// access, while AppContainer *loopback* is refused separately and needs a `CheckNetIsolation`
    /// loopback exemption. A loopback fetch would have tested the wrong mechanism. Nor can the child
    /// be asked about its own token: `whoami.exe` is refused inside the AppContainer with
    /// `Access is denied.`, the same way `waitfor.exe` is, and the child is resumed before a caller
    /// could inspect its token while suspended. The end-to-end half is genuinely untested, and is
    /// recorded as such rather than approximated by a test that would pass for the wrong reason.
    #[test]
    fn a_granted_capability_is_reported_back_exactly_as_requested() {
        let package = AppContainerIdentity::derive(PROFILE_NAME).expect("derive the package SID");

        // The default: no capabilities at all, which is what makes the OS refuse outbound network.
        let default =
            SecurityAttributeList::for_identity(&package).expect("build the default list");
        assert!(
            default.granted_capabilities().is_empty(),
            "the default capability list was not empty, so network denial was never the default"
        );
        assert_eq!(
            default.granted_capability_count(),
            0,
            "the default list reported a non-zero capability count"
        );

        // An explicit request round-trips as the SID it named.
        let capability = capability_sid(INTERNET_CLIENT_CAPABILITY).expect("parse the capability");
        let granted = SecurityAttributeList::with_capabilities(&package, vec![capability])
            .expect("build a list with one capability");
        assert_eq!(
            granted.granted_capabilities(),
            vec![INTERNET_CLIENT_CAPABILITY.to_owned()],
            "the reported capability is not the one that was requested"
        );
        assert_eq!(
            granted.granted_capability_count(),
            1,
            "one capability was requested but a different count would reach the OS"
        );
        assert_eq!(
            granted.identity().sid_text(),
            package.sid_text(),
            "the capability list changed the AppContainer identity it applies to"
        );
    }

    /// Capabilities requested without an identity to attach them to are refused, not dropped.
    ///
    /// The alternative is the worst failure mode available here: the caller asks for
    /// `internetClient`, the list comes back reporting it as granted, and no token ever carried it.
    /// Reporting the refusal keeps the guarantee fail-closed.
    #[test]
    fn capabilities_without_an_identity_are_refused_rather_than_silently_dropped() {
        let capability = capability_sid(INTERNET_CLIENT_CAPABILITY).expect("parse the capability");
        let outcome = SecurityAttributeList::build(None, vec![capability]);
        assert!(
            matches!(outcome, Err(AppContainerError::CapabilitiesWithoutIdentity)),
            "capabilities with no identity were not refused"
        );

        // With no identity *and* no capabilities there is nothing to lose, so an inert list is fine.
        assert!(
            SecurityAttributeList::build(None, Vec::new()).is_ok(),
            "an empty, identity-free list should be allowed, because it grants nothing"
        );
    }
}
