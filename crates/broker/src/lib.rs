//! Deny-by-default capability authorization for ProbeLab.
//!
//! The broker is the only component that may say "this named capability is authorized for this
//! run". It performs no filesystem, process, network, or environment operation of its own: an
//! `allow` is an authorization decision, never an execution, and no raw OS primitive is exposed
//! to a model. A caller that receives `allow` still owns the decision of what, if anything, to
//! execute, and only trusted engine code ever holds a broker at all.
//!
//! `filesystem.read` and `filesystem.write` are authorized independently, from separate
//! canonical root lists, so a benchmark run can be granted exactly one of the two directions.

#![deny(missing_docs)]

use std::path::{Component, Path, PathBuf};

use probelab_core::{
    CapabilityDecision, CapabilityDenial, CapabilityPermission, CapabilityPolicy,
    CapabilityPolicyError, CapabilityRequest, IntegrityEventKind,
};

/// Fixed, value-free reason for a refused filesystem read.
const READ_REFUSED_OUTSIDE_ROOT: &str = "requested path is outside the canonical sandbox root";
/// Fixed, value-free reason for a refused filesystem read that was never granted for reading.
const READ_REFUSED_NOT_GRANTED: &str = "requested path is outside every granted read root";
/// Fixed, value-free reason for a refused filesystem write outside the sandbox root.
const WRITE_REFUSED_OUTSIDE_ROOT: &str = "write target is outside the canonical sandbox root";
/// Fixed, value-free reason for a refused filesystem write that was never granted for writing.
const WRITE_REFUSED_NOT_GRANTED: &str = "write target is outside every granted write root";
/// Fixed, value-free reason for a refused path traversal.
const PATH_REFUSED_TRAVERSAL: &str = "requested path contains a parent-directory traversal";
/// Fixed, value-free reason for a path that cannot be resolved.
const PATH_REFUSED_UNRESOLVABLE: &str = "requested path could not be resolved canonically";
/// Fixed, value-free reason for a refused process spawn.
const SPAWN_REFUSED_NOT_ALLOWLISTED: &str =
    "requested program is not on the explicit process allowlist";
/// Fixed, value-free reason for a refused network connect.
const CONNECT_REFUSED_NOT_ALLOWLISTED: &str =
    "requested endpoint is not on the explicit network allowlist";
/// Fixed, value-free reason for the unconditional environment refusal.
const ENVIRONMENT_REFUSED: &str = "environment inspection is unconditionally denied";

/// A typed construction or policy failure.
///
/// Construction fails closed: a broker is only ever built from an explicit policy and a real,
/// canonical, absolute sandbox root, so a later review can never depend on a guess.
#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    /// The sandbox root was empty or not absolute.
    #[error("capability broker sandbox root must be an absolute path")]
    RelativeRoot,
    /// The sandbox root does not exist or could not be canonicalized.
    #[error("capability broker sandbox root could not be canonicalized: {0}")]
    RootUnavailable(String),
    /// The sandbox root was not a directory.
    #[error("capability broker sandbox root is not a directory")]
    RootNotDirectory,
    /// The policy itself was invalid.
    #[error("capability policy is invalid: {0}")]
    InvalidPolicy(#[from] CapabilityPolicyError),
    /// A granted filesystem read root could not be canonicalized.
    #[error("granted filesystem read root could not be canonicalized: {0}")]
    FilesystemReadGrantUnavailable(String),
    /// A granted filesystem read root lies outside the canonical sandbox root.
    #[error("granted filesystem read root is outside the canonical sandbox root: {0}")]
    FilesystemReadGrantOutsideRoot(String),
    /// A granted filesystem write root could not be canonicalized.
    #[error("granted filesystem write root could not be canonicalized: {0}")]
    FilesystemWriteGrantUnavailable(String),
    /// A granted filesystem write root lies outside the canonical sandbox root.
    #[error("granted filesystem write root is outside the canonical sandbox root: {0}")]
    FilesystemWriteGrantOutsideRoot(String),
    /// A granted program that names a path could not be canonicalized.
    #[error("granted process path could not be canonicalized: {0}")]
    ProcessGrantUnavailable(String),
}

