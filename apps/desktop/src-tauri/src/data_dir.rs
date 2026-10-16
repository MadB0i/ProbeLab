//! Where this process keeps its database, and how a test build is kept away from the real one.
//!
//! # Why this exists
//!
//! Scripted product tests drive the real executable. That executable resolved its data directory through
//! `app.path().app_data_dir()`, which on Windows goes through the known-folder APIs and therefore
//! **ignores `APPDATA`, `LOCALAPPDATA` and every other environment variable** an override might use. An
//! earlier attempt to redirect it that way appeared to work and did not: the test opened the
//! developer's real archive, found 41 genuine arena recordings in it, and wrote more matches into it.
//!
//! Two things were wrong with that, and only one was the environment variable.
//!
//! 1. **There was no override that could work.** On this platform the directory is not derived from the
//!    environment at all, so nothing set in a child process could redirect it.
//! 2. **Nothing verified where it was going before it wrote.** A test that cannot state its storage
//!    location is a test that can quietly mutate someone's history.
//!
//! # The rule
//!
//! A build compiled with the `test-harness` feature **must not** open the production archive. If the
//! override is absent, relative, or resolves to a path that is not plainly a scratch directory, start-up
//! fails. There is no fallback to the real directory: a fail-closed refusal costs one clear error
//! message, and a silent fallback costs someone's recordings.
//!
//! A production build — one without the feature — never consults the environment at all and always
//! resolves the platform's own directory, so this code cannot affect a shipped build.
//!
//! # Why a feature rather than a runtime flag
//!
//! An environment variable honoured by production would be a way for a stray variable to point a real
//! user's installation at a different database. Compiling the behaviour in makes the test behaviour
//! absent from a shipped binary rather than merely unused, so there is nothing to accidentally trigger.

use std::path::{Path, PathBuf};

/// The environment variable a test-harness build reads its scratch directory from.
pub const TEST_DATA_DIR_ENV: &str = "PROBELAB_TEST_DATA_DIR";

/// Which storage this build is using, stated rather than inferred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageMode {
    /// The platform's own application-data directory. The only mode a shipped build can be in.
    Platform,
    /// An explicitly provided scratch directory, available only to a `test-harness` build.
    IsolatedTest,
}

/// The outcome of resolving storage, including what was refused and why.
#[derive(Clone, Debug)]
pub struct ResolvedStorage {
    /// The directory to use.
    pub directory: PathBuf,
    /// How that directory was chosen.
    pub mode: StorageMode,
}

/// A refusal to use a directory. Always fatal — never a fallback.
#[derive(Clone, Debug)]
pub enum StorageRefusal {
    /// A test build was started without the variable that names its scratch directory.
    MissingOverride,
    /// The override was present but is not an absolute path, so its meaning depends on the working
    /// directory and is therefore not knowable at a glance.
    NotAbsolute(PathBuf),
    /// The override resolved to the production directory. Using it would write into real user data,
    /// which is precisely what this module exists to prevent.
    WouldTargetProduction(PathBuf),
}

impl std::fmt::Display for StorageRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageRefusal::MissingOverride => write!(
                formatter,
                "this build requires {TEST_DATA_DIR_ENV} to name a scratch directory, and it was not \
                 set. It will not fall back to the real application-data directory, because a test \
                 harness that silently opens someone's archive is worse than one that refuses to start."
            ),
            StorageRefusal::NotAbsolute(path) => write!(
                formatter,
                "{TEST_DATA_DIR_ENV} was set to {:?}, which is not an absolute path. A relative path \
                 resolves against the working directory and could land anywhere, so it is refused.",
                path.display()
            ),
            StorageRefusal::WouldTargetProduction(path) => write!(
                formatter,
                "{TEST_DATA_DIR_ENV} resolved to {:?}, which is the production application-data \
                 directory. A test build must never write there.",
                path.display()
            ),
        }
    }
}

impl std::error::Error for StorageRefusal {}

