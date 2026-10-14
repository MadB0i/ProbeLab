//! Batch evaluation over recorded arena matches.
//!
//! # What this file adds
//!
//! One command that plays a sequence of matches and writes each to the durable archive as it finishes,
//! one that stops a running batch, and one that reads statistics back out of that archive.
//!
//! # Why the statistics are read from the archive rather than from what the batch just played
//!
//! The batch holds the full `ArenaMatchView` for every match it played, so it could summarise itself.
//! It does not: it archives each match, then *re-reads* the recordings it wrote and computes from
//! those. The re-read costs one query per match and buys the property that matters — the numbers on the
//! panel are provably the numbers in the database, including after a restart. A batch that summarised
//! its own memory could report a total the archive does not support: after a crash part-way through,
//! the panel would show six matches while the database held four.
//!
//! # Why every match gets a fresh seat
//!
//! A seat holds per-episode state — the grid policies reset between episodes for exactly this reason —
//! so one seat object reused across a batch would let match two start from wherever match one left
//! off. `build_seat` is therefore called per match. Both seat *kinds* are still validated before the
//! first match runs, so an unbuildable provider is a refusal rather than a batch that fails on its
//! first match and looks like a model that lost it.
//!
//! # What a batch is not
//!
//! It is not a rating and it does not rank agents. A batch produces counts for one game and one
//! pairing; turning those into a claim about which agent is better is `comparison`'s job, and it needs
//! evidence this file does not gather. `alternate_sides` defaults on in the panel because a seat that
//! only ever plays first has been given a handicap rather than tested — but it is a switch, and the
//! report says which way it was set.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use probelab_core::{GameSide, TelemetryRecorder};
use probelab_lab::{
    ArenaMatchView, ArenaRunner, ArenaStatistics, SqliteArenaMatchStore, TurnTelemetry,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::arena::{
    build_seat, new_match_id, parse_game, unix_milliseconds, ArenaCommandError, ArenaSeatRequest,
    TurnEnvelope, ARENA_MATCH_EVENT, ARENA_TURN_EVENT,
};
use crate::AppState;

/// Progress of a running batch.
pub const ARENA_BATCH_EVENT: &str = "probe-lab/arena-batch";

/// The most matches one batch may play.
///
/// A bound, not a suggestion. A batch against a real model is minutes per match, so an unbounded count
/// is an unbounded wait with no predictable end and no way for the panel to warn about it. Fifty is far
/// more than anyone reads in a sitting and far less than an accidental unbounded loop.
pub const MAX_BATCH_MATCHES: usize = 50;

/// The smallest batch worth running.
///
/// One, not zero: a caller that genuinely wants a single recording is not refused, and the statistics
/// label it a small sample, which is the honest description of one match.
pub const MIN_BATCH_MATCHES: usize = 1;

/// How many archived recordings the statistics command will read.
pub const STATISTICS_READ_LIMIT: usize = 200;

/// A request to play a sequence of matches.
///
/// The same shape as a single-match request plus a count, because a batch is not a different kind of
/// thing: it is the same match, repeated, with the tested seat taking the other side on alternate runs.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArenaBatchRequest {
    /// Which game every match in the batch plays.
    pub game: String,
    /// The seat under test.
    pub tested_agent: ArenaSeatRequest,
    /// The opposing seat.
    pub opponent_agent: ArenaSeatRequest,
    /// How many matches to play, within `MIN_BATCH_MATCHES..=MAX_BATCH_MATCHES`.
    pub matches: usize,
    /// The first seed. Match `n` uses `seed + n`, so every match is independently reproducible.
    pub seed: u64,
    /// Whether the tested seat alternates sides between matches.
    pub alternate_sides: bool,
}