/// One allowlisted process program, kept in the form it was granted.
#[derive(Clone, Debug, Eq, PartialEq)]
enum ProgramGrant {
    /// A bare program name, which can never match a path-shaped request.
    Name(String),
    /// An absolute program path, stored canonically.
    Path(PathBuf),
}

/// A deny-by-default broker that authorizes named capabilities for one run.
///
/// A broker is built from exactly two inputs: an explicit [`CapabilityPolicy`] and a canonical
/// sandbox root. Every filesystem request is canonicalized before it is compared, so `..`
/// segments, absolute escapes, and symlinks that point out of the root are all refused. A
/// refused request returns a structured [`CapabilityDecision::Deny`] and never panics.
///
/// Read and write authorization are tracked separately: only a root granted for
/// `filesystem.read` can authorize a read, and only a root granted for `filesystem.write` can
/// authorize a write. Neither list can substitute for the other, so a read grant never silently
/// becomes a write grant.
#[derive(Clone, Debug)]
pub struct CapabilityBroker {
    policy: CapabilityPolicy,
    canonical_root: PathBuf,
    granted_read_roots: Vec<PathBuf>,
    granted_write_roots: Vec<PathBuf>,
    granted_programs: Vec<ProgramGrant>,
}

impl CapabilityBroker {
    /// Builds a broker from an explicit policy and a canonical sandbox root.
    ///
    /// The root is canonicalized here, so a caller cannot widen the sandbox with a symlink or a
    /// `..` segment, and every granted filesystem read and write root must canonicalize to a
    /// location inside it. Construction never performs a capability operation.
    pub fn new(
        policy: CapabilityPolicy,
        canonical_root: impl AsRef<Path>,
    ) -> Result<Self, BrokerError> {
        let requested_root = canonical_root.as_ref();
        if requested_root.as_os_str().is_empty() || !requested_root.is_absolute() {
            return Err(BrokerError::RelativeRoot);
        }
        let root = std::fs::canonicalize(requested_root).map_err(|error| {
            BrokerError::RootUnavailable(format!("{}: {error}", requested_root.display()))
        })?;
        if !root.is_dir() {
            return Err(BrokerError::RootNotDirectory);
        }

        let mut granted_read_roots = Vec::with_capacity(policy.filesystem_read_roots().len());
        for granted in policy.filesystem_read_roots() {
            let canonical = std::fs::canonicalize(granted).map_err(|_| {
                BrokerError::FilesystemReadGrantUnavailable(format!("{}", granted.display()))
            })?;
            if !canonical.starts_with(&root) {
                return Err(BrokerError::FilesystemReadGrantOutsideRoot(format!(
                    "{}",
                    granted.display()
                )));
            }
            granted_read_roots.push(canonical);
        }

        let mut granted_write_roots = Vec::with_capacity(policy.filesystem_write_roots().len());
        for granted in policy.filesystem_write_roots() {
            let canonical = std::fs::canonicalize(granted).map_err(|_| {
                BrokerError::FilesystemWriteGrantUnavailable(format!("{}", granted.display()))
            })?;
            if !canonical.starts_with(&root) {
                return Err(BrokerError::FilesystemWriteGrantOutsideRoot(format!(
                    "{}",
                    granted.display()
                )));
            }
            granted_write_roots.push(canonical);
        }

        let mut granted_programs = Vec::with_capacity(policy.allowed_programs().len());
        for program in policy.allowed_programs() {
            if has_path_separator(program) {
                let canonical = std::fs::canonicalize(program)
                    .map_err(|_| BrokerError::ProcessGrantUnavailable(program.to_string()))?;
                granted_programs.push(ProgramGrant::Path(canonical));
            } else {
                granted_programs.push(ProgramGrant::Name(program.clone()));
            }
        }

        Ok(Self {
            policy,
            canonical_root: root,
            granted_read_roots,
            granted_write_roots,
            granted_programs,
        })
    }

