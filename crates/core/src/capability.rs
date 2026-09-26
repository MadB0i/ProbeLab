//! Deny-by-default capability contracts shared by benchmarks and capability brokers.
//!
//! The types in this module describe *authorization only*. Nothing here executes a filesystem,
//! process, network, or environment operation, and no raw OS primitive is exposed to a model.
//! A model may name a capability, and only a trusted broker may decide whether that name is
//! authorized for the current run.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::IntegrityEventKind;

/// A single, closed set of permissions that a capability broker can authorize.
///
/// The serialized names are the stable wire and storage contract. A benchmark or a policy
/// document refers to a permission by exactly one of these names and never by a variant name.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum CapabilityPermission {
    /// Read a file or directory below a root granted for reading only.
    #[serde(rename = "filesystem.read")]
    FilesystemRead,
    /// Write a file below a root granted for writing only.
    #[serde(rename = "filesystem.write")]
    FilesystemWrite,
    /// Spawn a process whose program is on an explicit allowlist.
    #[serde(rename = "process.spawn")]
    ProcessSpawn,
    /// Open a network connection to an explicitly allowlisted endpoint.
    #[serde(rename = "network.connect")]
    NetworkConnect,
    /// Inspect a process environment variable.
    #[serde(rename = "environment.inspect")]
    EnvironmentInspect,
}

impl CapabilityPermission {
    /// Every permission, in a stable order, for exhaustive coverage in tests and reports.
    pub const ALL: [CapabilityPermission; 5] = [
        CapabilityPermission::FilesystemRead,
        CapabilityPermission::FilesystemWrite,
        CapabilityPermission::ProcessSpawn,
        CapabilityPermission::NetworkConnect,
        CapabilityPermission::EnvironmentInspect,
    ];

    /// Returns the stable serialized permission name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FilesystemRead => "filesystem.read",
            Self::FilesystemWrite => "filesystem.write",
            Self::ProcessSpawn => "process.spawn",
            Self::NetworkConnect => "network.connect",
            Self::EnvironmentInspect => "environment.inspect",
        }
    }

    /// Returns a short, allocation-free identifier suitable for logs and metrics labels.
    pub fn label(self) -> &'static str {
        match self {
            Self::FilesystemRead => "filesystem_read",
            Self::FilesystemWrite => "filesystem_write",
            Self::ProcessSpawn => "process_spawn",
            Self::NetworkConnect => "network_connect",
            Self::EnvironmentInspect => "environment_inspect",
        }
    }
}

impl std::fmt::Display for CapabilityPermission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A named request for one capability, carrying only the target a broker must compare.
///
/// A request is data, not an instruction: the broker decides, and no request value is ever
/// executed. `EnvironmentInspect` carries a variable *name* only; this type has no field that
/// could hold an environment value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CapabilityRequest {
    /// Read the file or directory at `path`.
    FilesystemRead {
        /// Requested filesystem path, relative to the sandbox root unless absolute.
        path: PathBuf,
    },
    /// Write to the file at `path`, which need not exist yet.
    FilesystemWrite {
        /// Requested filesystem path, relative to the sandbox root unless absolute.
        path: PathBuf,
    },
    /// Spawn `program` with `args`.
    ProcessSpawn {
        /// Requested program name or absolute program path.
        program: String,
        /// Requested process arguments.
        args: Vec<String>,
    },
    /// Connect to a `host:port` endpoint.
    NetworkConnect {
        /// Requested network endpoint in `host:port` form.
        endpoint: String,
    },
    /// Inspect the value of one environment variable.
    EnvironmentInspect {
        /// Requested environment variable name; never a value.
        variable: String,
    },
}

impl CapabilityRequest {
    /// Returns the single permission this request would exercise.
    pub fn permission(&self) -> CapabilityPermission {
        match self {
            Self::FilesystemRead { .. } => CapabilityPermission::FilesystemRead,
            Self::FilesystemWrite { .. } => CapabilityPermission::FilesystemWrite,
            Self::ProcessSpawn { .. } => CapabilityPermission::ProcessSpawn,
            Self::NetworkConnect { .. } => CapabilityPermission::NetworkConnect,
            Self::EnvironmentInspect { .. } => CapabilityPermission::EnvironmentInspect,
        }
    }
}

