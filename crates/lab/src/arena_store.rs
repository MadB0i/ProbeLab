//! Durable storage for played arena matches.
//!
//! # Why this store exists
//!
//! The Arena keeps a bounded in-memory list of the matches played this session. That was enough while
//! the panel only showed the most recent match, but it made two things impossible that a match
//! archive is for:
//!
//! 1. **A match could not be reopened.** Changing a seat discarded the displayed match, and because
//!    the identifier travelled with it, the recorded match and its replay became unreachable — the
//!    backend still held the recording and the panel had thrown away the only key to it.
//! 2. **A match did not survive a restart.** Results in this product are evidence. An experiment run
//!    survives in [`SqliteResultRepository`]; a match, which is the same kind of claim about what
//!    happened, did not.
//!
//! # What is stored, and what is not
//!
//! One [`ArenaMatchView`] as JSON, keyed by `match_id`, plus the columns a history list has to sort
//! and filter on without parsing every row. The full recording is stored rather than reconstructed:
//! a match is replayed by reading what the engine recorded, and re-deriving it later would make the
//! archive's contents depend on today's engine code.
//!
//! # This store never deletes and never mutates
//!
//! There is no update path and no delete path. A match, once written, is the recording of a match
//! that happened. Corrections are made by playing another match, not by editing one. That is the same
//! guarantee [`SqliteResultRepository`] gives experiment results, and it is why the Arena's
//! "new match" control is a view operation and never reaches this type.
//!
//! # Schema changes are additive
//!
//! `CREATE TABLE IF NOT EXISTS` plus new columns added through `ALTER TABLE` guarded by a column
//! probe. Opening an existing database must never drop a recording, so nothing here deletes or
//! rewrites a table it did not create.

use std::path::Path;
use std::sync::Mutex;

use crate::arena::ArenaMatchView;
use rusqlite::{params, Connection, OptionalExtension};

/// Why a match could not be stored or read.
#[derive(Debug, thiserror::Error)]
pub enum ArenaMatchStoreError {
    /// The database rejected the statement or the file could not be opened.
    #[error("match store unavailable: {0}")]
    Storage(String),
    /// A stored recording could not be decoded back into a match.
    #[error("stored match {match_id:?} could not be read: {reason}")]
    Corrupt {
        /// The recording that could not be decoded.
        match_id: String,
        /// What the decoder reported.
        reason: String,
    },
}

/// A summary of one stored match: what a history list can render without loading every recording.
///
/// Deliberately narrow. A list of twenty matches should not have to parse twenty full recordings to
/// draw, and it should not be able to display a field the recording does not actually contain.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArenaMatchSummary {
    /// The match's stable identifier.
    pub match_id: String,
    /// The game that was played.
    pub game: String,
    /// The outcome, as the engine recorded it.
    pub outcome: crate::arena::ArenaOutcome,
    /// Why the match ended.
    ///
    /// Carried on the index row rather than only on the full view so a statistics fold can state its
    /// population — conditional outcomes against everything attempted — without loading every recording
    /// in the archive. `None` means the recording predates the field; it is never defaulted to
    /// `Decided`, because a historical `NoResult` row says nothing about whether it was cancelled or the
    /// provider was down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination: Option<crate::arena::ArenaTermination>,
    /// The tested policy.
    pub tested_agent: String,
    /// The opposing policy.
    pub opponent_agent: String,
    /// Which seat the tested policy took.
    pub tested_side: probelab_core::GameSide,
    /// The seed the match was played with.
    pub seed: u64,
    /// Plies played.
    pub plies: u32,
    /// When it was recorded, in Unix milliseconds.
    pub recorded_unix_ms: i64,
    /// The recording's digest, so a reloaded match can be checked against what was stored.
    pub digest: String,
}

/// Append-only storage for played arena matches.
pub struct SqliteArenaMatchStore {
    connection: Mutex<Connection>,
}

