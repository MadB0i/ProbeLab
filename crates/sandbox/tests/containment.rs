//! Tests for the Windows OS-level defense-in-depth containment layer.
//!
//! The Windows tests spawn a real child process, `%ComSpec%`, under a real job object with a real
//! low-integrity token, and assert what the operating system did to it. Each child runs a batch
//! script that the test wrote into the designated sandbox directory and starts with its own name
//! as the argument, so no command-line quoting is involved and the assertions cannot be an
//! artifact of how a path happens to be spelled.
//!
//! Every test is deterministic: paths live under a unique temporary directory, the scripts are
//! fixed, and every temporary file and directory is removed when the test finishes.
//!
//! The non-Windows tests assert only what is true everywhere: that a platform without this
//! implementation reports `ContainmentError::UnsupportedPlatform` instead of pretending to
//! contain anything.

use probelab_sandbox::{
    ContainmentError, ContainmentLimits, RestrictedProcessGuard, RestrictedProcessSpec,
};

#[test]
fn a_spec_is_data_and_never_starts_anything() {
    let spec = RestrictedProcessSpec::new("cmd.exe")
        .with_args(["/D", "/S", "/C"])
        .with_arg("echo probe")
        .with_env("PROBELAB_PROBE", "1");
    assert_eq!(spec.program(), "cmd.exe");
    assert_eq!(spec.args().len(), 4);
    assert_eq!(spec.environment().len(), 1);
    assert!(spec.current_directory().is_none());
}

#[test]
fn an_impossible_limit_is_refused_before_any_process_is_considered() {
    let refused = RestrictedProcessGuard::spawn(
        RestrictedProcessSpec::new("cmd.exe"),
        &ContainmentLimits::strict().with_active_processes(0),
    );
    assert!(matches!(
        refused.err(),
        Some(ContainmentError::ZeroLimit("active_processes"))
    ));
}

#[test]
fn a_spec_without_a_program_is_refused_everywhere() {
    let refused =
        RestrictedProcessGuard::spawn(RestrictedProcessSpec::new(""), &ContainmentLimits::strict());
    assert!(matches!(
        refused.err(),
        Some(ContainmentError::EmptyProgram)
    ));
}

#[cfg(not(windows))]
mod unsupported {
    use super::ContainmentLimits;
    use probelab_sandbox::{
        is_process_alive, read_integrity, ContainmentError, RestrictedProcessGuard,
        RestrictedProcessSpec, SandboxDirectory,
    };

    #[test]
    fn a_platform_without_os_containment_says_so_instead_of_pretending() {
        let spawn = RestrictedProcessGuard::spawn(
            RestrictedProcessSpec::new("cmd.exe"),
            &ContainmentLimits::strict(),
        );
        assert!(matches!(
            spawn.err(),
            Some(ContainmentError::UnsupportedPlatform)
        ));

        let directory = SandboxDirectory::prepare("designated");
        assert!(matches!(
            directory.err(),
            Some(ContainmentError::UnsupportedPlatform)
        ));

        assert!(matches!(
            read_integrity("designated").err(),
            Some(ContainmentError::UnsupportedPlatform)
        ));
        assert!(matches!(
            is_process_alive(4).err(),
            Some(ContainmentError::UnsupportedPlatform)
        ));
    }
}

#[cfg(windows)]
mod enforced {
    use probelab_sandbox::{
        is_process_alive, process_integrity, read_integrity, ContainmentError, ContainmentLimits,
        MandatoryIntegrity, RestrictedProcessGuard, RestrictedProcessSpec, SandboxDirectory,
        LOW_INTEGRITY_SID,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

    /// A unique temporary directory tree that is removed when the test finishes.
    struct TempTree {
        root: PathBuf,
    }

    impl TempTree {
        fn new(label: &str) -> Self {
            let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "probelab-containment-{label}-{}-{sequence}",
                std::process::id()
            ));
            remove_tree(&root);
            fs::create_dir_all(&root).expect("create temporary tree");
            let root = plain_path(fs::canonicalize(&root).expect("canonical temporary tree"));
            Self { root }
        }

