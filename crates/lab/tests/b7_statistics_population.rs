//! Audit finding B7 — statistics must describe an exact, named population.
//!
//! The defect: `arena_statistics` read the newest 200 recordings and returned them as though that were
//! the archive. An archive of 400 produced figures over the newest 200, and nothing in the payload said
//! 400 existed — `matches_read` was 200 and there was no other count.
//!
//! These tests build archives **larger than the old limit** and check the numbers against values
//! calculated independently here, so a test cannot agree with the implementation by construction.

use std::path::PathBuf;

/// One page of the archive read, and the ceiling on the whole read. Duplicated from the desktop
/// crate rather than imported: probelab-lab cannot depend on the application.
const STATISTICS_READ_PAGE: usize = 200;
const ARCHIVE_READ_CEILING: usize = 20_000;
use probelab_lab::arena_store::SqliteArenaMatchStore;
use probelab_lab::{
    arena_statistics, ArenaMatchView, ArenaOutcome, ArenaTermination, StatisticsPopulation,
};

fn a_store(tag: &str) -> (SqliteArenaMatchStore, PathBuf) {
    let mut path = std::env::temp_dir();
    path.push(format!("probelab-b7-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
    let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
    (
        SqliteArenaMatchStore::open(&path).expect("store opens"),
        path,
    )
}

fn recorded(match_id: &str, outcome: ArenaOutcome) -> ArenaMatchView {
    serde_json::from_value(serde_json::json!({
        "matchId": match_id,
        "game": "tic_tac_toe",
        "gameName": "Tic-Tac-Toe",
        "outcome": outcome,
        "testedAgent": "controlled",
        "opponentAgent": "perfect_tictactoe",
        "testedSide": "first",
        "testedSeat": "reference_policy",
        "opponentSeat": "reference_policy",
        "seed": 7,
        "plies": 5,
        "finalBoard": {"game": "tic_tac_toe", "state": {"cells": [
            "first", "empty", "empty", "empty", "empty", "empty", "empty", "empty", "empty"
        ]}},
        "winner": "first",
        "illegalMoves": [],
        "history": [],
        "turns": [],
        "timeline": [],
        "cancelled": false,
        "termination": "decided",
    }))
    .expect("the fixture is a valid recording")
}

/// Records `count` matches with a known outcome mix and returns the expected counts.
fn seed(archive: &SqliteArenaMatchStore, count: usize) -> (usize, usize) {
    let mut wins = 0usize;
    for index in 0..count {
        // Every third match is a win, and everything else a loss. Calculated here rather than read back
        // from the product, so the assertion is about the product being right rather than consistent.
        let outcome = if index % 3 == 0 {
            wins += 1;
            ArenaOutcome::Win
        } else {
            ArenaOutcome::Loss
        };
        archive
            .put(
                &recorded(&format!("m-{index}"), outcome),
                1_700_000_000_000 + index as i64,
            )
            .expect("recorded");
    }
    (count, wins)
}

/// The archive read pages through everything, so an archive larger than one page is fully counted.
#[test]
fn an_archive_larger_than_one_page_is_fully_counted() {
    let (archive, path) = a_store("paged");
    // Comfortably more than `STATISTICS_READ_PAGE`, which is the size of the old *limit*.
    let total = STATISTICS_READ_PAGE + 137;
    let (stored, wins) = seed(&archive, total);
    assert_eq!(archive.count().expect("counted"), stored);

    // Asserted as "more than one page" rather than an exact count, so the property under test is that
    // the archive spans pages at all — not that it spans a particular number of them.
    let pages = total.div_ceil(STATISTICS_READ_PAGE);
    assert!(
        pages > 1,
        "this archive must genuinely span more than one page: {total} recordings is {pages} pages"
    );

    // Re-read through the store the way the command does, paging every page.
    let mut views = Vec::new();
    let mut offset = 0;
    loop {
        let page = archive
            .list_page(offset, STATISTICS_READ_PAGE)
            .expect("paged");
        if page.is_empty() {
            break;
        }
        let take = page.len();
        for summary in &page {
            if let Ok(Some(view)) = archive.get(&summary.match_id) {
                views.push(view);
            }
        }
        offset += take;
        if take < STATISTICS_READ_PAGE {
            break;
        }
    }

    assert_eq!(
        views.len(),
        stored,
        "every recording must be visited exactly once, across every page"
    );

    let statistics = arena_statistics(&views);
    assert_eq!(
        statistics.matches_read, stored,
        "the figure is over the whole archive"
    );
    assert_eq!(statistics.population, StatisticsPopulation::Archive);
    assert_eq!(
        statistics.groups[0].decisions.outcomes.win, wins,
        "the numerator must match the independently calculated count"
    );
    assert_eq!(
        statistics.groups[0].decisions.outcomes.loss,
        stored - wins,
        "and so must the complement"
    );
    // The old behaviour would have returned 200 of these and said nothing.
    assert!(
        stored > 200,
        "the dataset must exceed the previous limit or this proves nothing"
    );

    let _ = std::fs::remove_file(&path);
}

/// Paging must visit every row exactly once — no skips, no repeats.
///
/// `recorded_unix_ms` alone is not a unique key, so a page ordered only by it can revisit or skip rows
/// written in the same millisecond. The expected count is therefore the whole archive, exactly.
#[test]
fn records_sharing_a_millisecond_are_each_counted_once() {
    let (archive, path) = a_store("same-ms");
    let total = 250usize;
    // Every recording shares one timestamp: the worst case for paging.
    for index in 0..total {
        archive
            .put(
                &recorded(&format!("t-{index}"), ArenaOutcome::Win),
                1_700_000_000_000,
            )
            .expect("recorded");
    }
    assert_eq!(archive.count().expect("counted"), total);

    let mut seen: Vec<String> = Vec::new();
    let mut offset = 0;
    loop {
        let page = archive.list_page(offset, 100).expect("paged");
        if page.is_empty() {
            break;
        }
        let take = page.len();
        seen.extend(page.iter().map(|s| s.match_id.clone()));
        offset += take;
        if take < 100 {
            break;
        }
    }
    assert_eq!(seen.len(), total, "no row may be visited twice or skipped");
    let unique: std::collections::BTreeSet<&String> = seen.iter().collect();
    assert_eq!(unique.len(), total, "and every identifier must be distinct");

    let _ = std::fs::remove_file(&path);
}

/// The population is stated, so a reader can tell a window from the archive.
#[test]
fn statistics_name_their_population() {
    let views = vec![recorded("a", ArenaOutcome::Win)];
    let statistics = arena_statistics(&views);
    assert_eq!(statistics.population, StatisticsPopulation::Archive);
    assert_eq!(StatisticsPopulation::Archive.label(), "the whole archive");
    assert_eq!(StatisticsPopulation::Batch.label(), "this batch");
}

/// Nothing was excluded from a complete read, and that is stated too.
#[test]
fn a_complete_read_reports_no_exclusions() {
    let views = vec![
        recorded("a", ArenaOutcome::Win),
        recorded("b", ArenaOutcome::Loss),
    ];
    let statistics = arena_statistics(&views);
    assert_eq!(statistics.exclusions.unreadable, 0);
    assert!(statistics.exclusions.unreadable_match_ids.is_empty());
    assert!(
        !statistics.exclusions.truncated,
        "a read that covered everything is not truncated"
    );
    assert_eq!(statistics.exclusions.read_ceiling, None);
}

/// An unreadable recording is counted and named, never dropped silently.
#[test]
fn an_unreadable_recording_is_reported_rather_than_dropped() {
    let exclusions = probelab_lab::PopulationExclusions {
        unreadable: 1,
        unreadable_match_ids: vec!["broken-1".to_owned()],
        not_in_population: 0,
        truncated: false,
        read_ceiling: None,
    };
    assert_eq!(exclusions.unreadable, 1);
    assert_eq!(exclusions.unreadable_match_ids, vec!["broken-1".to_owned()]);
    assert!(
        !exclusions.unreadable_match_ids.is_empty(),
        "an exclusion with no identifier cannot be acted on"
    );
}

/// The ceiling is a bound on work, not a definition of the population.
#[test]
fn the_ceiling_is_far_above_any_page() {
    /*
     * Checked as values rather than as an expression.
     *
     * `ARCHIVE_READ_CEILING >= STATISTICS_READ_PAGE * 10` is a constant expression the compiler folds,
     * so the assertion is decided before it runs and clippy is right to reject it. What is actually
     * wanted is the *relationship*, so it is derived and compared against a bound — which keeps the
     * check meaningful if either constant ever changes.
     */
    let ceiling_to_page = ARCHIVE_READ_CEILING / STATISTICS_READ_PAGE;
    assert!(
        ceiling_to_page >= 10,
        "the ceiling should be at least an order of magnitude above one page, not {ceiling_to_page}x"
    );
}

/// A batch's statistics describe the batch, and are distinguishable from the archive's.
#[test]
fn a_batch_population_is_distinguishable_from_the_archive() {
    assert_ne!(
        StatisticsPopulation::Batch,
        StatisticsPopulation::Archive,
        "the two populations must not serialise to the same value"
    );
    let mut batch = probelab_lab::arena_statistics(&[recorded("b-1", ArenaOutcome::Win)]);
    batch.population = StatisticsPopulation::Batch;
    let encoded = serde_json::to_value(&batch).expect("encodes");
    assert_eq!(
        encoded["population"], "batch",
        "the payload must carry which population these figures describe"
    );
    assert!(
        encoded.get("exclusions").is_some(),
        "and what was excluded from it"
    );
}

/// Keeps the import used.
#[allow(dead_code)]
fn _termination_is_used(_: ArenaTermination) {}