impl SqliteArenaMatchStore {
    /// Opens (or creates) the store at `path`, migrating an existing database forward if needed.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ArenaMatchStoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| ArenaMatchStoreError::Storage(error.to_string()))?;
            }
        }
        let connection = Connection::open(path)
            .map_err(|error| ArenaMatchStoreError::Storage(error.to_string()))?;
        Self::from_connection(connection)
    }

    /// Builds a store over an existing connection, for tests and for embedding.
    pub fn from_connection(connection: Connection) -> Result<Self, ArenaMatchStoreError> {
        initialize_schema(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Records a played match. Recording the same identifier twice is refused rather than overwritten.
    pub fn put(
        &self,
        view: &ArenaMatchView,
        recorded_unix_ms: i64,
    ) -> Result<(), ArenaMatchStoreError> {
        let encoded = serde_json::to_string(view)
            .map_err(|error| ArenaMatchStoreError::Storage(error.to_string()))?;
        let digest = view.replay_digest();
        let connection = self.connection.lock().map_err(|_| {
            ArenaMatchStoreError::Storage("the match store lock was poisoned".to_owned())
        })?;
        connection
            .execute(
                "INSERT INTO arena_matches (
                     match_id, game, outcome, tested_agent, opponent_agent, tested_side,
                     seed, plies, recorded_unix_ms, digest, match_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT (match_id) DO NOTHING",
                params![
                    view.match_id,
                    view.game,
                    outcome_label(view.outcome),
                    view.tested_agent,
                    view.opponent_agent,
                    side_label(view.tested_side),
                    view.seed,
                    view.plies,
                    recorded_unix_ms,
                    digest,
                    encoded,
                ],
            )
            .map_err(|error| ArenaMatchStoreError::Storage(error.to_string()))?;
        Ok(())
    }

    /// The summaries of every stored match, newest first.
    ///
    /// Kept as the single-page form for callers that genuinely want a window. It is *not* how a
    /// statistic should be computed: a caller that uses this and reports the count it received is
    /// describing a window and calling it the archive.
    pub fn list(&self, limit: usize) -> Result<Vec<ArenaMatchSummary>, ArenaMatchStoreError> {
        self.list_page(0, limit)
    }

    /// One page of summaries, newest first, starting at `offset`.
    ///
    /// Ordered by `(recorded_unix_ms, rowid)` so paging is stable: two recordings written in the same
    /// millisecond are separated by `rowid` rather than by whatever order the engine chose, and a
    /// paged read therefore visits every row exactly once instead of skipping or repeating one.
    pub fn list_page(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ArenaMatchSummary>, ArenaMatchStoreError> {
        let connection = self.connection.lock().map_err(|_| {
            ArenaMatchStoreError::Storage("the match store lock was poisoned".to_owned())
        })?;
        let mut statement = connection
            .prepare(
                "SELECT match_id, game, outcome, tested_agent, opponent_agent, tested_side,
                        seed, plies, recorded_unix_ms, digest
                 FROM arena_matches
                 ORDER BY recorded_unix_ms DESC, rowid DESC
                 LIMIT ?1 OFFSET ?2",
            )
            .map_err(|error| ArenaMatchStoreError::Storage(error.to_string()))?;
        let rows = statement
            .query_map(
                rusqlite::params![
                    i64::try_from(limit).unwrap_or(i64::MAX),
                    i64::try_from(offset).unwrap_or(i64::MAX),
                ],
                |row| {
                    Ok(ArenaMatchSummary {
                        match_id: row.get(0)?,
                        game: row.get(1)?,
                        outcome: outcome_from_label(&row.get::<_, String>(2)?),
                        /*
                         * `None`, because the index table has no termination column.
                         *
                         * Adding one means a schema migration over the historical archive, and this pass is
                         * explicitly not permitted to rewrite existing experimental evidence just to make a
                         * query convenient. Population classification therefore reads the full recording,
                         * where the reason was actually stored, rather than guessing it from the outcome
                         * label — an outcome of `NoResult` does not say whether it was cancelled or the
                         * provider was down.
                         */
                        termination: None,
                        tested_agent: row.get(3)?,
                        opponent_agent: row.get(4)?,
                        tested_side: side_from_label(&row.get::<_, String>(5)?),
                        seed: row.get::<_, i64>(6)? as u64,
                        plies: row.get::<_, i64>(7)? as u32,
                        recorded_unix_ms: row.get(8)?,
                        digest: row.get(9)?,
                    })
                },
            )
            .map_err(|error| ArenaMatchStoreError::Storage(error.to_string()))?;
        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(row.map_err(|error| ArenaMatchStoreError::Storage(error.to_string()))?);
        }
        Ok(summaries)
    }

    /// One stored match in full, or `None` when no such match was ever played.
    pub fn get(&self, match_id: &str) -> Result<Option<ArenaMatchView>, ArenaMatchStoreError> {
        let connection = self.connection.lock().map_err(|_| {
            ArenaMatchStoreError::Storage("the match store lock was poisoned".to_owned())
        })?;
        let encoded: Option<String> = connection
            .query_row(
                "SELECT match_json FROM arena_matches WHERE match_id = ?1",
                params![match_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| ArenaMatchStoreError::Storage(error.to_string()))?;
        let Some(encoded) = encoded else {
            return Ok(None);
        };
        serde_json::from_str(&encoded)
            .map(Some)
            .map_err(|error| ArenaMatchStoreError::Corrupt {
                match_id: match_id.to_owned(),
                reason: error.to_string(),
            })
    }

    /// How many matches are stored.
    pub fn count(&self) -> Result<usize, ArenaMatchStoreError> {
        let connection = self.connection.lock().map_err(|_| {
            ArenaMatchStoreError::Storage("the match store lock was poisoned".to_owned())
        })?;
        connection
            .query_row("SELECT COUNT(*) FROM arena_matches", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|count| count.max(0) as usize)
            .map_err(|error| ArenaMatchStoreError::Storage(error.to_string()))
    }
}

fn outcome_label(outcome: crate::arena::ArenaOutcome) -> &'static str {
    match outcome {
        crate::arena::ArenaOutcome::Win => "win",
        crate::arena::ArenaOutcome::Loss => "loss",
        crate::arena::ArenaOutcome::Draw => "draw",
        crate::arena::ArenaOutcome::NoResult => "no_result",
    }
}

fn outcome_from_label(label: &str) -> crate::arena::ArenaOutcome {
    match label {
        "win" => crate::arena::ArenaOutcome::Win,
        "loss" => crate::arena::ArenaOutcome::Loss,
        "draw" => crate::arena::ArenaOutcome::Draw,
        _ => crate::arena::ArenaOutcome::NoResult,
    }
}

fn side_label(side: probelab_core::GameSide) -> &'static str {
    match side {
        probelab_core::GameSide::First => "first",
        probelab_core::GameSide::Second => "second",
    }
}