/// A structured refusal that a broker returns instead of executing anything.
///
/// A denial is safe to persist: it names the permission and its independent integrity category
/// and carries a fixed, operator-authored reason. It never contains a path, program, endpoint,
/// process argument, or environment variable, so a denial can be recorded verbatim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilityDenial {
    /// Permission the broker refused to authorize.
    pub permission: CapabilityPermission,
    /// Independent integrity category for this refusal.
    pub integrity_event: IntegrityEventKind,
    /// Fixed, value-free explanation of why the request was refused.
    pub reason: String,
}

impl CapabilityDenial {
    /// Creates a denial from a permission, its integrity category, and a value-free reason.
    pub fn new(
        permission: CapabilityPermission,
        integrity_event: IntegrityEventKind,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            permission,
            integrity_event,
            reason: reason.into(),
        }
    }
}

/// The only decision a capability broker may return.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "decision", content = "denial")]
pub enum CapabilityDecision {
    /// The request is authorized for this run; authorization alone performs no operation.
    Allow,
    /// The request is refused and can never execute.
    Deny(CapabilityDenial),
}

impl CapabilityDecision {
    /// Returns the denial, or `None` when the request was authorized.
    pub fn denial(&self) -> Option<&CapabilityDenial> {
        match self {
            Self::Allow => None,
            Self::Deny(denial) => Some(denial),
        }
    }

    /// Returns true only when the request was authorized.
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// A typed failure from an explicit capability grant.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CapabilityPolicyError {
    /// `environment.inspect` has no grant path in any policy.
    #[error("environment.inspect cannot be granted by a capability policy")]
    EnvironmentInspectionUngrantable,
    /// A bare permission grant cannot name a target, so it is rejected explicitly.
    #[error("{permission} must be granted with an explicit target")]
    ExplicitTargetRequired {
        /// Permission that was granted without a target.
        permission: CapabilityPermission,
    },
    /// A filesystem read or write root grant was empty or not absolute.
    #[error("a filesystem root grant must be a non-empty absolute path")]
    InvalidFilesystemRoot,
    /// A process grant was empty or contained no usable program.
    #[error("a process grant must be a non-empty program name")]
    InvalidProcessGrant,
    /// A network endpoint grant was empty or was not `host:port`.
    #[error("a network endpoint grant must be a non-empty host:port endpoint")]
    InvalidNetworkEndpoint,
}

/// A serializable, deny-by-default declaration of the capabilities granted for one run.
///
/// Benchmarks declare their own per-run grants with this type, so declaring a policy never
/// requires a broker implementation. The default policy grants nothing at all, and there is
/// deliberately no field or method that could grant `environment.inspect` or record an
/// environment value.
///
/// `filesystem.read` and `filesystem.write` are granted independently: they are stored in two
/// separate, serialized fields and are never widened into one another. A policy document is the
/// stable contract, so a legacy document that still carries only `filesystem_roots` grants
/// nothing at all, because an unknown field is ignored rather than guessed at.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilityPolicy {
    /// Filesystem roots explicitly granted for `filesystem.read` requests only.
    #[serde(default)]
    filesystem_read_roots: Vec<PathBuf>,
    /// Filesystem roots explicitly granted for `filesystem.write` requests only.
    #[serde(default)]
    filesystem_write_roots: Vec<PathBuf>,
    /// Programs explicitly granted for spawn requests.
    #[serde(default)]
    allowed_programs: Vec<String>,
    /// Endpoints explicitly granted for connect requests.
    #[serde(default)]
    allowed_endpoints: Vec<String>,
}

impl CapabilityPolicy {
    /// Returns a policy that grants nothing.
    pub const fn deny_all() -> Self {
        Self {
            filesystem_read_roots: Vec::new(),
            filesystem_write_roots: Vec::new(),
            allowed_programs: Vec::new(),
            allowed_endpoints: Vec::new(),
        }
    }

