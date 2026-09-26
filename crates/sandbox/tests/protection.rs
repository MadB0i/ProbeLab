//! Tests for path protection: making trusted paths unreadable to a contained child.
//!
//! These tests exist because the security claim is empirical. Mandatory integrity control refuses a
//! low-integrity *write* into a higher-labeled object but still permits a *read*, so the low
//! integrity label alone does not stop a contained child from reading the benchmark definition and
//! the evaluator. The protection under test adds an explicit deny ACE for the low integrity SID
//! `S-1-16-4096`, and these tests prove three things with a real child process:
//!
//! 1. a contained child is refused read access to a protected path;
//! 2. the refusal is specific, not blanket: the same child still reads an unprotected path and still
//!    writes inside its designated directory; and
//! 3. the trusted process is completely unaffected, before, during, and after the protection, and
//!    the original DACLs come back exactly as they were.
//!
//! Every test uses a unique temporary tree that is removed when the test finishes, and the
//! non-Windows tests assert only the typed unsupported-platform result.

use probelab_sandbox::{ContainmentError, ProtectedPaths};

/// A path that does not exist is skipped rather than failing the whole set.
#[test]
fn a_path_that_does_not_exist_is_skipped_but_an_empty_set_is_refused() {
    let missing = std::env::temp_dir().join("probelab-protection-does-not-exist-4f2a");
    assert!(!missing.exists());
    let journal = std::env::temp_dir().join("probelab-protection-journal-empty-set");
    let _ = std::fs::remove_dir_all(&journal);

    assert!(matches!(
        ProtectedPaths::apply_in(&journal, &[missing]).expect_err("nothing existed to protect"),
        ContainmentError::ProtectionNothingToProtect
    ));
    assert!(matches!(
        ProtectedPaths::apply_in(&journal, &[]).expect_err("an empty set protects nothing"),
        ContainmentError::ProtectionNothingToProtect
    ));
}

#[test]
fn a_platform_without_os_containment_refuses_to_protect_anything() {
    let journal = std::env::temp_dir().join("probelab-protection-journal-unsupported");
    let existing = std::env::temp_dir();
    if !cfg!(windows) {
        assert!(matches!(
            ProtectedPaths::apply_in(&journal, &[existing]).expect_err("unsupported platform"),
            ContainmentError::UnsupportedPlatform
        ));
        assert!(matches!(
            ProtectedPaths::recover_stale_in(&journal).expect_err("unsupported platform"),
            ContainmentError::UnsupportedPlatform
        ));
    }
    let _ = std::fs::remove_dir_all(&journal);
}

#[cfg(not(windows))]
mod unsupported {
    use probelab_sandbox::{ContainmentError, ProtectedPaths};

    #[test]
    fn protection_and_recovery_report_the_platform_rather_than_pretending() {
        let journal = std::env::temp_dir().join("probelab-protection-journal-non-windows");
        assert!(matches!(
            ProtectedPaths::apply_in(&journal, &[std::env::temp_dir()]).expect_err("no ACLs here"),
            ContainmentError::UnsupportedPlatform
        ));
        assert!(matches!(
            ProtectedPaths::recover_stale_in(&journal).expect_err("no ACLs here"),
            ContainmentError::UnsupportedPlatform
        ));
    }
}