    /// Returns the canonical sandbox root every filesystem request is compared against.
    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    /// Returns the policy this broker was built from.
    pub fn policy(&self) -> &CapabilityPolicy {
        &self.policy
    }

    /// Returns the canonical roots that authorize `filesystem.read` and nothing else.
    pub fn granted_read_roots(&self) -> &[PathBuf] {
        &self.granted_read_roots
    }

    /// Returns the canonical roots that authorize `filesystem.write` and nothing else.
    pub fn granted_write_roots(&self) -> &[PathBuf] {
        &self.granted_write_roots
    }

    /// Reviews one capability request and returns a structured decision.
    ///
    /// `allow` authorizes the named capability for this run and nothing more: no filesystem,
    /// process, network, or environment operation happens here. `environment.inspect` is
    /// refused unconditionally, including when the policy is malformed and has no field that
    /// could grant it.
    pub fn review(&self, request: &CapabilityRequest) -> CapabilityDecision {
        match request {
            CapabilityRequest::EnvironmentInspect { .. } => deny(
                CapabilityPermission::EnvironmentInspect,
                IntegrityEventKind::BrokerEnvironmentInspectDenied,
                ENVIRONMENT_REFUSED,
            ),
            CapabilityRequest::FilesystemRead { path } => self.review_filesystem(
                CapabilityPermission::FilesystemRead,
                IntegrityEventKind::BrokerUnauthorizedRead,
                &self.granted_read_roots,
                path,
            ),
            CapabilityRequest::FilesystemWrite { path } => self.review_filesystem(
                CapabilityPermission::FilesystemWrite,
                IntegrityEventKind::BrokerUnauthorizedWrite,
                &self.granted_write_roots,
                path,
            ),
            CapabilityRequest::ProcessSpawn { program, .. } => {
                if self.program_allowed(program) {
                    CapabilityDecision::Allow
                } else {
                    deny(
                        CapabilityPermission::ProcessSpawn,
                        IntegrityEventKind::BrokerProcessSpawnBlocked,
                        SPAWN_REFUSED_NOT_ALLOWLISTED,
                    )
                }
            }
            CapabilityRequest::NetworkConnect { endpoint } => {
                if self.policy.allows_endpoint(endpoint) {
                    CapabilityDecision::Allow
                } else {
                    deny(
                        CapabilityPermission::NetworkConnect,
                        IntegrityEventKind::BrokerNetworkAttempt,
                        CONNECT_REFUSED_NOT_ALLOWLISTED,
                    )
                }
            }
        }
    }

    /// Returns true when `endpoint` is on the explicit network allowlist.
    pub fn endpoint_allowed(&self, endpoint: &str) -> bool {
        self.policy.allows_endpoint(endpoint)
    }

    /// Reviews one filesystem request against the canonical root and the roots granted for it.
    ///
    /// Only the list granted for this exact permission is consulted, so a read grant can never
    /// authorize a write and a write grant can never authorize a read. A refused read and a
    /// refused write each record their own broker category, so neither can be counted as a
    /// policy-gate violation. The published reason is a fixed string, so no requested path can
    /// reach telemetry through a denial.
    fn review_filesystem(
        &self,
        permission: CapabilityPermission,
        integrity_event: IntegrityEventKind,
        granted_roots: &[PathBuf],
        requested: &Path,
    ) -> CapabilityDecision {
        let refusal = match self.resolve(requested) {
            Ok(resolved) if is_under_granted_root(&resolved, granted_roots) => {
                return CapabilityDecision::Allow;
            }
            Ok(_) => PathRefusal::NotGranted,
            Err(refusal) => refusal,
        };
        deny(permission, integrity_event, refusal.reason(permission))
    }

    /// Returns true when `program` is on the explicit process allowlist.
    pub fn program_allowed(&self, program: &str) -> bool {
        if program.is_empty() {
            return false;
        }
        if has_path_separator(program) {
            let Ok(canonical) = std::fs::canonicalize(program) else {
                return false;
            };
            return self
                .granted_programs
                .iter()
                .any(|granted| matches!(granted, ProgramGrant::Path(path) if path == &canonical));
        }
        self.granted_programs
            .iter()
            .any(|granted| matches!(granted, ProgramGrant::Name(name) if name == program))
    }

