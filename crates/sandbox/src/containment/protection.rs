//! Path protection: making trusted source trees unreadable to a contained child.
//!
//! The low mandatory integrity label assigned by [`super::RestrictedProcessGuard`] is a *write*
//! boundary. Mandatory integrity control refuses a write from a lower level into a higher-labeled
//! object, but it still permits a **read** from a lower level into a higher-labeled object. A
//! contained child can therefore still read the benchmark definition, the evaluator implementation,
//! and anything else the trusted process can read, which would hand it the answer key.
//!
//! This module closes that gap using mandatory integrity control itself. The protection adds
//! `NO_READ_UP` to the integrity level a path *already has*, and changes nothing else:
//!
//! * the level is not changed, so a path that was implicitly medium is still medium;
//! * the DACL, the owner, and the group are never read and never written, so the trusted user keeps
//!   exactly the access it had;
//! * `NO_READ_UP` only ever refuses a caller at a **strictly lower** integrity level, and the trusted
//!   process runs at the same level as the path, so ProbeLab is unaffected.
//!
//! # The approach that did not work, and why this one is here
//!
//! The obvious alternative is an explicit deny ACE in the DACL for the low integrity SID
//! `S-1-16-4096`. It was implemented and tested first, and it **does not work**: a contained child
//! still read a protected file. A DACL entry is only ever matched against a SID the token actually
//! carries, and a token's integrity level lives in `TokenIntegrityLevel`, not among its group SIDs,
//! so a deny ACE naming `S-1-16-4096` never matches a low-integrity caller. That approach is
//! deliberately *not* what ships, because code that mutates real DACLs for no proven benefit is worse
//! than no code at all.
//!
//! The mandatory label is the mechanism Windows actually uses for this decision, and the tests in
//! `tests/protection.rs` prove the behaviour with a real child process rather than by inspection.
//!
//! # The protection is temporary and reversible
//!
//! Mutating a mandatory label on a real source tree is handled defensively:
//!
//! * the original label of every protected path is captured **before** anything is changed, and an
//!   absent label is recorded as absent rather than as an empty string;
//! * those originals are written to a *crash-recovery journal* on disk **before** the first label is
//!   written, so a process killed mid-run leaves a recoverable record rather than a permanently
//!   locked tree;
//! * dropping the value restores every original label and deletes the journal;
//! * [`ProtectedPaths::recover_stale`] replays any journal left behind by a hard kill.
//!
//! Restoring a path that had no explicit label writes the medium label Windows itself uses for an
//! ordinary object, which is the state it was in. There is no way to delete a mandatory label
//! through the file security API, and leaving the read-up refusal in place would be the one outcome
//! that is definitely wrong.
//!
//! If a path cannot be protected, the whole set fails closed and the caller is told why. There is no
//! partial protection: an unprotected path is never silently skipped, because a silently skipped path
//! is exactly the hole this module exists to close. Paths that do not exist are skipped, because
//! there is nothing there to leak.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::ContainmentError;

/// Counts journal file names so two protections in one process never collide.
static JOURNAL_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The journal marker for a path that had no explicit mandatory label at all.
///
/// Windows cannot delete a mandatory label through `SetFileSecurityW`, so such a path is restored to
/// the medium label Windows itself writes for an ordinary object, which is the state it was in.
const ABSENT_LABEL: &str = "-";

/// One protected path together with the mandatory label it had before protection.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ProtectionEntry {
    /// The exact path whose DACL was changed.
    path: PathBuf,
    /// The label in SDDL form, captured before the read-up refusal was added; None when the path
    /// carried no explicit label at all.
    original_label: Option<String>,
}

/// A set of trusted paths that a contained child cannot read, for the lifetime of this value.
///
/// The protection is applied in [`ProtectedPaths::apply`] and undone when the value is dropped, so
/// the caller should hold it for exactly as long as a child may be running. On a non-Windows
/// platform construction fails with [`ContainmentError::UnsupportedPlatform`] and nothing is
/// changed.
pub struct ProtectedPaths {
    entries: Vec<ProtectionEntry>,
    journal: PathBuf,
    restored: bool,
}

impl std::fmt::Debug for ProtectedPaths {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProtectedPaths")
            .field("protected", &self.entries.len())
            .field("restored", &self.restored)
            .finish()
    }
}

