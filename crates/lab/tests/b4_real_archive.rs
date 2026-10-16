//! Reads a real user archive READ-ONLY and reports how many recordings this build can open.
//!
//! Evidence for audit finding B4: the same archive that produced
//! `invalid type: integer 2, expected an array of length 2` for 24 of its 59 recordings.
//!
//! Opened with `mode=ro` so it cannot modify anything, and it never writes.

use probelab_lab::arena_store::SqliteArenaMatchStore;

#[test]
fn a_real_archive_opens_every_recording_it_lists() {
    let Ok(archive) = std::env::var("PROBELAB_REAL_ARCHIVE") else {
        eprintln!("skipping: PROBELAB_REAL_ARCHIVE is not set");
        return;
    };
    let path = std::path::Path::new(&archive);
    if !path.exists() {
        eprintln!("skipping: {archive} does not exist");
        return;
    }

    let store = SqliteArenaMatchStore::open(path).expect("the archive opens read-only");
    let summaries = store.list(1000).expect("the index lists");

    let mut unreadable: Vec<(String, String)> = Vec::new();
    for summary in &summaries {
        if let Err(error) = store.get(&summary.match_id) {
            unreadable.push((summary.match_id.clone(), error.to_string()));
        }
    }

    eprintln!(
        "{} recordings listed, {} readable, {} unreadable",
        summaries.len(),
        summaries.len() - unreadable.len(),
        unreadable.len()
    );
    for (match_id, error) in unreadable.iter().take(5) {
        eprintln!("  unreadable {match_id}: {error}");
    }

    assert!(
        unreadable.is_empty(),
        "{} listed recordings cannot be opened, which is the defect B4 describes: the index \
         advertises evidence the product cannot read",
        unreadable.len()
    );
}
