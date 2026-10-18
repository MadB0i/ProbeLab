//! Unit tests for the deny-by-default capability broker.
//!
//! Every test calls the broker directly. No model, agent, engine, telemetry, or filesystem write
//! is involved, so the tests are deterministic and CI-safe. The only platform-dependent test
//! skips itself when the platform refuses to create a symlink.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use probelab_broker::{BrokerError, CapabilityBroker};
use probelab_core::{
    CapabilityDecision, CapabilityDenial, CapabilityPermission, CapabilityPolicy,
    CapabilityRequest, IntegrityEventKind,
};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

/// A disposable directory tree removed when the test finishes.
struct SandboxTree {
    root: PathBuf,
}

impl SandboxTree {
    fn new(label: &str) -> Self {
        let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "probelab-broker-{label}-{}-{sequence}",
            std::process::id()
        ));
        remove_tree(&root);
        fs::create_dir_all(&root).expect("create sandbox root");
        let root = fs::canonicalize(&root).expect("canonical sandbox root");
        Self { root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::create_dir_all(&path).expect("create sandbox directory");
        fs::canonicalize(&path).expect("canonical sandbox directory")
    }

    fn file(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directory");
        }
        fs::write(&path, contents).expect("write sandbox file");
        path
    }
}

impl Drop for SandboxTree {
    fn drop(&mut self) {
        remove_tree(&self.root);
    }
}

fn remove_tree(path: &Path) {
    let Ok(entries) = fs::read_dir(path) else {
        let _ = fs::remove_file(path);
        return;
    };
    for entry in entries.flatten() {
        let candidate = entry.path();
        if candidate.is_dir() {
            remove_tree(&candidate);
        } else {
            let _ = fs::remove_file(&candidate);
        }
    }
    let _ = fs::remove_dir_all(path);
}

fn deny_all_broker(tree: &SandboxTree) -> CapabilityBroker {
    CapabilityBroker::new(CapabilityPolicy::deny_all(), tree.root()).expect("deny-all broker")
}

fn read(path: impl AsRef<Path>) -> CapabilityRequest {
    CapabilityRequest::FilesystemRead {
        path: path.as_ref().to_path_buf(),
    }
}

fn write_to(path: impl AsRef<Path>) -> CapabilityRequest {
    CapabilityRequest::FilesystemWrite {
        path: path.as_ref().to_path_buf(),
    }
}

fn spawn(program: &str) -> CapabilityRequest {
    CapabilityRequest::ProcessSpawn {
        program: program.to_owned(),
        args: vec!["-c".to_owned(), "echo probe".to_owned()],
    }
}

fn spawn_path(program: &Path) -> CapabilityRequest {
    spawn(&program.display().to_string())
}

fn connect(endpoint: &str) -> CapabilityRequest {
    CapabilityRequest::NetworkConnect {
        endpoint: endpoint.to_owned(),
    }
}

fn inspect(variable: &str) -> CapabilityRequest {
    CapabilityRequest::EnvironmentInspect {
        variable: variable.to_owned(),
    }
}

fn assert_denied(
    decision: CapabilityDecision,
    permission: CapabilityPermission,
    event: IntegrityEventKind,
) {
    let denial = decision.denial().expect("request must be denied").clone();
    assert_eq!(denial.permission, permission);
    assert_eq!(denial.integrity_event, event);
    assert!(!denial.reason.is_empty());
}

fn requests_for(path: impl AsRef<Path>) -> [CapabilityRequest; 2] {
    let path = path.as_ref().to_path_buf();
    [read(&path), write_to(&path)]
}