/// Resolves storage for this process.
///
/// `platform_directory` is what the platform reported; it is only used when this build is permitted to
/// use it. The split is deliberate: the decision about *which* directory is a pure function of the build
/// feature and the environment, so it is testable without a window, a display, or a database.
pub fn resolve_storage(
    platform_directory: &Path,
    test_harness_build: bool,
    override_value: Option<&str>,
) -> Result<ResolvedStorage, StorageRefusal> {
    if !test_harness_build {
        // A production build ignores the environment entirely. Not "ignores it by default" — the
        // variable is never read, so it cannot be set to redirect a shipped installation.
        return Ok(ResolvedStorage {
            directory: platform_directory.to_path_buf(),
            mode: StorageMode::Platform,
        });
    }

    let raw = override_value.ok_or(StorageRefusal::MissingOverride)?;
    let candidate = PathBuf::from(raw);

    if !candidate.is_absolute() {
        return Err(StorageRefusal::NotAbsolute(candidate));
    }

    // The one comparison that actually matters. Checked before anything is created or opened, so a
    // misconfigured harness stops before the first write rather than after it.
    if candidate == platform_directory {
        return Err(StorageRefusal::WouldTargetProduction(candidate));
    }

    Ok(ResolvedStorage {
        directory: candidate,
        mode: StorageMode::IsolatedTest,
    })
}

/// Whether this build was compiled to permit an isolated directory.
///
/// `cfg!(feature = …)` rather than a runtime constant, so the production code path contains no branch
/// that could be reached by an environment variable alone.
pub const fn is_test_harness_build() -> bool {
    cfg!(feature = "test-harness")
}

/// Resolves storage for a running process, using this build's own inputs.
pub fn resolve_for_process(platform_directory: &Path) -> Result<ResolvedStorage, StorageRefusal> {
    if !is_test_harness_build() {
        // The variable is not read at all in this branch, which is what the module comment claims.
        // Reading it and then ignoring the value would be functionally identical and semantically
        // worse: it would mean a shipped binary could be observed touching a test-only variable, and
        // the guarantee "absent from a shipped build" would become a claim about behaviour rather than
        // about what the code does.
        return resolve_storage(platform_directory, false, None);
    }
    resolve_storage(
        platform_directory,
        true,
        std::env::var(TEST_DATA_DIR_ENV).ok().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        PathBuf::from(if cfg!(windows) {
            r"C:\Users\test\AppData\Local\Temp\probelab-abc123"
        } else {
            "/tmp/probelab-abc123"
        })
    }

    fn production() -> PathBuf {
        PathBuf::from(if cfg!(windows) {
            r"C:\Users\someone\AppData\Roaming\com.probelab.desktop"
        } else {
            "/home/someone/.local/share/com.probelab.desktop"
        })
    }

    /// The production guarantee: no environment variable can move a shipped build.
    #[test]
    fn a_production_build_ignores_the_override_entirely() {
        for value in [None, Some("/tmp/elsewhere"), Some("relative")] {
            let resolved = resolve_storage(&production(), false, value)
                .expect("a production build always resolves");
            assert_eq!(resolved.mode, StorageMode::Platform);
            assert_eq!(resolved.directory, production());
        }
    }

    /// Fail closed, never fall back.
    #[test]
    fn a_test_build_without_the_variable_refuses_rather_than_falling_back() {
        let error = resolve_storage(&production(), true, None).expect_err("must refuse");
        assert!(matches!(error, StorageRefusal::MissingOverride));
        let message = error.to_string();
        assert!(
            message.contains("will not fall back"),
            "the refusal must explain itself: {message}"
        );
    }

    #[test]
    fn a_test_build_refuses_a_relative_override() {
        let error =
            resolve_storage(&production(), true, Some("relative/path")).expect_err("must refuse");
        assert!(matches!(error, StorageRefusal::NotAbsolute(_)));
    }

    /// The check that protects real user data.
    #[test]
    fn a_test_build_refuses_the_production_directory() {
        let error = resolve_storage(&production(), true, Some(&production().to_string_lossy()))
            .expect_err("must refuse");
        assert!(matches!(error, StorageRefusal::WouldTargetProduction(_)));
        assert!(error.to_string().contains("must never write there"));
    }

    #[test]
    fn a_test_build_accepts_an_absolute_scratch_directory() {
        let resolved = resolve_storage(&production(), true, Some(&scratch().to_string_lossy()))
            .expect("resolves");
        assert_eq!(resolved.mode, StorageMode::IsolatedTest);
        assert_eq!(resolved.directory, scratch());
    }

    /// A shipped binary cannot contain the test branch at all.
    #[test]
    fn the_feature_decides_which_branch_exists() {
        assert_eq!(
            is_test_harness_build(),
            cfg!(feature = "test-harness"),
            "the two must agree or the binary does not do what this file claims"
        );
    }
}