        fn root(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            remove_tree(&self.root);
        }
    }

    /// Removes the `\\?\` prefix a canonical Windows path carries.
    ///
    /// A verbatim path is a device path rather than a drive path, and a child process that is given
    /// one as its working directory cannot use it: `cmd.exe` reports that UNC paths are unsupported
    /// and falls back to its own directory. Every path handed to a contained child is therefore a
    /// plain drive path.
    fn plain_path(path: PathBuf) -> PathBuf {
        let text = path.to_string_lossy();
        match text.strip_prefix(r"\\?\") {
            Some(drive_path) if !drive_path.starts_with("UNC\\") => PathBuf::from(drive_path),
            _ => path,
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

    /// Returns true when `condition` becomes true within `budget`, polling every ten milliseconds.
    fn eventually(mut condition: impl FnMut() -> bool, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        loop {
            if condition() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The environment variable the test child reads its inside path from.
    const INSIDE: &str = "PROBELAB_CONTAINMENT_INSIDE";
    /// The environment variable the test child reads its outside path from.
    const OUTSIDE: &str = "PROBELAB_CONTAINMENT_OUTSIDE";
    /// The exit code the test child reports when the outside file was created anyway.
    const ESCAPED: u32 = 0;
    /// The exit code the test child reports when the outside file was never created.
    const CONTAINED: u32 = 7;

    /// A script that attempts to write one file inside the designated directory and one outside
    /// it, then reports whether the outside file exists. The paths are quoted inside the script
    /// file itself, so nothing about the child depends on how a path is spelled, and both writes
    /// are attempted before the report so a successful outside write cannot be mistaken for a
    /// silent success.
    const WRITE_INSIDE_AND_OUTSIDE: &str = "echo contained >\"%PROBELAB_CONTAINMENT_INSIDE%\"\r\necho escaped >\"%PROBELAB_CONTAINMENT_OUTSIDE%\"\r\nif exist \"%PROBELAB_CONTAINMENT_OUTSIDE%\" exit /b 0\r\nexit /b 7";

    /// A script that writes one file and then spins far longer than any test is willing to wait.
    const WRITE_THEN_SPIN: &str =
        "echo started >\"%PROBELAB_CONTAINMENT_INSIDE%\"\r\nfor /l %%i in (1,1,100000000) do @rem";

    fn comspec() -> PathBuf {
        std::env::var_os("ComSpec")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows\\System32\\cmd.exe"))
    }

    /// Writes a child script into the designated directory and returns its bare file name.
    ///
    /// The child is started in that directory and receives only the file name, so the command line
    /// contains no quoted path and no escape sequence.
    fn write_child_script(directory: &Path, name: &str, body: &str) -> String {
        fs::write(directory.join(name), format!("@echo off\r\n{body}"))
            .expect("write child script");
        name.to_owned()
    }

    /// Builds the contained child that runs `script` from inside `directory`.
    fn contained_child(
        directory: &Path,
        script: &str,
        paths: &[(&str, &Path)],
    ) -> RestrictedProcessSpec {
        let mut spec = RestrictedProcessSpec::new(comspec())
            .with_args(["/D", "/C", script])
            .with_current_directory(directory);
        for (key, value) in paths {
            spec = spec.with_env(*key, value.as_os_str());
        }
        spec
    }

    #[test]
    fn a_low_integrity_child_writes_inside_the_designated_directory_and_nowhere_else() {
        let tree = TempTree::new("escape");
        let designated = SandboxDirectory::prepare(tree.root().join("designated"))
            .expect("prepare the designated sandbox directory");
        let inside = designated.path().join("inside.txt");
        let outside = tree.root().join("outside.txt");
        let script = write_child_script(
            designated.path(),
            "write-inside-and-outside.cmd",
            WRITE_INSIDE_AND_OUTSIDE,
        );

        assert_eq!(
            designated.integrity().expect("designated label"),
            MandatoryIntegrity::Low,
            "the designated directory is not low integrity"
        );
        let parent = tree.root().parent().expect("parent of the tree");
        assert!(
            !read_integrity(parent).expect("parent label").is_low(),
            "the parent must be above the low integrity level for this test to mean anything"
        );

        let mut guard = RestrictedProcessGuard::spawn(
            contained_child(
                designated.path(),
                &script,
                &[(INSIDE, inside.as_path()), (OUTSIDE, outside.as_path())],
            ),
            &ContainmentLimits::strict(),
        )
        .expect("start a contained child");
        let outcome = guard.wait().expect("the contained child exits");

        assert!(
            inside.exists(),
            "the low-integrity child could not write inside the designated directory"
        );
        assert_eq!(
            read_integrity(&inside).expect("inside file label"),
            MandatoryIntegrity::Low,
            "the file the child created is not labeled low, so the child did not run at low integrity"
        );
        assert!(
            !outside.exists(),
            "the low-integrity child escaped the designated directory and wrote outside it"
        );
        assert_ne!(
            outcome.exit_code(),
            ESCAPED,
            "the child reported that it wrote outside the designated directory"
        );
        assert_eq!(
            outcome.exit_code(),
            CONTAINED,
            "the child did not report a contained result"
        );

        fs::write(&outside, "control")
            .expect("the trusted parent can write the path the child could not");
        assert!(outside.exists());
        fs::remove_file(&outside).expect("remove the control file");
    }

    #[test]
    fn a_designated_directory_is_labeled_low_and_protected() {
        let tree = TempTree::new("designated");
        let path = tree.root().join("designated");
        let designated =
            SandboxDirectory::prepare(&path).expect("prepare the designated directory");

        assert_eq!(
            designated.path(),
            plain_path(fs::canonicalize(&path).expect("canonical path"))
        );
        designated
            .verify()
            .expect("the designated directory is contained");
        assert_eq!(LOW_INTEGRITY_SID, "S-1-16-4096");

        let again =
            SandboxDirectory::prepare(&path).expect("re-preparing a designated directory succeeds");
        again.verify().expect("still contained");
        again
            .remove_tree()
            .expect("remove the designated directory");
        assert!(!path.exists());
    }

    #[test]
    fn a_designated_directory_under_a_low_integrity_parent_is_refused() {
        let tree = TempTree::new("low-parent");
        let designated = SandboxDirectory::prepare(tree.root().join("designated"))
            .expect("prepare the designated directory");
        let refused = SandboxDirectory::prepare(designated.path().join("nested"));
        assert!(matches!(
            refused.err(),
            Some(ContainmentError::ParentIntegrityNotHigher(_))
        ));
    }

    #[test]
    fn the_job_object_reports_every_limit_it_was_asked_to_enforce() {
        let limits = ContainmentLimits::strict()
            .with_job_cpu_time(Duration::from_secs(45))
            .with_per_process_cpu_time(Duration::from_secs(15));
        let mut guard = RestrictedProcessGuard::spawn(
            RestrictedProcessSpec::new(comspec())
                .with_args(["/D", "/S", "/C"])
                .with_arg("exit /b 0"),
            &limits,
        )
        .expect("start a contained child");

        assert_eq!(guard.effective_limits().expect("job limits"), limits);
        assert_eq!(guard.total_process_count().expect("assigned processes"), 1);
        let outcome = guard.wait().expect("the child exits");
        assert_eq!(outcome.exit_code(), 0);
        assert_eq!(guard.active_process_count().expect("active processes"), 0);
    }

    #[test]
    fn a_cpu_time_limit_terminates_a_child_that_would_otherwise_run_forever() {
        let tree = TempTree::new("cpu-limit");
        let designated = SandboxDirectory::prepare(tree.root().join("designated"))
            .expect("prepare the designated directory");
        let started = designated.path().join("started.txt");
        let script = write_child_script(designated.path(), "spin.cmd", WRITE_THEN_SPIN);
        let mut guard = RestrictedProcessGuard::spawn(
            contained_child(designated.path(), &script, &[(INSIDE, started.as_path())]),
            &ContainmentLimits::strict()
                .with_per_process_cpu_time(Duration::from_secs(2))
                .with_job_cpu_time(Duration::from_secs(2)),
        )
        .expect("start a contained child");

        let outcome = guard.wait().expect("the job stops the child");
        assert!(
            started.exists(),
            "the contained child did not get as far as running its script"
        );
        assert!(
            outcome.hit_cpu_time_limit(),
            "the child was not stopped by the CPU time limit, only after {:?} of CPU time",
            outcome.total_cpu_time()
        );
        assert_ne!(outcome.exit_code(), 0);
    }

    #[test]
    fn dropping_the_guard_terminates_the_child() {
        let tree = TempTree::new("drop");
        let designated = SandboxDirectory::prepare(tree.root().join("designated"))
            .expect("prepare the designated directory");
        let started = designated.path().join("started.txt");
        let script = write_child_script(designated.path(), "spin.cmd", WRITE_THEN_SPIN);
        let process_id = {
            let guard = RestrictedProcessGuard::spawn(
                contained_child(designated.path(), &script, &[(INSIDE, started.as_path())]),
                &ContainmentLimits::interactive(),
            )
            .expect("start a contained child");
            let process_id = guard
                .child_process_id()
                .expect("contained child identifier");
            assert!(is_process_alive(process_id).expect("the child is running"));
            assert_eq!(
                process_integrity(process_id).expect("the child's own token level"),
                MandatoryIntegrity::Low,
                "the contained child's primary token is not at the low integrity level"
            );
            assert!(
                !process_integrity(std::process::id())
                    .expect("this process's token level")
                    .is_low(),
                "the trusted process must not be at the low integrity level"
            );
            process_id
        };

        assert!(
            eventually(
                || !is_process_alive(process_id).unwrap_or(false),
                Duration::from_secs(10)
            ),
            "dropping the guard left an uncontained child running as {process_id}"
        );
    }

    #[test]
    fn terminate_ends_the_child_before_its_budget_runs_out() {
        let tree = TempTree::new("terminate");
        let designated = SandboxDirectory::prepare(tree.root().join("designated"))
            .expect("prepare the designated directory");
        let started = designated.path().join("started.txt");
        let script = write_child_script(designated.path(), "spin.cmd", WRITE_THEN_SPIN);
        let mut guard = RestrictedProcessGuard::spawn(
            contained_child(designated.path(), &script, &[(INSIDE, started.as_path())]),
            &ContainmentLimits::interactive(),
        )
        .expect("start a contained child");
        assert!(guard.is_running().expect("the child is running"));

        guard.terminate().expect("terminate the job");
        let outcome = guard
            .wait_for(Duration::from_secs(10))
            .expect("wait for the terminated child")
            .expect("the terminated child is reaped");
        assert!(!guard.is_running().expect("the child has stopped"));
        assert_ne!(outcome.exit_code(), 0);
    }
}