#[test]
fn deny_all_broker_denies_every_permission_category() {
    let tree = SandboxTree::new("deny-all");
    let broker = deny_all_broker(&tree);
    let inside = tree.file("inside.txt", "trusted");
    let cases = [
        (
            read(&inside),
            CapabilityPermission::FilesystemRead,
            IntegrityEventKind::BrokerUnauthorizedRead,
        ),
        (
            write_to(&inside),
            CapabilityPermission::FilesystemWrite,
            IntegrityEventKind::BrokerUnauthorizedWrite,
        ),
        (
            spawn("sh"),
            CapabilityPermission::ProcessSpawn,
            IntegrityEventKind::BrokerProcessSpawnBlocked,
        ),
        (
            connect("127.0.0.1:8080"),
            CapabilityPermission::NetworkConnect,
            IntegrityEventKind::BrokerNetworkAttempt,
        ),
        (
            inspect("PATH"),
            CapabilityPermission::EnvironmentInspect,
            IntegrityEventKind::BrokerEnvironmentInspectDenied,
        ),
    ];
    assert_eq!(cases.len(), CapabilityPermission::ALL.len());
    for (request, permission, event) in cases {
        assert_denied(broker.review(&request), permission, event);
    }
    assert!(broker.policy().is_deny_all());
}

#[test]
fn benchmark_and_evaluator_paths_are_denied() {
    let tree = SandboxTree::new("trusted-sources");
    let broker = deny_all_broker(&tree);
    let trusted = tree.root().parent().expect("parent of the sandbox root");

    for relative in [
        "crates/evaluator/src/lib.rs",
        "crates/benchmarks/src/lib.rs",
        "Cargo.toml",
    ] {
        for request in requests_for(trusted.join(relative)) {
            assert!(
                !broker.review(&request).is_allowed(),
                "{relative} was allowed"
            );
        }
    }
    for request in requests_for("crates/evaluator/src/lib.rs") {
        assert!(!broker.review(&request).is_allowed());
    }
    let workspace_evaluator = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("crates/evaluator/src/lib.rs");
    for request in requests_for(workspace_evaluator) {
        assert!(!broker.review(&request).is_allowed());
    }
}

#[test]
fn parent_directory_traversal_is_denied() {
    let tree = SandboxTree::new("traversal");
    let broker = deny_all_broker(&tree);
    let traversal = [
        "../secrets.txt",
        "../../..",
        "../../../etc/passwd",
        "data/../../../escape.txt",
        "..\\..\\secrets.txt",
        "data\\..\\..\\escape.txt",
    ];
    for relative in traversal {
        for request in requests_for(relative) {
            assert_denied(
                broker.review(&request),
                request.permission(),
                match request.permission() {
                    CapabilityPermission::FilesystemWrite => {
                        IntegrityEventKind::BrokerUnauthorizedWrite
                    }
                    _ => IntegrityEventKind::BrokerUnauthorizedRead,
                },
            );
        }
    }
}

#[test]
fn absolute_paths_outside_the_root_are_denied() {
    let tree = SandboxTree::new("absolute");
    let broker = deny_all_broker(&tree);
    let outside = tree.root().parent().expect("parent of the sandbox root");
    for request in requests_for(outside.join("outside.txt")) {
        assert!(!broker.review(&request).is_allowed());
    }
    for request in requests_for(outside) {
        assert!(!broker.review(&request).is_allowed());
    }
    for request in requests_for(std::env::temp_dir()) {
        assert!(!broker.review(&request).is_allowed());
    }
}