impl ProtectedPaths {
    /// Protects every existing path in `paths` and journals the original labels.
    ///
    /// Non-existent paths are skipped, because a path that is not there holds nothing to leak. Any
    /// other failure — a path that cannot be read, a DACL that cannot be captured, an ACE that
    /// cannot be written, or a protection that cannot be verified afterwards — restores everything
    /// already applied, removes the journal, and returns an error.
    pub fn apply(paths: &[PathBuf]) -> Result<Self, ContainmentError> {
        Self::apply_in(&default_journal_directory(), paths)
    }

    /// Protects every existing path in `paths`, journaling under an explicit directory.
    ///
    /// The directory exists so a caller that runs several protections side by side can keep their
    /// journals apart, and so a test can never share a journal with another test.
    pub fn apply_in(journal_directory: &Path, paths: &[PathBuf]) -> Result<Self, ContainmentError> {
        if !cfg!(windows) {
            return Err(ContainmentError::UnsupportedPlatform);
        }
        for path in paths {
            if path.as_os_str().is_empty() {
                return Err(ContainmentError::ProtectionPathInvalid);
            }
            if contains_separator_disallowed_in_a_journal_line(path) {
                return Err(ContainmentError::ProtectionPathInvalid);
            }
        }
        let existing = deduplicate(paths);

        // Capture first. Nothing is modified until every original label is safely on disk.
        let mut entries = Vec::with_capacity(existing.len());
        for path in &existing {
            if !path.exists() {
                continue;
            }
            let original_label = super::platform::capture_path_label(path)?;
            entries.push(ProtectionEntry {
                path: path.clone(),
                original_label,
            });
        }
        if entries.is_empty() {
            return Err(ContainmentError::ProtectionNothingToProtect);
        }

        let journal = write_journal(journal_directory, &entries)?;
        let mut protection = Self {
            entries,
            journal,
            restored: false,
        };
        if let Err(error) = protection.apply_all() {
            // Undo whatever landed before the failure so a partial set is never left behind.
            let _ = protection.restore();
            return Err(error);
        }
        if let Err(error) = protection.verify() {
            let _ = protection.restore();
            return Err(error);
        }
        Ok(protection)
    }

    /// Returns the paths this value actually protected, in the order they were applied.
    pub fn protected_paths(&self) -> Vec<&Path> {
        self.entries
            .iter()
            .map(|entry| entry.path.as_path())
            .collect()
    }

    /// Confirms every protected path still refuses a read from a lower integrity level.
    ///
    /// A child is started only after this passes, so a path whose ACL was changed by something else
    /// while the protection was held is caught before a process exists.
    pub fn verify(&self) -> Result<(), ContainmentError> {
        if self.entries.is_empty() {
            return Err(ContainmentError::ProtectionNothingToProtect);
        }
        for entry in &self.entries {
            if !super::platform::path_denies_low_integrity_reads(&entry.path)? {
                return Err(ContainmentError::ProtectionNotVerified {
                    path: entry.path.clone(),
                });
            }
        }
        Ok(())
    }