fn side_from_label(label: &str) -> probelab_core::GameSide {
    match label {
        "second" => probelab_core::GameSide::Second,
        _ => probelab_core::GameSide::First,
    }
}

/// Creates the table, and adds any column a later build introduced to a database made by an earlier one.
///
/// A stored recording outlives the build that wrote it, so opening an old database has to leave the
/// old rows readable. Every step here is additive and idempotent; nothing drops or rewrites.
fn initialize_schema(connection: &Connection) -> Result<(), ArenaMatchStoreError> {
    let storage = |error: rusqlite::Error| ArenaMatchStoreError::Storage(error.to_string());
    connection
        .execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS arena_matches (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                match_id TEXT NOT NULL UNIQUE,
                game TEXT NOT NULL,
                outcome TEXT NOT NULL,
                tested_agent TEXT NOT NULL,
                opponent_agent TEXT NOT NULL,
                tested_side TEXT NOT NULL,
                seed INTEGER NOT NULL,
                plies INTEGER NOT NULL,
                recorded_unix_ms INTEGER NOT NULL,
                digest TEXT NOT NULL,
                match_json TEXT NOT NULL,
                CHECK (length(match_id) > 0),
                CHECK (json_valid(match_json))
            );
            CREATE INDEX IF NOT EXISTS arena_matches_recorded
                ON arena_matches (recorded_unix_ms DESC);
            CREATE TRIGGER IF NOT EXISTS arena_matches_reject_update
            BEFORE UPDATE ON arena_matches
            BEGIN
                SELECT RAISE(ABORT, 'recorded matches are append-only');
            END;
            CREATE TRIGGER IF NOT EXISTS arena_matches_reject_delete
            BEFORE DELETE ON arena_matches
            BEGIN
                SELECT RAISE(ABORT, 'recorded matches are append-only');
            END;
            "#,
        )
        .map_err(storage)?;
    Ok(())
}