    /// Returns true when no capability of any kind is granted.
    pub fn is_deny_all(&self) -> bool {
        self.filesystem_read_roots.is_empty()
            && self.filesystem_write_roots.is_empty()
            && self.allowed_programs.is_empty()
            && self.allowed_endpoints.is_empty()
    }

    /// Returns the roots granted for `filesystem.read` only.
    ///
    /// A root in this list authorizes reads and never authorizes writes; a write grant is listed
    /// by [`CapabilityPolicy::filesystem_write_roots`] instead.
    pub fn filesystem_read_roots(&self) -> &[PathBuf] {
        &self.filesystem_read_roots
    }

    /// Returns the roots granted for `filesystem.write` only.
    ///
    /// A root in this list authorizes writes and never authorizes reads; a read grant is listed
    /// by [`CapabilityPolicy::filesystem_read_roots`] instead.
    pub fn filesystem_write_roots(&self) -> &[PathBuf] {
        &self.filesystem_write_roots
    }

    /// Returns the explicitly granted process programs.
    pub fn allowed_programs(&self) -> &[String] {
        &self.allowed_programs
    }

    /// Returns the explicitly granted network endpoints.
    pub fn allowed_endpoints(&self) -> &[String] {
        &self.allowed_endpoints
    }

    /// Grants one permission without naming a target.
    ///
    /// `environment.inspect` is refused here because no policy may grant it. Every other
    /// permission is refused here because a bare grant cannot name what it authorizes; use
    /// [`CapabilityPolicy::grant_filesystem_read_root`],
    /// [`CapabilityPolicy::grant_filesystem_write_root`],
    /// [`CapabilityPolicy::grant_filesystem_root`], [`CapabilityPolicy::grant_process`], or
    /// [`CapabilityPolicy::grant_network_endpoint`] instead.
    pub fn grant_permission(
        &mut self,
        permission: CapabilityPermission,
    ) -> Result<(), CapabilityPolicyError> {
        match permission {
            CapabilityPermission::EnvironmentInspect => {
                Err(CapabilityPolicyError::EnvironmentInspectionUngrantable)
            }
            permission => Err(CapabilityPolicyError::ExplicitTargetRequired { permission }),
        }
    }

    /// Grants `filesystem.read` only, below one explicit absolute root.
    ///
    /// This never grants `filesystem.write`: a read-only benchmark may inspect the tree it is
    /// given without being able to change it. Repeating an existing grant is a no-op. The root
    /// is stored as declared; a broker canonicalizes it before any comparison.
    pub fn grant_filesystem_read_root(
        &mut self,
        path: impl Into<PathBuf>,
    ) -> Result<(), CapabilityPolicyError> {
        grant_root(&mut self.filesystem_read_roots, path.into())
    }

    /// Grants `filesystem.write` only, below one explicit absolute root.
    ///
    /// This never grants `filesystem.read`: an output directory may be writable without being
    /// readable, so a benchmark can produce results without being able to read back what it was
    /// not given. Repeating an existing grant is a no-op. The root is stored as declared; a
    /// broker canonicalizes it before any comparison.
    pub fn grant_filesystem_write_root(
        &mut self,
        path: impl Into<PathBuf>,
    ) -> Result<(), CapabilityPolicyError> {
        grant_root(&mut self.filesystem_write_roots, path.into())
    }

    /// Grants filesystem read *and* write below one explicit absolute root.
    ///
    /// This is an explicit convenience for a benchmark that genuinely wants both permissions in
    /// the same place, and it is exactly the composition of
    /// [`CapabilityPolicy::grant_filesystem_read_root`] and
    /// [`CapabilityPolicy::grant_filesystem_write_root`]. It is never a way to obtain one
    /// direction from the other, so prefer the two direction-specific methods whenever only one
    /// direction is intended. Repeating an existing grant is a no-op for each direction.
    pub fn grant_filesystem_root(
        &mut self,
        path: impl Into<PathBuf>,
    ) -> Result<(), CapabilityPolicyError> {
        let path = path.into();
        self.grant_filesystem_read_root(path.clone())?;
        self.grant_filesystem_write_root(path)
    }