/// How a batch is doing, emitted after every match.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArenaBatchProgress {
    /// The batch's identifier, and what [`cancel_arena_batch`] takes.
    pub batch_id: String,
    /// Matches finished so far, including the one just played.
    pub completed: usize,
    /// Matches requested.
    pub requested: usize,
    /// The identifier of the match just archived.
    pub match_id: String,
    /// What that match produced, as the backend's own label.
    pub outcome: String,
    /// Whether the operator stopped the batch on or just after this match.
    pub stopped: bool,
}

/// The result of a batch.
///
/// `statistics` covers *this batch's* recordings only, re-read from the archive after the last match
/// was written. The archive-wide view is [`arena_statistics`]'s job.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArenaBatchReport {
    /// The batch's identifier.
    pub batch_id: String,
    /// Matches actually played. Lower than `requested` when the batch was stopped.
    pub completed: usize,
    /// Matches requested.
    pub requested: usize,
    /// Whether the batch stopped early.
    pub stopped: bool,
    /// Whether the tested seat alternated sides.
    pub alternate_sides: bool,
    /// Every match played, in order.
    pub match_ids: Vec<String>,
    /// Statistics over exactly those recordings.
    pub statistics: ArenaStatistics,
}

/// Plays a batch of matches, archiving each as it finishes.
///
/// Stopping takes effect at the next ply boundary of the match in flight *and* prevents any further
/// match from starting, so a stop during a long model match ends the batch rather than ending one
/// match and beginning the next.
///
/// Each match is archived before the next one starts. That ordering is the point: a batch interrupted
/// halfway leaves a prefix in the database that is a true record of what was played, rather than a
/// partial batch whose summary cannot be reconstructed.
#[tauri::command(rename_all = "snake_case")]
pub async fn run_arena_batch(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ArenaBatchRequest,
) -> Result<ArenaBatchReport, ArenaCommandError> {
    let game = parse_game(&request.game)?;
    if !(MIN_BATCH_MATCHES..=MAX_BATCH_MATCHES).contains(&request.matches) {
        return Err(ArenaCommandError {
            code: "batch_too_large".to_owned(),
            message: format!(
                "a batch must play between {MIN_BATCH_MATCHES} and {MAX_BATCH_MATCHES} matches; \
                 {} was asked for",
                request.matches
            ),
        });
    }
    let tested_label = request.tested_agent.seat_label();
    let opponent_label = request.opponent_agent.seat_label();
    if tested_label == opponent_label {
        // Refused rather than run. A seat playing itself produces a deterministic, meaningless
        // distribution, and the refusal names the mistake instead of returning a confident-looking row.
        return Err(ArenaCommandError {
            code: "seat_against_itself".to_owned(),
            message: "the tested seat and the opponent are the same agent; a match between two \
                      copies of one agent measures the engine, not either seat"
                .to_owned(),
        });
    }

    // Validate both seat kinds before anything runs, so a bad provider configuration is refused up
    // front rather than half-way through a batch that has already archived real matches.
    build_seat(&request.tested_agent)?;
    build_seat(&request.opponent_agent)?;

    let batch_id = new_match_id(game, request.seed);
    let cancel = state.arena.begin(&batch_id)?;
    let recorder: Arc<dyn TelemetryRecorder> = state.recorder.clone();
    let archive = state.arena_matches.clone();
    let runner = ArenaRunner::new();
    let requested = request.matches;
    let alternate = request.alternate_sides;
    let base_seed = request.seed;

    // Cloned for the worker, because the report is assembled on this side once the worker returns and
    // the identifier is needed for both. A `String` clone here is cheaper than a lock.
    let batch_id_for_worker = batch_id.clone();
    let archive_for_worker = Arc::clone(&archive);

    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let mut played_ids: Vec<String> = Vec::with_capacity(requested);
        let mut stopped = false;
        let batch_id = batch_id_for_worker;
        let archive = &archive_for_worker;

        for index in 0..requested {
            // Checked before each match, so a stop during match three does not start match four.
            if cancel.load(Ordering::SeqCst) {
                stopped = true;
                break;
            }

            let seed = base_seed.wrapping_add(index as u64);
            let tested_side = if alternate && index % 2 == 1 {
                GameSide::Second
            } else {
                GameSide::First
            };
            let match_id = new_match_id(game, seed);
            let spec = probelab_lab::ArenaMatchSpec::new(
                game,
                &tested_label,
                &opponent_label,
                tested_side,
                seed,
                &match_id,
            );

            // Fresh seats per match: a seat carries per-episode state that must not leak between games.
            let tested = match build_seat(&request.tested_agent) {
                Ok(seat) => seat,
                Err(error) => {
                    tracing::error!(code = %error.code, message = %error.message, "a batch seat could not be built mid-batch");
                    stopped = true;
                    break;
                }
            };
            let opponent = match build_seat(&request.opponent_agent) {
                Ok(seat) => seat,
                Err(error) => {
                    tracing::error!(code = %error.code, message = %error.message, "a batch opponent could not be built mid-batch");
                    stopped = true;
                    break;
                }
            };

            let app_for_turns = app.clone();
            let id_for_turns = match_id.clone();
            let mut on_turn = move |turn: &TurnTelemetry| {
                if let Err(error) = app_for_turns.emit(
                    ARENA_TURN_EVENT,
                    TurnEnvelope {
                        match_id: id_for_turns.clone(),
                        turn: turn.clone(),
                    },
                ) {
                    tracing::warn!(
                        error = %error,
                        "a live arena turn could not be emitted; the match itself is unaffected"
                    );
                }
            };

            let played = runner.play(
                &spec,
                tested,
                opponent,
                Some(recorder.clone()),
                Some(cancel.as_ref()),
                &mut on_turn,
            );

            match played {
                Ok(played) => {
                    if let Err(error) = archive.put(&played.view, unix_milliseconds()) {
                        // The match happened and the batch keeps going, but this recording is not in
                        // the archive, so it will not appear in the statistics below. Logged rather
                        // than swallowed: a batch whose summary outruns its archive is exactly the
                        // failure this command is shaped to avoid.
                        tracing::error!(
                            match_id = %played.view.match_id,
                            error = %error,
                            "a batch match could not be written to the durable archive; it will not \
                             appear in this batch's statistics and will be lost when the app closes"
                        );
                    }
                    if let Err(error) = app.emit(ARENA_MATCH_EVENT, &played.view) {
                        tracing::warn!(
                            error = %error,
                            "a completed batch match could not be emitted; the archive still holds it"
                        );
                    }
                    let stopped_here = cancel.load(Ordering::SeqCst);
                    emit_progress(
                        &app,
                        &batch_id,
                        played_ids.len() + 1,
                        requested,
                        &match_id,
                        played.view.outcome.label(),
                        stopped_here,
                    );
                    played_ids.push(played.view.match_id.clone());
                    if stopped_here {
                        stopped = true;
                    }
                }
                Err(error) => {
                    // `runner.play` returns a `NoResult` view with a failure reason rather than an
                    // error, so this arm is a fault outside the match. Either way the matches already
                    // archived are kept and the batch ends here.
                    stopped = true;
                    tracing::error!(
                        match_id = %match_id,
                        error = %error,
                        "a batch match could not be played; the batch stopped here and the matches \
                         already archived are kept"
                    );
                    break;
                }
            }
        }
        (played_ids, stopped)
    })
    .await;

    // The handle is released whichever way the batch ended, so a finished batch can never stop
    // whatever starts next under the same identifier.
    state.arena.finish(&batch_id);

    let (played_ids, stopped) = outcome.map_err(|error| ArenaCommandError {
        code: "engine_failed".to_owned(),
        message: format!("the arena worker thread could not be joined: {error}"),
    })?;

    // Read back from the archive rather than taken from the values this batch still holds, so what is
    // reported is provably what the database holds. See the module comment.
    let statistics = statistics_for(&archive, &played_ids);

    Ok(ArenaBatchReport {
        batch_id,
        completed: played_ids.len(),
        requested,
        stopped,
        alternate_sides: alternate,
        match_ids: played_ids,
        statistics,
    })
}

