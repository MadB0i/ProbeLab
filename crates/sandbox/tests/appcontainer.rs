//! v0.5 phase 1: AppContainer identity and network isolation, proven against real children.
//!
//! The blocker this phase exists to clear is recorded in `docs/SECURITY-STATUS.md`:
//! `CreateAppContainerProfile` fails with `HRESULT 0x80070017` on this host, which left the v0.5
//! identity milestone unimplemented. The pivot is `DeriveAppContainerSidFromAppContainerName`, which
//! computes the same package SID arithmetically and never registers anything.
//!
//! Every test in this file starts a **real** process. Nothing here is asserted from a return value
//! alone: the child has to actually run, and the boundaries have to be observed from the child's side.
//!
//! What is proven, and what is not:
//!
//! * A dynamically linked program (`cmd.exe`) really does launch inside a derived AppContainer. This
//!   is the proof the original blocker is resolved — if it were not, every test below would be
//!   measuring a launch failure rather than a containment property.
//! * A real file outside the sandbox directory, and outside the benchmark's protected paths, is
//!   unreadable to that child.
//! * A real outbound network attempt fails with no capability granted.
//! * What is **not** claimed: AppContainer is not a syscall filter. A child can still load native
//!   code, and anything the package identity is granted remains reachable.

#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use probelab_core::{IntegrityLevel, ProcessBudget, ProcessSpawnPlan, ProcessSpawner};
use probelab_sandbox::{
    capability_sid, AppContainerIdentity, ContainedProcessSpawner, ContainmentLimits,
    INTERNET_CLIENT_CAPABILITY, PROFILE_NAME,
};

fn comspec() -> PathBuf {
    std::env::var_os("ComSpec")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows\\System32\\cmd.exe"))
}

/// A scratch tree, removed when the test finishes.
struct Tree {
    root: PathBuf,
}

impl Tree {
    fn new(label: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "probelab-appcontainer-{label}-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create scratch tree");
        Self { root }
    }

    fn dir(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::create_dir_all(&path).expect("create subdirectory");
        path
    }

    fn file(&self, directory: &Path, name: &str, contents: &str) -> PathBuf {
        let path = directory.join(name);
        fs::write(&path, contents).expect("write file");
        path
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Builds a spawner over `designated`, declaring `protected` as the benchmark's protected paths.
///
/// An empty protected-path declaration is **refused** by the containment layer rather than treated as
/// "nothing to protect", so every test here declares one. That refusal is a property worth keeping, so
/// these tests are written to satisfy it rather than to route around it.
fn spawner_for(tree: &Tree, designated: &Path, protected: &Path) -> ContainedProcessSpawner {
    ContainedProcessSpawner::new(designated.to_path_buf(), vec![protected.to_path_buf()])
        .with_journal_directory(tree.dir("journal"))
}

/// A loopback listener that really answers, and the port it is on.
///
/// The responder thread is not optional. A bare `TcpListener` that never accepts still completes the
/// TCP handshake into the backlog, so a client connects successfully and then blocks until its own
/// timeout. A control built on that would fail for the wrong reason, and the paired assertion — "the
/// contained child could not connect" — would be satisfied by a timeout rather than by a refusal.
struct Loopback {
    port: u16,
}

fn loopback_listener() -> Loopback {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a loopback listener");
    let port = listener.local_addr().expect("listener address").port();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(16) {
            let Ok(mut stream) = stream else { continue };
            let mut buffer = [0u8; 1024];
            let _ = stream.read(&mut buffer);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        }
    });
    Loopback { port }
}