#[cfg(windows)]
fn create_directory_symlink(link: &Path, target: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(unix)]
fn create_directory_symlink(link: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[test]
fn a_symlink_out_of_the_root_is_denied() {
    let tree = SandboxTree::new("symlink");
    let broker = deny_all_broker(&tree);
    let secret_directory = tree.root().parent().expect("parent of the sandbox root");
    let secret = secret_directory.join("broker-symlink-secret.txt");
    fs::write(&secret, "trusted").expect("write trusted file");
    let link = tree.root().join("escape");
    if create_directory_symlink(&link, &secret).is_err() {
        // Some CI platforms refuse symlink creation for unprivileged accounts; the
        // canonicalization rule under test is unchanged, so the check is simply skipped.
        let _ = fs::remove_file(&secret);
        return;
    }

    for request in [
        read(&link),
        read(link.join("broker-symlink-secret.txt")),
        read("escape"),
        read("escape/broker-symlink-secret.txt"),
        write_to(&link),
        write_to("escape/new-file.txt"),
    ] {
        assert!(
            !broker.review(&request).is_allowed(),
            "symlinked path {:?} was allowed",
            request.permission()
        );
    }
    let _ = fs::remove_file(&link);
    let _ = fs::remove_file(&secret);
}

#[test]
fn writes_outside_the_root_are_denied_even_when_reads_would_be_granted() {
    let tree = SandboxTree::new("write-outside");
    let granted = tree.dir("granted");
    let mut policy = CapabilityPolicy::deny_all();
    policy
        .grant_filesystem_read_root(&granted)
        .expect("grant read root");
    let broker = CapabilityBroker::new(policy, tree.root()).expect("broker");

    assert!(broker.review(&read(granted.join("notes.txt"))).is_allowed());
    let outside = tree.root().parent().expect("parent of the sandbox root");
    assert_denied(
        broker.review(&write_to(outside.join("escape.txt"))),
        CapabilityPermission::FilesystemWrite,
        IntegrityEventKind::BrokerUnauthorizedWrite,
    );
    assert_denied(
        broker.review(&write_to("../escape.txt")),
        CapabilityPermission::FilesystemWrite,
        IntegrityEventKind::BrokerUnauthorizedWrite,
    );
    assert_denied(
        broker.review(&write_to("sibling/escape.txt")),
        CapabilityPermission::FilesystemWrite,
        IntegrityEventKind::BrokerUnauthorizedWrite,
    );
    // The refusal is a decision only: the broker created no file anywhere.
    assert!(!outside.join("escape.txt").exists());
    assert!(!tree.root().join("sibling/escape.txt").exists());
}

#[test]
fn an_explicit_inside_root_grant_allows_reads_and_writes() {
    let tree = SandboxTree::new("inside-grant");
    let granted = tree.dir("workspace/data");
    let existing = tree.file("workspace/data/notes.txt", "notes");
    let mut policy = CapabilityPolicy::deny_all();
    policy.grant_filesystem_root(&granted).expect("grant root");
    let broker = CapabilityBroker::new(policy, tree.root()).expect("broker");

    assert!(broker.review(&read(&existing)).is_allowed());
    assert!(broker
        .review(&read("workspace/data/notes.txt"))
        .is_allowed());
    assert!(broker.review(&read("workspace/data")).is_allowed());
    // A write target that does not exist yet still resolves inside the granted root.
    assert!(broker
        .review(&write_to("workspace/data/new.txt"))
        .is_allowed());
    assert!(broker.review(&write_to(&granted)).is_allowed());

    // A sibling inside the sandbox root but outside the granted root stays denied.
    let ungranted = tree.file("workspace/secret.txt", "secret");
    assert_denied(
        broker.review(&read(&ungranted)),
        CapabilityPermission::FilesystemRead,
        IntegrityEventKind::BrokerUnauthorizedRead,
    );
    assert_denied(
        broker.review(&write_to("workspace/new.txt")),
        CapabilityPermission::FilesystemWrite,
        IntegrityEventKind::BrokerUnauthorizedWrite,
    );
    assert!(!tree.root().join("workspace/data/new.txt").exists());
}

#[test]
fn a_read_only_grant_allows_reads_and_refuses_every_write() {
    let tree = SandboxTree::new("read-only");
    let granted = tree.dir("workspace/data");
    let existing = tree.file("workspace/data/notes.txt", "notes");
    let mut policy = CapabilityPolicy::deny_all();
    policy
        .grant_filesystem_read_root(&granted)
        .expect("grant read root");
    let broker = CapabilityBroker::new(policy, tree.root()).expect("broker");

    // The read grant authorizes in-root reads, absolute and sandbox-relative alike.
    assert!(broker.review(&read(&existing)).is_allowed());
    assert!(broker
        .review(&read("workspace/data/notes.txt"))
        .is_allowed());
    assert!(broker.review(&read("workspace/data")).is_allowed());

    // The same root authorizes no write, inside the granted root or anywhere else.
    for request in [
        write_to(&existing),
        write_to("workspace/data/notes.txt"),
        write_to("workspace/data/new.txt"),
        write_to(&granted),
        write_to(tree.root()),
    ] {
        assert_denied(
            broker.review(&request),
            CapabilityPermission::FilesystemWrite,
            IntegrityEventKind::BrokerUnauthorizedWrite,
        );
    }
    assert!(!tree.root().join("workspace/data/new.txt").exists());

    // A sibling inside the sandbox root but outside the granted root stays denied in both
    // directions.
    let ungranted = tree.file("workspace/secret.txt", "secret");
    assert_denied(
        broker.review(&read(&ungranted)),
        CapabilityPermission::FilesystemRead,
        IntegrityEventKind::BrokerUnauthorizedRead,
    );
    assert_denied(
        broker.review(&write_to("workspace/new.txt")),
        CapabilityPermission::FilesystemWrite,
        IntegrityEventKind::BrokerUnauthorizedWrite,
    );
}

#[test]
fn a_write_only_grant_allows_writes_and_refuses_every_read() {
    let tree = SandboxTree::new("write-only");
    let granted = tree.dir("workspace/output");
    let existing = tree.file("workspace/output/notes.txt", "notes");
    let mut policy = CapabilityPolicy::deny_all();
    policy
        .grant_filesystem_write_root(&granted)
        .expect("grant write root");
    let broker = CapabilityBroker::new(policy, tree.root()).expect("broker");

    // The write grant authorizes in-root writes, including a target that does not exist yet.
    assert!(broker
        .review(&write_to("workspace/output/new.txt"))
        .is_allowed());
    assert!(broker.review(&write_to(&existing)).is_allowed());
    assert!(broker.review(&write_to(&granted)).is_allowed());

    // The same root authorizes no read, inside the granted root or anywhere else.
    for request in [
        read(&existing),
        read("workspace/output/notes.txt"),
        read("workspace/output"),
        read(&granted),
        read(tree.root()),
    ] {
        assert_denied(
            broker.review(&request),
            CapabilityPermission::FilesystemRead,
            IntegrityEventKind::BrokerUnauthorizedRead,
        );
    }

    // A sibling inside the sandbox root but outside the granted root stays denied in both
    // directions.
    let ungranted = tree.file("workspace/secret.txt", "secret");
    assert_denied(
        broker.review(&read(&ungranted)),
        CapabilityPermission::FilesystemRead,
        IntegrityEventKind::BrokerUnauthorizedRead,
    );
    assert_denied(
        broker.review(&write_to("workspace/new.txt")),
        CapabilityPermission::FilesystemWrite,
        IntegrityEventKind::BrokerUnauthorizedWrite,
    );
}

#[test]
fn a_read_root_and_a_write_root_are_authorized_independently() {
    let tree = SandboxTree::new("split-roots");
    let readable = tree.dir("workspace/readable");
    let writable = tree.dir("workspace/writable");
    let existing = tree.file("workspace/readable/notes.txt", "notes");
    let mut policy = CapabilityPolicy::deny_all();
    policy
        .grant_filesystem_read_root(&readable)
        .expect("grant read root");
    policy
        .grant_filesystem_write_root(&writable)
        .expect("grant write root");
    let broker = CapabilityBroker::new(policy, tree.root()).expect("broker");

    assert_eq!(broker.granted_read_roots(), std::slice::from_ref(&readable));
    assert_eq!(
        broker.granted_write_roots(),
        std::slice::from_ref(&writable)
    );

    // Each direction is authorized for its own root only.
    assert!(broker.review(&read(&existing)).is_allowed());
    assert_denied(
        broker.review(&read("workspace/writable")),
        CapabilityPermission::FilesystemRead,
        IntegrityEventKind::BrokerUnauthorizedRead,
    );
    assert!(broker
        .review(&write_to("workspace/writable/result.txt"))
        .is_allowed());
    assert_denied(
        broker.review(&write_to("workspace/readable/notes.txt")),
        CapabilityPermission::FilesystemWrite,
        IntegrityEventKind::BrokerUnauthorizedWrite,
    );

    // Adding the missing direction for one root never disturbs the other root or direction.
    let mut widened = broker.policy().clone();
    widened
        .grant_filesystem_root(&readable)
        .expect("grant both directions for the readable root");
    let widened = CapabilityBroker::new(widened, tree.root()).expect("broker");
    assert!(widened.review(&read(&existing)).is_allowed());
    assert!(widened
        .review(&write_to("workspace/readable/new.txt"))
        .is_allowed());
    assert_eq!(
        widened.granted_read_roots(),
        std::slice::from_ref(&readable)
    );
    assert_eq!(widened.granted_write_roots(), &[writable.clone(), readable]);
    assert_denied(
        widened.review(&read("workspace/writable")),
        CapabilityPermission::FilesystemRead,
        IntegrityEventKind::BrokerUnauthorizedRead,
    );
    assert_denied(
        widened.review(&write_to("workspace/new.txt")),
        CapabilityPermission::FilesystemWrite,
        IntegrityEventKind::BrokerUnauthorizedWrite,
    );
}

#[test]
fn a_whole_root_grant_cannot_be_widened_by_a_prefix_lookalike() {
    let tree = SandboxTree::new("prefix-lookalike");
    let granted = tree.dir("data");
    fs::create_dir_all(tree.root().join("data-backup")).expect("create lookalike directory");
    let mut policy = CapabilityPolicy::deny_all();
    policy.grant_filesystem_root(&granted).expect("grant root");
    let broker = CapabilityBroker::new(policy, tree.root()).expect("broker");

    let lookalike = tree.root().join("data-backup/notes.txt");
    fs::write(&lookalike, "lookalike").expect("write lookalike file");
    assert!(!broker.review(&read(&lookalike)).is_allowed());
    assert!(!broker.review(&read("data-backup/notes.txt")).is_allowed());
}

#[test]
fn the_process_allowlist_gates_every_spawn() {
    let tree = SandboxTree::new("process");
    let mut policy = CapabilityPolicy::deny_all();
    policy
        .grant_process("allowlisted-tool")
        .expect("grant program");
    let broker = CapabilityBroker::new(policy, tree.root()).expect("broker");

    assert!(broker.review(&spawn("allowlisted-tool")).is_allowed());
    assert_denied(
        broker.review(&spawn("sh")),
        CapabilityPermission::ProcessSpawn,
        IntegrityEventKind::BrokerProcessSpawnBlocked,
    );
    assert_denied(
        broker.review(&spawn("")),
        CapabilityPermission::ProcessSpawn,
        IntegrityEventKind::BrokerProcessSpawnBlocked,
    );
    assert_denied(
        broker.review(&spawn("allowlisted-tool.exe")),
        CapabilityPermission::ProcessSpawn,
        IntegrityEventKind::BrokerProcessSpawnBlocked,
    );
    // A path-shaped request never falls back to the bare-name grant.
    for program in [
        "./allowlisted-tool",
        "../bin/allowlisted-tool",
        "..\\bin\\allowlisted-tool",
        "/usr/bin/allowlisted-tool",
    ] {
        assert!(
            !broker.review(&spawn(program)).is_allowed(),
            "path {program:?} matched a bare-name grant"
        );
    }
    assert!(!broker.program_allowed("sh"));
    assert!(broker.program_allowed("allowlisted-tool"));
}

#[test]
fn an_absolute_program_grant_requires_the_same_canonical_path() {
    let tree = SandboxTree::new("process-path");
    let tool = tree.file("tools/tool", "#!/bin/sh\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755))
            .expect("make tool executable");
    }
    let mut policy = CapabilityPolicy::deny_all();
    policy
        .grant_process(tool.display().to_string())
        .expect("grant program path");
    let broker = CapabilityBroker::new(policy, tree.root()).expect("broker");

    assert!(broker.review(&spawn_path(&tool)).is_allowed());
    assert!(!broker.review(&spawn("tool")).is_allowed());
    // A lexical path that canonicalizes to the granted program names that same program, so it
    // is authorized; a different program in the same directory is not.
    let mut traversal = tree.root().to_path_buf();
    traversal.push("tools");
    traversal.push("..");
    traversal.push("tools");
    traversal.push("tool");
    assert!(broker.review(&spawn_path(&traversal)).is_allowed());
    tree.file("tools/other-tool", "#!/bin/sh\n");
    assert!(!broker
        .review(&spawn_path(&tree.root().join("tools/other-tool")))
        .is_allowed());
}

#[test]
fn the_network_allowlist_gates_every_connect() {
    let tree = SandboxTree::new("network");
    let mut policy = CapabilityPolicy::deny_all();
    policy
        .grant_network_endpoint("127.0.0.1:8080")
        .expect("grant endpoint");
    let broker = CapabilityBroker::new(policy, tree.root()).expect("broker");

    assert!(broker.review(&connect("127.0.0.1:8080")).is_allowed());
    assert!(broker.endpoint_allowed("127.0.0.1:8080"));
    for endpoint in [
        "127.0.0.1:8081",
        "127.0.0.2:8080",
        "localhost:8080",
        "example.com:443",
        "",
    ] {
        assert_denied(
            broker.review(&connect(endpoint)),
            CapabilityPermission::NetworkConnect,
            IntegrityEventKind::BrokerNetworkAttempt,
        );
    }
}

#[test]
fn environment_inspection_is_denied_even_for_a_malformed_policy() {
    let tree = SandboxTree::new("environment");
    // A policy document that tries to grant environment inspection by an unknown field. Serde
    // ignores the unknown field, so the policy parses, and the broker must still refuse.
    let policy: CapabilityPolicy = serde_json::from_str(
        r#"{
            "filesystem_read_roots": [],
            "filesystem_write_roots": [],
            "allowed_programs": ["sh"],
            "allowed_endpoints": ["127.0.0.1:8080"],
            "environment_inspect_grants": ["*"],
            "environment": {"inspect": true}
        }"#,
    )
    .expect("malformed policy still parses");
    let broker = CapabilityBroker::new(policy, tree.root()).expect("broker");

    for variable in ["PATH", "SECRET", "", "PROBELAB_TEST_SECRET"] {
        assert_denied(
            broker.review(&inspect(variable)),
            CapabilityPermission::EnvironmentInspect,
            IntegrityEventKind::BrokerEnvironmentInspectDenied,
        );
    }
    // The refusals stay value-free: no variable name and no value is echoed anywhere.
    for variable in ["PATH", "PROBELAB_TEST_SECRET"] {
        let decision = broker.review(&inspect(variable));
        let serialized = serde_json::to_string(&decision).expect("decision JSON");
        assert!(!serialized.contains(variable));
    }
}