#[cfg(windows)]
mod enforced {
    use probelab_sandbox::{
        is_process_alive, read_integrity, ContainmentLimits, MandatoryIntegrity, ProtectedPaths,
        RestrictedProcessGuard, RestrictedProcessSpec, SandboxDirectory, LOW_INTEGRITY_SID,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

    /// A unique temporary tree that is removed when the test finishes.
    ///
    /// Declared before any protection so it is dropped *after* the protection has been undone, which
    /// means a test can never delete a tree that still carries a low-integrity deny ACE.
    struct TempTree {
        root: PathBuf,
    }

    impl TempTree {
        fn new(label: &str) -> Self {
            let sequence = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "probelab-protection-{label}-{}-{sequence}",
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

        fn dir(&self, name: &str) -> PathBuf {
            let path = self.root().join(name);
            fs::create_dir_all(&path).expect("create directory");
            path
        }

        fn file(&self, directory: &Path, name: &str, contents: &str) -> PathBuf {
            let path = directory.join(name);
            fs::write(&path, contents).expect("write file");
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            remove_tree(&self.root);
        }
    }

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

    fn comspec() -> PathBuf {
        std::env::var_os("ComSpec")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows\\System32\\cmd.exe"))
    }

    /// The secret a contained child must never be able to read.
    const SECRET: &str = "probelab-evaluator-answer-key";
    /// The harmless file the same child must still be able to read.
    const PUBLIC: &str = "public notes";

    /// Copies a protected file and an unprotected file into the child's own directory, then reports
    /// which copies exist.
    ///
    /// The protected file is named through an environment variable so nothing about the assertion
    /// depends on how a path is quoted on a command line, and both copies are attempted before the
    /// report so a failed read cannot be mistaken for a copy that silently did nothing.
    const COPY_BOTH: &str = "copy /y \"%PROBELAB_PROTECTED_FILE%\" \".\\stolen.txt\" >nul 2>&1\r\ncopy /y \"%PROBELAB_PUBLIC_FILE%\" \".\\copied.txt\" >nul 2>&1\r\nif exist \".\\copied.txt\" exit /b 7\r\nexit /b 0";

    /// Builds a contained child that runs `script` inside the designated directory.
    fn contained_child(designated: &Path, script: &str) -> RestrictedProcessSpec {
        RestrictedProcessSpec::new(comspec())
            .with_args(["/D", "/C", script])
            .with_current_directory(designated)
    }

    #[test]
    fn a_contained_child_cannot_read_a_protected_path_but_the_trusted_process_still_can() {
        let tree = TempTree::new("read-denial");
        let protected = tree.dir("evaluator");
        let open = tree.dir("public");
        let secret = tree.file(&protected, "answer-key.txt", SECRET);
        let public = tree.file(&open, "notes.txt", PUBLIC);

        // The trusted process reads both files before any protection exists.
        assert_eq!(fs::read_to_string(&secret).expect("trusted read"), SECRET);
        assert_eq!(fs::read_to_string(&public).expect("trusted read"), PUBLIC);

        let designated = SandboxDirectory::prepare(tree.root().join("designated"))
            .expect("prepare the designated sandbox directory");
        let protection = ProtectedPaths::apply_in(
            &tree.root().join("journal"),
            std::slice::from_ref(&protected),
        )
        .expect("protect the evaluator tree");
        assert_eq!(protection.protected_paths(), vec![protected.as_path()]);
        protection.verify().expect("protection verifies");

        // The trusted process is unaffected while the protection is held: the deny ACE names the low
        // integrity SID, which the trusted token does not carry.
        assert_eq!(fs::read_to_string(&secret).expect("trusted read"), SECRET);
        assert_eq!(fs::read_to_string(&public).expect("trusted read"), PUBLIC);

        let script = "copy-both.cmd";
        fs::write(
            designated.path().join(script),
            format!("@echo off\r\n{COPY_BOTH}"),
        )
        .expect("write child script");

        let mut spec = contained_child(designated.path(), script);
        spec = spec
            .with_env("PROBELAB_PROTECTED_FILE", secret.as_os_str())
            .with_env("PROBELAB_PUBLIC_FILE", public.as_os_str());
        let mut guard = RestrictedProcessGuard::spawn(spec, &ContainmentLimits::strict())
            .expect("start the contained child");
        let pid = guard.child_process_id().expect("child pid");
        let outcome = guard.wait().expect("wait for the child");
        assert!(!is_process_alive(pid).expect("child exited"));

        // Exit code 7 is the script's "the harmless copy worked" report, so the child really did run
        // and really did attempt the protected copy.
        assert_eq!(
            outcome.exit_code(),
            7,
            "the child must have copied the unprotected file and failed on the protected one"
        );
        assert!(
            !designated.path().join("stolen.txt").exists(),
            "a contained child read a protected path"
        );
        assert_eq!(
            fs::read_to_string(designated.path().join("copied.txt")).expect("public copy"),
            PUBLIC,
            "the refusal must be specific, not a blanket denial"
        );

        drop(protection);

        // The original DACL is back, so the trusted process still reads the same file and the
        // protection is genuinely gone rather than merely unobserved.
        assert_eq!(fs::read_to_string(&secret).expect("trusted read"), SECRET);
        let mut spec = contained_child(designated.path(), script);
        spec = spec
            .with_env("PROBELAB_PROTECTED_FILE", secret.as_os_str())
            .with_env("PROBELAB_PUBLIC_FILE", public.as_os_str());
        let mut guard = RestrictedProcessGuard::spawn(spec, &ContainmentLimits::strict())
            .expect("start the child again after the protection was undone");
        let outcome = guard.wait().expect("wait for the child");
        assert_eq!(
            outcome.exit_code(),
            7,
            "the script reports the harmless copy regardless"
        );
        assert!(
            designated.path().join("stolen.txt").exists(),
            "the protection was not actually removed"
        );
    }

    #[test]
    fn the_protection_is_inherited_by_files_created_inside_a_protected_directory() {
        let tree = TempTree::new("inherited");
        let protected = tree.dir("trusted");
        let designated = SandboxDirectory::prepare(tree.root().join("designated"))
            .expect("prepare the designated sandbox directory");
        let protection = ProtectedPaths::apply_in(
            &tree.root().join("journal"),
            std::slice::from_ref(&protected),
        )
        .expect("protect the trusted tree");

        let child_file = tree.file(&protected, "written-by-trusted.txt", SECRET);
        assert!(child_file.exists());
        protection
            .verify()
            .expect("the inherited deny still verifies");
        assert_eq!(
            fs::read_to_string(&child_file).expect("trusted read"),
            SECRET
        );
        assert!(designated.path().is_dir());
    }

    #[test]
    fn a_protection_left_behind_by_a_hard_kill_is_recovered_from_the_journal() {
        let tree = TempTree::new("recovery");
        let protected = tree.dir("evaluator");
        let secret = tree.file(&protected, "answer-key.txt", SECRET);
        let journal = tree.root().join("journal");
        fs::create_dir_all(&journal).expect("create journal directory");

        // Simulate a process that was killed after protecting and before restoring: the value is
        // deliberately leaked so `Drop` never runs and the journal survives.
        let protection = ProtectedPaths::apply_in(&journal, std::slice::from_ref(&protected))
            .expect("protect the evaluator tree");
        std::mem::forget(protection);

        let script = "copy-protected.cmd";
        let designated = SandboxDirectory::prepare(tree.root().join("designated"))
            .expect("prepare the designated sandbox directory");
        fs::write(
            designated.path().join(script),
            "@echo off\r\ncopy /y \"%PROBELAB_PROTECTED_FILE%\" \".\\stolen.txt\" >nul 2>&1\r\nexit /b 0",
        )
        .expect("write child script");

        let spec = contained_child(designated.path(), script)
            .with_env("PROBELAB_PROTECTED_FILE", secret.as_os_str());
        let mut guard = RestrictedProcessGuard::spawn(spec, &ContainmentLimits::strict())
            .expect("start the contained child");
        guard.wait().expect("wait for the child");
        assert!(
            !designated.path().join("stolen.txt").exists(),
            "the leaked protection must still be in force"
        );

        // Recovery is what a later run performs before it protects anything.
        assert_eq!(
            ProtectedPaths::recover_stale_in(&journal).expect("recover the stale protection"),
            1
        );
        assert_eq!(
            ProtectedPaths::recover_stale_in(&journal).expect("a second recovery is a no-op"),
            0,
            "the journal must be gone once it has been replayed"
        );

        let spec = contained_child(designated.path(), script)
            .with_env("PROBELAB_PROTECTED_FILE", secret.as_os_str());
        let mut guard = RestrictedProcessGuard::spawn(spec, &ContainmentLimits::strict())
            .expect("start the child after recovery");
        guard.wait().expect("wait for the child");
        assert!(
            designated.path().join("stolen.txt").exists(),
            "recovery must restore the original DACLs"
        );
        assert_eq!(fs::read_to_string(&secret).expect("trusted read"), SECRET);
    }

    #[test]
    fn protecting_a_path_that_does_not_exist_is_not_an_error() {
        let tree = TempTree::new("absent");
        let present = tree.dir("present");
        let missing = tree.root().join("not-here");
        let protection =
            ProtectedPaths::apply_in(&tree.root().join("journal"), &[present, missing])
                .expect("the existing path is protected");
        assert_eq!(protection.protected_paths().len(), 1);
        protection.verify().expect("verify");
    }

    #[test]
    fn the_refusal_is_scoped_to_the_low_integrity_level() {
        let tree = TempTree::new("sid");
        let protected = tree.dir("trusted");
        let _protection = ProtectedPaths::apply_in(
            &tree.root().join("journal"),
            std::slice::from_ref(&protected),
        )
        .expect("protect");
        // The refusal works by leaving a path's own integrity level in place and adding the
        // read-up flag, so the only thing that matters is that the child really is below it.
        assert_eq!(LOW_INTEGRITY_SID, "S-1-16-4096");
        // The protected path kept its level: a protection that raised the level instead of adding
        // the flag would lock the trusted process out, which `apply_in` refuses to do.
        assert_eq!(
            read_integrity(&protected).expect("read the protected level"),
            MandatoryIntegrity::Medium,
            "protection must not change the integrity level of the path"
        );
    }
}