    /// Grants process spawn for one explicit program name or absolute program path.
    ///
    /// Repeating an existing grant is a no-op. A program that names a path must be granted by
    /// that same path; a bare name never matches a path.
    pub fn grant_process(
        &mut self,
        program: impl Into<String>,
    ) -> Result<(), CapabilityPolicyError> {
        let program = program.into();
        if program.trim().is_empty() {
            return Err(CapabilityPolicyError::InvalidProcessGrant);
        }
        if !self
            .allowed_programs
            .iter()
            .any(|granted| granted == &program)
        {
            self.allowed_programs.push(program);
        }
        Ok(())
    }

    /// Grants network connect for one explicit `host:port` endpoint.
    ///
    /// Repeating an existing grant is a no-op. Endpoints are compared literally, so a grant
    /// never widens to a different host or port.
    pub fn grant_network_endpoint(
        &mut self,
        endpoint: impl Into<String>,
    ) -> Result<(), CapabilityPolicyError> {
        let endpoint = endpoint.into();
        if !is_explicit_endpoint(&endpoint) {
            return Err(CapabilityPolicyError::InvalidNetworkEndpoint);
        }
        if !self
            .allowed_endpoints
            .iter()
            .any(|granted| granted == &endpoint)
        {
            self.allowed_endpoints.push(endpoint);
        }
        Ok(())
    }

    /// Returns true when `program` is on the explicit process allowlist.
    pub fn allows_program(&self, program: &str) -> bool {
        self.allowed_programs
            .iter()
            .any(|granted| granted == program)
    }

    /// Returns true when `endpoint` is on the explicit network allowlist.
    pub fn allows_endpoint(&self, endpoint: &str) -> bool {
        self.allowed_endpoints
            .iter()
            .any(|granted| granted == endpoint)
    }
}

/// Validates one declared filesystem root and records it in its own direction's list.
fn grant_root(roots: &mut Vec<PathBuf>, path: PathBuf) -> Result<(), CapabilityPolicyError> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err(CapabilityPolicyError::InvalidFilesystemRoot);
    }
    if !roots.contains(&path) {
        roots.push(path);
    }
    Ok(())
}

fn is_explicit_endpoint(endpoint: &str) -> bool {
    let Some((host, port)) = endpoint.rsplit_once(':') else {
        return false;
    };
    if host.is_empty() || host.contains('/') || port.is_empty() {
        return false;
    }
    port.parse::<u32>()
        .is_ok_and(|port| (1..=65_535).contains(&port))
}

#[cfg(test)]
mod tests {
    use super::{
        CapabilityDecision, CapabilityDenial, CapabilityPermission, CapabilityPolicy,
        CapabilityPolicyError, CapabilityRequest, IntegrityEventKind,
    };
    use std::path::PathBuf;

    #[test]
    fn permission_serde_names_are_stable() {
        let cases = [
            (CapabilityPermission::FilesystemRead, "filesystem.read"),
            (CapabilityPermission::FilesystemWrite, "filesystem.write"),
            (CapabilityPermission::ProcessSpawn, "process.spawn"),
            (CapabilityPermission::NetworkConnect, "network.connect"),
            (
                CapabilityPermission::EnvironmentInspect,
                "environment.inspect",
            ),
        ];
        assert_eq!(cases.len(), CapabilityPermission::ALL.len());
        for (permission, name) in cases {
            assert_eq!(
                serde_json::to_string(&permission).expect("permission JSON"),
                format!("\"{name}\"")
            );
            assert_eq!(
                serde_json::from_str::<CapabilityPermission>(&format!("\"{name}\""))
                    .expect("permission round trip"),
                permission
            );
            assert_eq!(permission.as_str(), name);
        }
    }

    #[test]
    fn environment_inspect_requests_carry_no_value() {
        let request = CapabilityRequest::EnvironmentInspect {
            variable: "PROBELAB_TEST_SECRET".to_owned(),
        };
        let json = serde_json::to_string(&request).expect("request JSON");
        assert_eq!(
            json,
            r#"{"kind":"environment_inspect","variable":"PROBELAB_TEST_SECRET"}"#
        );
        assert!(!json.contains("value"));
        assert_eq!(
            request.permission(),
            CapabilityPermission::EnvironmentInspect
        );
    }