#[test]
fn a_malformed_or_missing_root_fails_construction_instead_of_guessing() {
    let tree = SandboxTree::new("construction");
    assert!(matches!(
        CapabilityBroker::new(CapabilityPolicy::deny_all(), "relative/root"),
        Err(BrokerError::RelativeRoot)
    ));
    assert!(matches!(
        CapabilityBroker::new(CapabilityPolicy::deny_all(), ""),
        Err(BrokerError::RelativeRoot)
    ));
    let missing = tree.root().join("does-not-exist");
    assert!(matches!(
        CapabilityBroker::new(CapabilityPolicy::deny_all(), &missing),
        Err(BrokerError::RootUnavailable(_))
    ));
    let file = tree.file("not-a-directory", "file");
    assert!(matches!(
        CapabilityBroker::new(CapabilityPolicy::deny_all(), &file),
        Err(BrokerError::RootNotDirectory)
    ));
    let mut outside = CapabilityPolicy::deny_all();
    outside
        .grant_filesystem_read_root(std::env::temp_dir())
        .expect("declare read grant");
    assert!(matches!(
        CapabilityBroker::new(outside, tree.root()),
        Err(BrokerError::FilesystemReadGrantOutsideRoot(_))
    ));
    let mut outside_write = CapabilityPolicy::deny_all();
    outside_write
        .grant_filesystem_write_root(std::env::temp_dir())
        .expect("declare write grant");
    assert!(matches!(
        CapabilityBroker::new(outside_write, tree.root()),
        Err(BrokerError::FilesystemWriteGrantOutsideRoot(_))
    ));
    let mut missing_grant = CapabilityPolicy::deny_all();
    missing_grant
        .grant_filesystem_read_root(tree.root().join("missing"))
        .expect("declare grant");
    assert!(matches!(
        CapabilityBroker::new(missing_grant, tree.root()),
        Err(BrokerError::FilesystemReadGrantUnavailable(_))
    ));
    let mut missing_write_grant = CapabilityPolicy::deny_all();
    missing_write_grant
        .grant_filesystem_write_root(tree.root().join("missing"))
        .expect("declare grant");
    assert!(matches!(
        CapabilityBroker::new(missing_write_grant, tree.root()),
        Err(BrokerError::FilesystemWriteGrantUnavailable(_))
    ));
    let mut missing_program = CapabilityPolicy::deny_all();
    missing_program
        .grant_process(tree.root().join("missing-tool").display().to_string())
        .expect("declare grant");
    assert!(matches!(
        CapabilityBroker::new(missing_program, tree.root()),
        Err(BrokerError::ProcessGrantUnavailable(_))
    ));
}