/// Stops a running batch.
///
/// Returns `false` when nothing with that identifier is running, so a stop control cannot report
/// success for a batch that already finished.
#[tauri::command(rename_all = "snake_case")]
pub fn cancel_arena_batch(state: State<'_, AppState>, batch_id: String) -> bool {
    state.arena.cancel(&batch_id)
}

/// Statistics over every recorded match, grouped by game and pairing.
///
/// Reads the durable archive, never the in-memory session surface: that surface holds at most twenty
/// matches and disappears on restart, so statistics drawn from it would be both truncated and
/// temporary. A statistic that vanished when the application closed would not be a statistic.
#[tauri::command]
pub fn arena_statistics(state: State<'_, AppState>) -> Result<ArenaStatistics, ArenaCommandError> {
    let archive = state.arena_matches.clone();
    let views = read_recordings(&archive, STATISTICS_READ_LIMIT);
    Ok(probelab_lab::arena_statistics(&views))
}

/// Reads up to `limit` recordings from the archive.
///
/// A row whose recording cannot be read is skipped rather than failing the whole read. One corrupt
/// recording should cost one statistic, not the entire history — and the skip is logged, so the gap
/// between `matches_read` and the archive's row count is visible rather than silent.
fn read_recordings(archive: &SqliteArenaMatchStore, limit: usize) -> Vec<ArenaMatchView> {
    let summaries = match archive.list(limit) {
        Ok(summaries) => summaries,
        Err(error) => {
            tracing::error!(
                error = %error,
                "the match archive could not be listed; these statistics are empty rather than \
                 wrong, and the panel shows an empty history rather than zeroes"
            );
            return Vec::new();
        }
    };

    let mut views = Vec::with_capacity(summaries.len());
    for summary in summaries {
        match archive.get(&summary.match_id) {
            Ok(Some(view)) => views.push(view),
            Ok(None) => {}
            Err(error) => tracing::error!(
                match_id = %summary.match_id,
                error = %error,
                "an archived match could not be read and is excluded from these statistics"
            ),
        }
    }
    views
}