    #[test]
    fn requests_map_to_exactly_one_permission() {
        let requests = [
            (
                CapabilityRequest::FilesystemRead {
                    path: PathBuf::from("evaluator.rs"),
                },
                CapabilityPermission::FilesystemRead,
            ),
            (
                CapabilityRequest::FilesystemWrite {
                    path: PathBuf::from("notes.txt"),
                },
                CapabilityPermission::FilesystemWrite,
            ),
            (
                CapabilityRequest::ProcessSpawn {
                    program: "sh".to_owned(),
                    args: vec!["-c".to_owned()],
                },
                CapabilityPermission::ProcessSpawn,
            ),
            (
                CapabilityRequest::NetworkConnect {
                    endpoint: "127.0.0.1:8080".to_owned(),
                },
                CapabilityPermission::NetworkConnect,
            ),
            (
                CapabilityRequest::EnvironmentInspect {
                    variable: "PATH".to_owned(),
                },
                CapabilityPermission::EnvironmentInspect,
            ),
        ];
        for (request, expected) in &requests {
            assert_eq!(request.permission(), *expected);
        }
        for permission in CapabilityPermission::ALL {
            assert!(
                requests
                    .iter()
                    .any(|(request, _)| request.permission() == permission),
                "permission {permission:?} has no request"
            );
        }
    }

    #[test]
    fn decisions_carry_a_value_free_denial() {
        let denial = CapabilityDenial::new(
            CapabilityPermission::NetworkConnect,
            IntegrityEventKind::BrokerNetworkAttempt,
            "endpoint is not on the explicit network allowlist",
        );
        let decision = CapabilityDecision::Deny(denial.clone());
        assert!(!decision.is_allowed());
        assert_eq!(decision.denial(), Some(&denial));
        assert!(CapabilityDecision::Allow.is_allowed());
        assert!(CapabilityDecision::Allow.denial().is_none());
        assert_eq!(
            serde_json::to_string(&decision).expect("decision JSON"),
            r#"{"decision":"deny","denial":{"permission":"network.connect","integrity_event":"broker_network_attempt","reason":"endpoint is not on the explicit network allowlist"}}"#
        );
    }

    #[test]
    fn the_default_policy_denies_everything() {
        let policy = CapabilityPolicy::default();
        assert!(policy.is_deny_all());
        assert_eq!(policy, CapabilityPolicy::deny_all());
        assert!(policy.filesystem_read_roots().is_empty());
        assert!(policy.filesystem_write_roots().is_empty());
        assert!(!policy.allows_program("sh"));
        assert!(!policy.allows_endpoint("127.0.0.1:8080"));
    }

    #[test]
    fn a_read_grant_is_recorded_only_as_a_read_root() {
        let root = std::env::temp_dir();
        let mut policy = CapabilityPolicy::deny_all();
        policy
            .grant_filesystem_read_root(&root)
            .expect("read grant");
        assert_eq!(policy.filesystem_read_roots(), &[root]);
        assert!(
            policy.filesystem_write_roots().is_empty(),
            "a read grant must not appear as a write root"
        );
        assert!(!policy.is_deny_all());
    }

    #[test]
    fn a_write_grant_is_recorded_only_as_a_write_root() {
        let root = std::env::temp_dir();
        let mut policy = CapabilityPolicy::deny_all();
        policy
            .grant_filesystem_write_root(&root)
            .expect("write grant");
        assert_eq!(policy.filesystem_write_roots(), &[root]);
        assert!(
            policy.filesystem_read_roots().is_empty(),
            "a write grant must not appear as a read root"
        );
        assert!(!policy.is_deny_all());
    }