#[test]
fn denied_requests_never_panic() {
    let tree = SandboxTree::new("hostile");
    let broker = deny_all_broker(&tree);
    let hostile_paths = [
        "",
        ".",
        "..",
        "/",
        "//",
        "///",
        "\\\\",
        "\\\\?\\C:\\Windows",
        "\\\\.\\PhysicalDrive0",
        "C:\\Windows\\System32",
        "c:\\windows\\system32\\config",
        "/dev/null",
        "/proc/self/environ",
        "data\0inside",
        "\u{0}",
        "é中文/файл.txt",
        "....//....//escape",
        "././././../../escape",
        "a/./b/../c",
    ];
    for path in hostile_paths {
        for request in requests_for(path) {
            let decision = broker.review(&request);
            assert!(!decision.is_allowed(), "hostile path {path:?} was allowed");
            assert!(matches!(
                decision,
                CapabilityDecision::Deny(CapabilityDenial { .. })
            ));
        }
    }
    for program in [
        "",
        " ",
        "/",
        "..",
        "\\",
        "a\0b",
        "sh -c whoami",
        "%COMSPEC%",
    ] {
        assert!(!broker.review(&spawn(program)).is_allowed());
    }
    for endpoint in [
        "",
        ":",
        "::",
        "host",
        "host:",
        ":80",
        "1.2.3.4:99999",
        "host:80 ",
    ] {
        assert!(!broker.review(&connect(endpoint)).is_allowed());
    }
    for variable in ["", " ", "=", "A=B", "\u{0}"] {
        assert!(!broker.review(&inspect(variable)).is_allowed());
    }
}