/// Computes statistics over exactly the recordings named, read back from the archive.
fn statistics_for(archive: &SqliteArenaMatchStore, match_ids: &[String]) -> ArenaStatistics {
    let mut views = Vec::with_capacity(match_ids.len());
    for match_id in match_ids {
        match archive.get(match_id) {
            Ok(Some(view)) => views.push(view),
            Ok(None) => tracing::error!(
                match_id = %match_id,
                "a batch match is absent from the archive it was written to; it will not appear in \
                 this batch's statistics"
            ),
            Err(error) => tracing::error!(
                match_id = %match_id,
                error = %error,
                "a batch match could not be read back and is excluded from this batch's statistics"
            ),
        }
    }
    probelab_lab::arena_statistics(&views)
}

/// Emits batch progress, logging rather than propagating a delivery failure.
///
/// The archive already holds the match at this point, so a frontend that missed the event still finds
/// the recording on its next read. Failing the batch over a missed notification would discard a real
/// result to fix a cosmetic problem.
fn emit_progress(
    app: &AppHandle,
    batch_id: &str,
    completed: usize,
    requested: usize,
    match_id: &str,
    outcome: &str,
    stopped: bool,
) {
    if let Err(error) = app.emit(
        ARENA_BATCH_EVENT,
        ArenaBatchProgress {
            batch_id: batch_id.to_owned(),
            completed,
            requested,
            match_id: match_id.to_owned(),
            outcome: outcome.to_owned(),
            stopped,
        },
    ) {
        tracing::warn!(
            error = %error,
            "batch progress could not be emitted; the match is archived and the caller still receives \
             the final report"
        );
    }
}