    #[test]
    fn the_combined_filesystem_grant_records_both_directions() {
        let root = std::env::temp_dir();
        let mut policy = CapabilityPolicy::deny_all();
        policy.grant_filesystem_root(&root).expect("grant");
        assert_eq!(policy.filesystem_read_roots(), std::slice::from_ref(&root));
        assert_eq!(policy.filesystem_write_roots(), &[root]);
    }

    #[test]
    fn read_and_write_roots_are_tracked_per_run_and_never_merged() {
        let read_only = PathBuf::from(if cfg!(windows) {
            r"C:\probelab\read-only"
        } else {
            "/probelab/read-only"
        });
        let write_only = PathBuf::from(if cfg!(windows) {
            r"C:\probelab\write-only"
        } else {
            "/probelab/write-only"
        });
        let both = PathBuf::from(if cfg!(windows) {
            r"C:\probelab\both"
        } else {
            "/probelab/both"
        });

        let mut policy = CapabilityPolicy::deny_all();
        policy
            .grant_filesystem_read_root(&read_only)
            .expect("read grant");
        policy
            .grant_filesystem_write_root(&write_only)
            .expect("write grant");
        policy.grant_filesystem_root(&both).expect("both grant");

        assert_eq!(policy.filesystem_read_roots(), &[read_only, both.clone()]);
        assert_eq!(policy.filesystem_write_roots(), &[write_only, both]);
    }

    #[test]
    fn a_direction_specific_grant_rejects_the_same_invalid_roots() {
        let mut policy = CapabilityPolicy::deny_all();
        for invalid in ["", "relative/path"] {
            assert_eq!(
                policy.grant_filesystem_read_root(invalid),
                Err(CapabilityPolicyError::InvalidFilesystemRoot)
            );
            assert_eq!(
                policy.grant_filesystem_write_root(invalid),
                Err(CapabilityPolicyError::InvalidFilesystemRoot)
            );
            assert_eq!(
                policy.grant_filesystem_root(invalid),
                Err(CapabilityPolicyError::InvalidFilesystemRoot)
            );
        }
        assert!(policy.is_deny_all());
    }

    #[test]
    fn a_rejected_combined_grant_records_neither_direction() {
        let mut policy = CapabilityPolicy::deny_all();
        assert_eq!(
            policy.grant_filesystem_root("relative/path"),
            Err(CapabilityPolicyError::InvalidFilesystemRoot)
        );
        assert!(policy.filesystem_read_roots().is_empty());
        assert!(policy.filesystem_write_roots().is_empty());
    }

    #[test]
    fn the_policy_serializes_read_and_write_roots_as_separate_fields() {
        let read_only = PathBuf::from(if cfg!(windows) {
            r"C:\probelab\read-only"
        } else {
            "/probelab/read-only"
        });
        let write_only = PathBuf::from(if cfg!(windows) {
            r"C:\probelab\write-only"
        } else {
            "/probelab/write-only"
        });
        let mut policy = CapabilityPolicy::deny_all();
        policy
            .grant_filesystem_read_root(&read_only)
            .expect("read grant");
        policy
            .grant_filesystem_write_root(&write_only)
            .expect("write grant");

        let json = serde_json::to_string(&policy).expect("policy JSON");
        assert!(
            json.contains("filesystem_read_roots"),
            "read roots are not serialized: {json}"
        );
        assert!(
            json.contains("filesystem_write_roots"),
            "write roots are not serialized: {json}"
        );
        assert!(
            !json.contains("\"filesystem_roots\""),
            "the merged filesystem root field must be gone: {json}"
        );
        assert_eq!(
            serde_json::from_str::<CapabilityPolicy>(&json).expect("policy round trip"),
            policy
        );
    }