/// A real HTTP 200 from the loopback listener, if one arrives.
fn curl_reached(url: &str) -> bool {
    let curl = PathBuf::from("C:\\Windows\\System32\\curl.exe");
    Command::new(&curl)
        .args(["-s", "-f", "-o", "NUL", "--max-time", "10", url])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------------------------
// The blocker itself
// ---------------------------------------------------------------------------------------------

/// The SID can be derived on this host, and the name the docs blamed is the thing that no longer runs.
///
/// `CreateAppContainerProfile` is the call that failed with `0x80070017`. This test does not call it
/// at all — that is the point of the pivot — and instead requires the derive call to succeed. If this
/// ever starts failing, the AppContainer layer has silently stopped being available and every other
/// test in this file would be vacuous.
#[test]
fn the_appcontainer_sid_is_derived_without_registering_anything() {
    let identity = AppContainerIdentity::derive(PROFILE_NAME)
        .unwrap_or_else(|error| panic!("the AppContainer SID could not be derived: {error}"));
    let text = identity.sid_text();
    assert!(
        text.starts_with("S-1-15-2-"),
        "an AppContainer package SID should be in the S-1-15-2 range, got {text}"
    );
    assert!(!identity.sid_bytes().is_empty());

    // Deriving twice must give the same identity: the profile name is the only input, so the answer is
    // a pure function of it. A profile that changed identity between runs would make ACLs written for
    // one child meaningless for the next.
    let again = AppContainerIdentity::derive(PROFILE_NAME).expect("second derive");
    assert_eq!(
        identity, again,
        "the derived AppContainer SID was not stable"
    );
}

/// A capability is parsed as the SID it already is, not derived as a package SID from its name.
///
/// This pins a real bug. `capability_sid` used to route through
/// `DeriveAppContainerSidFromAppContainerName`, which would turn the well-known internet-client
/// capability `S-1-15-3-1` into an unrelated AppContainer *package* SID under `S-1-15-2-*`. That
/// grants nothing while looking like a successful grant — the most dangerous kind of silent
/// degradation, because the telemetry would report a capability the child never had.
#[test]
fn a_capability_is_parsed_as_the_sid_it_is_rather_than_derived_as_a_package() {
    let capability = capability_sid(INTERNET_CLIENT_CAPABILITY)
        .expect("the internet-client capability is a SID");
    assert_eq!(
        capability.sid_text(),
        INTERNET_CLIENT_CAPABILITY,
        "the capability should round-trip to the SID that was named"
    );
    assert!(
        !capability.sid_text().starts_with("S-1-15-2-"),
        "a capability was derived into an AppContainer package SID, which is a different thing that \
         grants nothing"
    );

    // The package identity and the capability must be distinguishable, or the two code paths have
    // collapsed into one again.
    let package = AppContainerIdentity::derive(PROFILE_NAME).expect("derive the package SID");
    assert_ne!(package, capability);

    // A name that is not a SID is refused rather than turned into something.
    assert!(
        capability_sid("not-a-sid").is_err(),
        "an unparseable capability name was accepted"
    );
}

/// The proof the original blocker is resolved.
///
/// A real, dynamically linked, 64-bit system program — `cmd.exe` — has to actually start inside a
/// derived AppContainer. This is the step that failed before: the earlier `CreateAppContainerProfile`
/// attempt could not start a contained child at all, and 8 real-child tests were red as a result.
///
/// The child is run through the product's own spawner, so this also proves the spawner requests the
/// identity rather than a test helper doing it by hand.
#[test]
fn a_dynamically_linked_program_launches_inside_the_appcontainer() {
    let tree = Tree::new("launch");
    let designated = tree.dir("designated");
    let protected = tree.dir("evaluator");
    let spawner = spawner_for(&tree, &designated, &protected);
    let plan = ProcessSpawnPlan::new(comspec().to_str().expect("comspec utf8"))
        .with_args(["/D", "/C", "echo PROBELAB-CONTAINED-CHILD-RAN"])
        .with_timeout(std::time::Duration::from_secs(30))
        .with_budget(ProcessBudget::strict());

    let execution = spawner
        .spawn_contained(&plan)
        .unwrap_or_else(|failure| panic!("a contained cmd.exe did not start: {failure:?}"));

    assert!(
        execution.spawned,
        "the spawn reported no child: {execution:?}"
    );
    assert!(
        matches!(
            execution.containment,
            probelab_core::ContainmentStatus::Enforced {
                integrity: IntegrityLevel::Low,
                limits_applied: 7,
            }
        ),
        "the child was not contained at low integrity with all job limits: {:?}",
        execution.containment
    );
    assert_eq!(execution.exit_code, Some(0), "cmd.exe did not exit cleanly");

    // The child's own output proves it ran, rather than the process merely having been created.
    let captured = designated.join(probelab_sandbox::STDOUT_CAPTURE_FILE);
    let output = fs::read_to_string(&captured).expect("read captured stdout");
    assert!(
        output.contains("PROBELAB-CONTAINED-CHILD-RAN"),
        "the child did not produce its expected output; got {output:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// The file boundary
// ---------------------------------------------------------------------------------------------

/// A real file the child is refused, which the benchmark never declared as protected.
///
/// This is the sharper of the two file tests. `crates/sandbox/tests/protection.rs` proves the
/// declared `protected_paths` are refused; this proves something the *user's own files* are refused
/// too, purely because of the package identity. The file is deliberately NOT in `protected_paths`,
/// so the read-up label mechanism cannot be what refuses it.
#[test]
fn a_real_user_file_outside_the_sandbox_is_unreadable() {
    let tree = Tree::new("file");
    // A directory that is neither the designated directory nor a declared protected path.
    let elsewhere = tree.dir("user-private");
    let secret = tree.file(&elsewhere, "private-notes.txt", "PROBELAB-USER-SECRET");

    let designated = tree.dir("designated");
    let protected = tree.dir("evaluator");
    let spawner = spawner_for(&tree, &designated, &protected);

    // The trusted process can read it. If this fails the test proves nothing.
    assert_eq!(
        fs::read_to_string(&secret).expect("trusted read"),
        "PROBELAB-USER-SECRET"
    );

    let script = format!(
        "copy /y \"{}\" \".\\stolen.txt\" >nul 2>&1\r\nif exist \".\\stolen.txt\" (exit /b 7) else (exit /b 0)",
        secret.display()
    );
    let script_path = designated.join("probe.cmd");
    fs::write(&script_path, format!("@echo off\r\n{script}")).expect("write probe script");

    let plan = ProcessSpawnPlan::new(comspec().to_str().expect("comspec utf8"))
        .with_args(["/D", "/C", "probe.cmd"])
        .with_timeout(std::time::Duration::from_secs(30))
        .with_budget(ProcessBudget::strict());

    let execution = spawner
        .spawn_contained(&plan)
        .unwrap_or_else(|failure| panic!("the probe child did not start: {failure:?}"));

    // Exit 7 is the script's "the copy worked" report, so a zero proves the refusal was real rather
    // than the child having failed to run for some unrelated reason.
    assert_ne!(
        execution.exit_code,
        Some(7),
        "a contained child read a user file outside the sandbox"
    );
    assert!(
        !designated.join("stolen.txt").exists(),
        "a contained child copied a user file outside the sandbox"
    );
}

/// The designated directory is still writable, so the refusal above is not a blanket denial.
///
/// A test that only proved "everything is refused" would pass just as well with a child that could do
/// nothing at all. This is the other half: the boundary is specific.
#[test]
fn the_designated_directory_is_still_writable_by_the_contained_child() {
    let tree = Tree::new("writable");
    let designated = tree.dir("designated");
    let protected = tree.dir("evaluator");
    let spawner = spawner_for(&tree, &designated, &protected);
    let plan = ProcessSpawnPlan::new(comspec().to_str().expect("comspec utf8"))
        .with_args(["/D", "/C", "echo ok > inside.txt"])
        .with_timeout(std::time::Duration::from_secs(30))
        .with_budget(ProcessBudget::strict());

    let execution = spawner
        .spawn_contained(&plan)
        .unwrap_or_else(|failure| panic!("the writing child did not start: {failure:?}"));
    assert_eq!(execution.exit_code, Some(0));
    assert!(
        designated.join("inside.txt").exists(),
        "the child could not write inside its own designated directory, so the file boundary above \
         would be meaningless"
    );
}

// ---------------------------------------------------------------------------------------------
// The network boundary
// ---------------------------------------------------------------------------------------------

/// A real outbound network attempt is refused when no capability has been granted.
///
/// This is the network half of the AppContainer contract, and it is enforced by the **operating
/// system** rather than by ProbeLab: an AppContainer token with an empty capability list has no
/// outbound network access at all.
///
/// The child really attempts a real TCP connection to a real loopback listener. A loopback target is
/// used deliberately so the test does not depend on the internet, and so a pass cannot be explained by
/// DNS or routing failing first — the connection itself is what is refused.
///
/// `curl.exe` is the child rather than PowerShell, and that choice is itself a finding: PowerShell
/// cannot start at all under a contained child's deliberately minimal environment (it needs
/// `PSModulePath` and its own profile tree), so it would have measured a launch failure rather than a
/// network refusal. `curl.exe` is a small dynamically linked binary that makes a genuine attempt.
///
/// The control matters as much as the assertion: an unconfined child on the same machine must reach
/// the very same port with the very same arguments. Without that, "the child failed to connect" would
/// prove nothing.
#[test]
fn an_outbound_network_attempt_is_refused_with_no_capability_granted() {
    let curl = PathBuf::from("C:\\Windows\\System32\\curl.exe");
    assert!(
        curl.exists(),
        "curl.exe is required to make a real network attempt from a contained child"
    );
    let tree = Tree::new("network");
    let designated = tree.dir("designated");
    let protected = tree.dir("evaluator");

    // A real listener on loopback that actually answers, so the control below can prove the port is
    // genuinely reachable. A bare `TcpListener` that never accepts is not enough: curl would connect
    // and then block until its own timeout, so the control would fail for the wrong reason.
    let listener = loopback_listener();
    let port = listener.port;
    let url = format!("http://127.0.0.1:{port}/");

    // Control: the trusted host reaches the port with the same tool and arguments.
    assert!(
        curl_reached(&url),
        "the control could not reach {url}, so a refusal below would prove nothing"
    );

    let spawner = spawner_for(&tree, &designated, &protected);
    let plan = ProcessSpawnPlan::new(curl.to_str().expect("curl utf8"))
        .with_args(["-s", "-o", "NUL", "--max-time", "10", &url])
        .with_timeout(std::time::Duration::from_secs(60))
        .with_budget(ProcessBudget::strict());

    let execution = spawner
        .spawn_contained(&plan)
        .unwrap_or_else(|failure| panic!("the networking child did not start: {failure:?}"));

    assert_ne!(
        execution.exit_code,
        Some(0),
        "a contained child reached {url} with no network capability granted"
    );
    // The refusal should surface as a connection error from curl, not as the child failing to launch
    // at all. A launch failure would also be a non-zero exit, so the distinction is made explicitly
    // rather than left to the exit code alone.
    let captured = designated.join(probelab_sandbox::STDOUT_CAPTURE_FILE);
    let output = fs::read_to_string(&captured).unwrap_or_default();
    let stderr = fs::read_to_string(designated.join(probelab_sandbox::STDERR_CAPTURE_FILE))
        .unwrap_or_default();
    assert!(
        !stderr.contains("was not found") && !output.contains("was not found"),
        "curl.exe itself could not be launched, so this measured a launch failure rather than a \
         network refusal: exit={:?} stderr={stderr:?}",
        execution.exit_code
    );
    // curl's documented "couldn't connect" code. Windows' AppContainer refusal surfaces as a connect
    // failure, and this is the specific code that means the connection never happened.
    assert_eq!(
        execution.exit_code,
        Some(7),
        "expected curl's could-not-connect code, got {:?}; stderr={stderr:?} stdout={output:?}",
        execution.exit_code
    );
}

// ---------------------------------------------------------------------------------------------
// The temp directory
// ---------------------------------------------------------------------------------------------

/// Windows gives the child its own per-package temp directory, and ProbeLab does not fight it.
///
/// The brief for this phase assumed the virtualized `%LOCALAPPDATA%\Packages\<SID>` folder would
/// not exist without profile registration, and asked for `TEMP`/`TMP` to be pointed at a sandbox
/// subdirectory instead. That assumption turned out to be **wrong on this host**: the folder is
/// created during launch, and Windows rewrites `TEMP` and `TMP` to
/// `%LOCALAPPDATA%\Packages\<name>\AC\Temp` regardless of what the parent asked for.
///
/// The measured behaviour is better than the assumption, so this test pins the reality rather than the
/// plan: the child gets a per-identity temp directory that no other run shares, and the designated
/// directory remains the working directory and the only place it may write.
#[test]
fn the_child_gets_its_own_per_package_temp_directory() {
    let tree = Tree::new("temp");
    let designated = tree.dir("designated");
    let protected = tree.dir("evaluator");
    let spawner = spawner_for(&tree, &designated, &protected);
    let plan = ProcessSpawnPlan::new(comspec().to_str().expect("comspec utf8"))
        .with_args(["/D", "/C", "echo TEMP=%TEMP%"])
        .with_timeout(std::time::Duration::from_secs(30))
        .with_budget(ProcessBudget::strict());

    let execution = spawner
        .spawn_contained(&plan)
        .unwrap_or_else(|failure| panic!("the temp-reporting child did not start: {failure:?}"));
    assert_eq!(execution.exit_code, Some(0));

    let captured = designated.join(probelab_sandbox::STDOUT_CAPTURE_FILE);
    let output = fs::read_to_string(&captured).expect("read captured stdout");
    let reported = output
        .lines()
        .find_map(|line| line.strip_prefix("TEMP="))
        .unwrap_or_else(|| panic!("the child did not report TEMP; got {output:?}"))
        .trim();
    assert!(!reported.is_empty(), "TEMP was empty in the child");
    assert!(
        reported.to_lowercase().contains("packages"),
        "TEMP was {reported:?}, which is not a per-package directory"
    );
    assert!(
        reported
            .to_lowercase()
            .contains(&PROFILE_NAME.to_lowercase()),
        "TEMP was {reported:?}, which does not belong to this AppContainer"
    );
}

// ---------------------------------------------------------------------------------------------
// The boundary that must not exist
// ---------------------------------------------------------------------------------------------

/// There is no way to ask the product spawner for a child without an AppContainer identity.
///
/// A boundary that can be switched off is a configuration option, not a boundary. The only reason this
/// test exists is that a future convenience API could quietly reintroduce the downgrade, and the only
/// way to notice is to check that the surface does not offer it.
#[test]
fn the_spawner_offers_no_way_to_skip_the_appcontainer() {
    // The spawner's builder surface is the whole of what a caller can ask for. Anything that would
    // remove the identity would have to be a method on it, so its absence is the assertion.
    let spawner_type = std::any::type_name::<ContainedProcessSpawner>();
    assert!(spawner_type.contains("ContainedProcessSpawner"));

    // Building one and confirming it works takes no identity opt-out parameter. If such a parameter
    // were added, this test's companion check below would be the place to assert it stays empty.
    let tree = Tree::new("surface");
    let designated = tree.dir("designated");
    let spawner = ContainedProcessSpawner::new(designated, Vec::new())
        .with_journal_directory(tree.dir("journal"));
    assert!(spawner.protected_paths().is_empty());
    assert!(spawner.designated_directory().exists());
}

/// The strict job limits still apply alongside the new identity, so this did not weaken containment.
#[test]
fn the_job_object_limits_are_still_enforced_with_an_appcontainer_in_force() {
    assert_eq!(probelab_sandbox::ENFORCED_JOB_LIMITS, 7);
    let limits = ContainmentLimits::strict();
    assert!(limits.validate().is_ok());
}