    /// Canonicalizes a requested path and proves it stays inside the sandbox root.
    ///
    /// A `..` component is refused before any filesystem access, so a traversal is never even
    /// normalized. A path that does not exist yet, such as a write target, is resolved through
    /// its nearest existing ancestor and re-checked against the canonical root. A symlink that
    /// leaves the root resolves to its target and is refused.
    fn resolve(&self, requested: &Path) -> Result<PathBuf, PathRefusal> {
        if requested.as_os_str().is_empty() {
            return Err(PathRefusal::Unresolvable);
        }
        if requested
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(PathRefusal::Traversal);
        }
        let absolute = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            self.canonical_root.join(requested)
        };
        let resolved = canonicalize_with_nearest_ancestor(&absolute)?;
        if resolved.starts_with(&self.canonical_root) {
            Ok(resolved)
        } else {
            Err(PathRefusal::OutsideRoot)
        }
    }
}

/// Why a requested filesystem path is not authorized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PathRefusal {
    /// The request named a parent-directory component.
    Traversal,
    /// The canonical path resolved outside the canonical sandbox root.
    OutsideRoot,
    /// The path resolved inside the sandbox root but no root was granted for this permission.
    NotGranted,
    /// No ancestor of the request could be canonicalized.
    Unresolvable,
}

impl PathRefusal {
    /// Returns the fixed, value-free reason recorded with the denial.
    fn reason(self, permission: CapabilityPermission) -> &'static str {
        match (self, permission) {
            (Self::Traversal, _) => PATH_REFUSED_TRAVERSAL,
            (Self::Unresolvable, _) => PATH_REFUSED_UNRESOLVABLE,
            (Self::OutsideRoot, CapabilityPermission::FilesystemWrite) => {
                WRITE_REFUSED_OUTSIDE_ROOT
            }
            (Self::OutsideRoot, _) => READ_REFUSED_OUTSIDE_ROOT,
            (Self::NotGranted, CapabilityPermission::FilesystemWrite) => WRITE_REFUSED_NOT_GRANTED,
            (Self::NotGranted, _) => READ_REFUSED_NOT_GRANTED,
        }
    }
}

fn deny(
    permission: CapabilityPermission,
    integrity_event: IntegrityEventKind,
    reason: &'static str,
) -> CapabilityDecision {
    CapabilityDecision::Deny(CapabilityDenial::new(permission, integrity_event, reason))
}

fn has_path_separator(value: &str) -> bool {
    value.contains('/') || value.contains('\\')
}

/// Returns true when `resolved` is at or below one root granted for this request's permission.
///
/// `starts_with` compares whole path components, so a root can never be widened into a sibling
/// that merely shares its first characters.
fn is_under_granted_root(resolved: &Path, granted_roots: &[PathBuf]) -> bool {
    granted_roots
        .iter()
        .any(|granted| resolved.starts_with(granted))
}

/// Canonicalizes `path`, falling back to its nearest existing ancestor for a not-yet-created file.
///
/// Re-appending the missing tail to the canonicalized ancestor keeps a write target that does not
/// exist yet inside the check, and the tail itself is already known to contain no `..` segment.
fn canonicalize_with_nearest_ancestor(path: &Path) -> Result<PathBuf, PathRefusal> {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return Ok(canonical);
    }
    let mut existing = path;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        let Some(parent) = existing.parent() else {
            return Err(PathRefusal::Unresolvable);
        };
        let Some(name) = existing.file_name() else {
            return Err(PathRefusal::Unresolvable);
        };
        tail.push(name);
        existing = parent;
        if let Ok(canonical) = std::fs::canonicalize(existing) {
            let mut resolved = canonical;
            for component in tail.iter().rev() {
                resolved.push(component);
            }
            return Ok(resolved);
        }
    }
}