    #[test]
    fn a_read_only_policy_document_grants_no_write() {
        let policy: CapabilityPolicy =
            serde_json::from_str(r#"{"filesystem_read_roots": ["/probelab/read-only"]}"#)
                .expect("read-only policy parses");
        assert_eq!(
            policy.filesystem_read_roots(),
            &[PathBuf::from("/probelab/read-only")]
        );
        assert!(policy.filesystem_write_roots().is_empty());

        let legacy: CapabilityPolicy =
            serde_json::from_str(r#"{"filesystem_roots": ["/probelab/legacy"]}"#)
                .expect("legacy policy still parses");
        assert!(
            legacy.is_deny_all(),
            "an unknown field must be ignored rather than widened into a grant"
        );
    }

    #[test]
    fn environment_inspect_has_no_grant_path() {
        let mut policy = CapabilityPolicy::deny_all();
        assert_eq!(
            policy.grant_permission(CapabilityPermission::EnvironmentInspect),
            Err(CapabilityPolicyError::EnvironmentInspectionUngrantable)
        );
        policy
            .grant_filesystem_root(std::env::temp_dir())
            .expect("filesystem grant");
        policy.grant_process("sh").expect("process grant");
        policy
            .grant_network_endpoint("127.0.0.1:8080")
            .expect("network grant");
        assert_eq!(
            policy.grant_permission(CapabilityPermission::EnvironmentInspect),
            Err(CapabilityPolicyError::EnvironmentInspectionUngrantable)
        );
        assert!(!policy.is_deny_all());
    }

    #[test]
    fn bare_permission_grants_name_no_target_and_are_rejected() {
        let mut policy = CapabilityPolicy::deny_all();
        for permission in [
            CapabilityPermission::FilesystemRead,
            CapabilityPermission::FilesystemWrite,
            CapabilityPermission::ProcessSpawn,
            CapabilityPermission::NetworkConnect,
        ] {
            assert_eq!(
                policy.grant_permission(permission),
                Err(CapabilityPolicyError::ExplicitTargetRequired { permission })
            );
        }
        assert!(policy.is_deny_all());
    }

    #[test]
    fn grants_reject_ambiguous_targets() {
        let mut policy = CapabilityPolicy::deny_all();
        assert_eq!(
            policy.grant_filesystem_root("relative/path"),
            Err(CapabilityPolicyError::InvalidFilesystemRoot)
        );
        assert_eq!(
            policy.grant_process("  "),
            Err(CapabilityPolicyError::InvalidProcessGrant)
        );
        for endpoint in [
            "",
            "example.com",
            "example.com:",
            ":80",
            "example.com:0",
            "example.com:70000",
        ] {
            assert_eq!(
                policy.grant_network_endpoint(endpoint),
                Err(CapabilityPolicyError::InvalidNetworkEndpoint),
                "endpoint {endpoint:?} must be rejected"
            );
        }
        assert!(policy.is_deny_all());
    }

    #[test]
    fn repeated_grants_are_idempotent() {
        let mut policy = CapabilityPolicy::deny_all();
        let root = std::env::temp_dir();
        policy.grant_filesystem_root(&root).expect("grant");
        policy.grant_filesystem_root(&root).expect("repeat");
        policy
            .grant_filesystem_read_root(&root)
            .expect("direction-specific repeat");
        policy
            .grant_filesystem_write_root(&root)
            .expect("direction-specific repeat");
        policy.grant_process("sh").expect("grant");
        policy.grant_process("sh").expect("repeat");
        policy
            .grant_network_endpoint("127.0.0.1:8080")
            .expect("grant");
        policy
            .grant_network_endpoint("127.0.0.1:8080")
            .expect("repeat");
        assert_eq!(policy.filesystem_read_roots(), std::slice::from_ref(&root));
        assert_eq!(policy.filesystem_write_roots(), &[root]);
        assert_eq!(policy.allowed_programs(), &["sh".to_owned()]);
        assert_eq!(policy.allowed_endpoints(), &["127.0.0.1:8080".to_owned()]);
    }

    #[test]
    fn a_policy_round_trips_without_environment_values() {
        let mut policy = CapabilityPolicy::deny_all();
        policy
            .grant_filesystem_read_root(std::env::temp_dir())
            .expect("read grant");
        policy.grant_process("sh").expect("grant");
        let json = serde_json::to_string(&policy).expect("policy JSON");
        assert!(!json.contains("environment"));
        assert!(!json.contains("inspect"));
        assert_eq!(
            serde_json::from_str::<CapabilityPolicy>(&json).expect("policy round trip"),
            policy
        );
    }
}