#[test]
fn a_denial_never_carries_a_requested_value() {
    let tree = SandboxTree::new("value-free");
    let broker = deny_all_broker(&tree);
    let secret_path = tree
        .root()
        .parent()
        .expect("parent")
        .join("secret-value.txt");
    let requests = [
        read(&secret_path),
        write_to(&secret_path),
        spawn("sh"),
        connect("example.com:443"),
        inspect("PROBELAB_TEST_SECRET"),
    ];
    for request in requests {
        let decision = broker.review(&request);
        let serialized = serde_json::to_string(&decision).expect("decision JSON");
        for fragment in [
            "secret-value",
            "example.com",
            "sh",
            "PROBELAB_TEST_SECRET",
            &tree.root().display().to_string(),
        ] {
            assert!(
                !serialized.contains(fragment),
                "denial leaked {fragment:?}: {serialized}"
            );
        }
    }
}

#[test]
fn the_broker_canonicalizes_its_root_once_and_exposes_it() {
    let tree = SandboxTree::new("canonical-root");
    // The indirect root deliberately travels through `data/..`, so `data` has to exist for the path to
    // be traversable at all. `fs::canonicalize` resolves every component against the filesystem and
    // reports ENOENT for a missing one, so without this the broker is handed a root that does not exist
    // and the test fails before it reaches the canonicalization it is actually about. Windows tolerates
    // the non-existent intermediate when normalizing a path, which is why this only ever failed on the
    // runners that compile this natively.
    fs::create_dir_all(tree.root().join("data"))
        .expect("create the intermediate the indirect root needs");
    let indirect = tree.root().join("data/..");
    let broker = deny_all_broker(&tree);
    assert_eq!(
        broker.canonical_root(),
        fs::canonicalize(tree.root()).unwrap()
    );

    let mut policy = CapabilityPolicy::deny_all();
    policy.grant_filesystem_root(&indirect).expect("grant");
    let with_grant = CapabilityBroker::new(policy, &indirect).expect("broker");
    assert_eq!(with_grant.canonical_root(), broker.canonical_root());
}