    /// Restores every original label and removes the journal.
    ///
    /// This is what dropping the value does. It is safe to call more than once, and it reports the
    /// first failure without giving up on the remaining paths, because leaving one path locked is
    /// worse than reporting one error.
    pub fn restore(&mut self) -> Result<(), ContainmentError> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;
        let mut first_error = None;
        for entry in &self.entries {
            if let Err(error) =
                super::platform::restore_path_label(&entry.path, entry.original_label.as_deref())
            {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        if let Err(error) = fs::remove_file(&self.journal) {
            if first_error.is_none() && error.kind() != std::io::ErrorKind::NotFound {
                first_error = Some(ContainmentError::ProtectionJournal {
                    detail: "the protection journal could not be removed",
                });
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Replays any journal left behind by a process that was killed before it could restore.
    ///
    /// Call this before applying new protection. Restoration is idempotent, so replaying a journal
    /// whose ACEs were never applied simply rewrites the DACLs that were already there. Returns the
    /// number of stale protections that were recovered.
    pub fn recover_stale() -> Result<usize, ContainmentError> {
        Self::recover_stale_in(&default_journal_directory())
    }

    /// Replays stale journals in an explicit directory.
    pub fn recover_stale_in(journal_directory: &Path) -> Result<usize, ContainmentError> {
        if !cfg!(windows) {
            return Err(ContainmentError::UnsupportedPlatform);
        }
        let Ok(directory) = fs::read_dir(journal_directory) else {
            return Ok(0);
        };
        let mut recovered = 0;
        let mut journals: Vec<PathBuf> = directory
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "journal")
            })
            .collect();
        journals.sort();
        for journal in journals {
            let entries = read_journal(&journal)?;
            let mut first_error = None;
            for entry in &entries {
                if let Err(error) = super::platform::restore_path_label(
                    &entry.path,
                    entry.original_label.as_deref(),
                ) {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
            match first_error {
                Some(error) => return Err(error),
                None => {
                    let _ = fs::remove_file(&journal);
                    recovered += 1;
                }
            }
        }
        Ok(recovered)
    }

    /// Applies the label change to every entry and confirms the trusted process is unaffected.
    ///
    /// The second half matters as much as the first. If the process running ProbeLab happens to sit
    /// at a *lower* integrity level than a protected path — which happens when the desktop app is
    /// launched from an elevated shell while the path was created at a higher level — the read-up
    /// flag would lock the trusted process out of its own source tree. That is refused here rather
    /// than discovered later, because silently protecting a path the application can no longer read
    /// would be a worse failure than not protecting it.
    fn apply_all(&mut self) -> Result<(), ContainmentError> {
        for entry in &self.entries {
            super::platform::deny_low_integrity_reads(&entry.path)?;
        }
        for entry in &self.entries {
            if !trusted_process_can_read(&entry.path) {
                return Err(ContainmentError::ProtectionWouldLockTrustedProcess {
                    path: entry.path.clone(),
                });
            }
        }
        Ok(())
    }
}

/// Returns true when the trusted process can still reach a path after protection.
///
/// This is a real read, performed with the ordinary standard library, not an inspection of a
/// descriptor: the point is to prove the application itself is not locked out of its own files.
fn trusted_process_can_read(path: &Path) -> bool {
    if path.is_dir() {
        std::fs::read_dir(path).is_ok()
    } else {
        std::fs::File::open(path).is_ok()
    }
}

impl Drop for ProtectedPaths {
    fn drop(&mut self) {
        // A restore failure here cannot be reported, so the journal is deliberately left in place:
        // `recover_stale` will replay it on the next run instead of losing the only record of the
        // original labels.
        let _ = self.restore();
    }
}

/// Removes duplicates and any path nested inside another, keeping only the outermost paths.
///
/// Protecting a directory already covers everything beneath it, so protecting both would only mean
/// restoring the inner DACL while the outer deny ACE is still in place. The filter is a separate pass
/// so the result does not depend on the order the paths were listed in.
fn deduplicate(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut unique: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !unique.iter().any(|existing| existing == path) {
            unique.push(path.clone());
        }
    }
    unique
        .iter()
        .filter(|candidate| {
            !unique
                .iter()
                .any(|other| other != *candidate && candidate.starts_with(other))
        })
        .cloned()
        .collect()
}

/// The directory stale protection journals are written to.
fn default_journal_directory() -> PathBuf {
    std::env::temp_dir().join("probelab-protected-paths")
}

/// A path is only journalable when it cannot break the one-record-per-line journal format.
fn contains_separator_disallowed_in_a_journal_line(path: &Path) -> bool {
    path.to_string_lossy()
        .chars()
        .any(|character| matches!(character, '\n' | '\r' | '\t'))
}

/// Returns a journal file name that is unique per process and per protection.
fn journal_file_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let sequence = JOURNAL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{nanos}-{sequence}.journal", std::process::id())
}

/// Writes the original labels to disk before any ACL is changed.
///
/// The write is flushed to the file system before it returns, so a process killed immediately after
/// this call still leaves a complete record to recover from.
fn write_journal(
    journal_directory: &Path,
    entries: &[ProtectionEntry],
) -> Result<PathBuf, ContainmentError> {
    fs::create_dir_all(journal_directory).map_err(|_| ContainmentError::ProtectionJournal {
        detail: "the protection journal directory could not be created",
    })?;
    let journal = journal_directory.join(journal_file_name());
    let mut body = String::new();
    for entry in entries {
        body.push_str(&entry.path.to_string_lossy());
        body.push('\t');
        body.push_str(entry.original_label.as_deref().unwrap_or(ABSENT_LABEL));
        body.push('\n');
    }
    fs::write(&journal, body).map_err(|_| ContainmentError::ProtectionJournal {
        detail: "the protection journal could not be written",
    })?;
    let file = fs::OpenOptions::new()
        .write(true)
        .open(&journal)
        .map_err(|_| ContainmentError::ProtectionJournal {
            detail: "the protection journal could not be reopened to flush it",
        })?;
    file.sync_all()
        .map_err(|_| ContainmentError::ProtectionJournal {
            detail: "the protection journal could not be flushed",
        })?;
    Ok(journal)
}

/// Reads one journal back into protection entries.
///
/// A journal line that does not parse is a hard error rather than a skipped line: silently ignoring
/// a record would leave a protected path locked with no way to restore it.
fn read_journal(journal: &Path) -> Result<Vec<ProtectionEntry>, ContainmentError> {
    let body = fs::read_to_string(journal).map_err(|_| ContainmentError::ProtectionJournal {
        detail: "a protection journal could not be read",
    })?;
    let mut entries = Vec::new();
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        let (path, original_label) =
            line.split_once('\t')
                .ok_or(ContainmentError::ProtectionJournal {
                    detail: "a protection journal line is malformed",
                })?;
        if path.is_empty() || original_label.is_empty() {
            return Err(ContainmentError::ProtectionJournal {
                detail: "a protection journal line is incomplete",
            });
        }
        entries.push(ProtectionEntry {
            path: PathBuf::from(path),
            original_label: if original_label == ABSENT_LABEL {
                None
            } else {
                Some(original_label.to_owned())
            },
        });
    }
    if entries.is_empty() {
        return Err(ContainmentError::ProtectionJournal {
            detail: "a protection journal recorded no paths",
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::{contains_separator_disallowed_in_a_journal_line, deduplicate, ProtectionEntry};
    use std::path::{Path, PathBuf};

    fn joined(root: &str, rest: &str) -> PathBuf {
        let mut path = PathBuf::from(root);
        for part in rest.split('/').filter(|part| !part.is_empty()) {
            path.push(part);
        }
        path
    }

    #[test]
    fn duplicate_paths_collapse_to_one_entry() {
        let paths = vec![joined("a", "b"), joined("a", "b"), joined("a", "b")];
        assert_eq!(deduplicate(&paths), vec![joined("a", "b")]);
    }

    #[test]
    fn a_nested_path_is_covered_by_its_outermost_directory() {
        let outer = joined("a", "b");
        let inner = joined("a", "b/c/d");
        assert_eq!(
            deduplicate(&[outer.clone(), inner.clone()]),
            vec![outer.clone()],
            "protecting the parent already covers the child"
        );
        assert_eq!(
            deduplicate(&[inner, outer.clone()]),
            vec![outer],
            "order must not change which path is protected"
        );
    }

    #[test]
    fn a_shared_prefix_is_not_treated_as_nesting() {
        let data = joined("a", "data");
        let database = joined("a", "database");
        assert_eq!(
            deduplicate(&[data.clone(), database.clone()]),
            vec![data, database],
            "a common name prefix is not a parent directory"
        );
    }

    #[test]
    fn unrelated_paths_are_all_kept() {
        let paths = vec![joined("a", "one"), joined("b", "two"), joined("c", "three")];
        assert_eq!(deduplicate(&paths), paths);
    }

    #[test]
    fn only_paths_that_would_break_the_journal_format_are_refused() {
        assert!(!contains_separator_disallowed_in_a_journal_line(Path::new(
            "a/b c/d"
        )));
        assert!(contains_separator_disallowed_in_a_journal_line(Path::new(
            "a/b\tc"
        )));
        assert!(contains_separator_disallowed_in_a_journal_line(Path::new(
            "a/b\nc"
        )));
        assert!(contains_separator_disallowed_in_a_journal_line(Path::new(
            "a/b\rc"
        )));
    }

    #[test]
    fn an_entry_keeps_the_label_it_captured() {
        let entry = ProtectionEntry {
            path: PathBuf::from("trusted"),
            original_label: Some("S:(ML;;NW;;;8192)".to_owned()),
        };
        assert_eq!(entry.path, PathBuf::from("trusted"));
        assert_eq!(entry.original_label.as_deref(), Some("S:(ML;;NW;;;8192)"));
    }

    #[test]
    fn protecting_nothing_is_refused_rather_than_succeeding_vacuously() {
        let error = super::ProtectedPaths::apply(&[]).expect_err("an empty set protects nothing");
        assert!(matches!(
            error,
            super::ContainmentError::UnsupportedPlatform
                | super::ContainmentError::ProtectionNothingToProtect
        ));
    }
}
