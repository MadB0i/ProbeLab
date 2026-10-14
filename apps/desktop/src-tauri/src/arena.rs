//! The Arena's command surface.
//!
//! # What this module does
//!
//! Offers the games this build can play, the agents that can legally play them, and one command that
//! plays a match. Every result it returns is a projection of what the engine's `play_match` produced.
//! This module decides nothing: not the winner, not a legal move, not a score.
//!
//! # What it deliberately does not offer
//!
//! A "submit a move" command. There is no human-turn path, and inventing one would mean a second
//! execution path with its own state transitions and its own validation — in a panel whose entire value
//! is that the environment is the only thing that moves the board. A match is seat against seat, it
//! runs to a terminal state, and the panel shows the recorded result.
//!
//! The commands are also strictly read-mostly. There is no force, no override, and no way to seed a
//! fabricated result: `play_arena_match` runs a real match or returns a typed refusal.
//!
//! # Why a real model match is asynchronous
//!
//! A policy-versus-policy tic-tac-toe match finishes in microseconds. A connect-four match against a
//! depth-bounded search takes seconds. A match against a real model on a laptop takes *minutes* — a
//! 42-ply game with a 0.5B model on CPU is thirty seconds of inference per ply at the low end, and
//! longer if the machine is busy.
//!
//! Running that on the IPC thread would freeze the window: the panel would not paint, the stop button
//! would not respond, and the operating system would mark the app as not responding. So the match
//! runs on a blocking worker, per-turn events stream out to the frontend as they are played, and the
//! command's future resolves when the match reaches a terminal state. Nothing about that changes the
//! engine: the same `play_match` produces the same result whether it is called from a test or from a
//! worker thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use probelab_core::{GameKind, GameSide, TelemetryRecorder};
use probelab_lab::{ArenaMatchView, ArenaRunError, ArenaRunner, TurnTelemetry};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::provider::ModelRunConfig;
use crate::AppState;

/// The event a live match emits on every ply.
pub const ARENA_TURN_EVENT: &str = "probe-lab/arena-turn";

/// The event emitted once when a match reaches its terminal state.
pub const ARENA_MATCH_EVENT: &str = "probe-lab/arena-match";

/// One game the arena can be asked to play.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArenaGameSummary {
    /// Stable identifier.
    pub id: String,
    /// Name for the panel header.
    pub name: String,
    /// One-sentence description.
    pub description: String,
    /// Board width, where the game has a fixed grid.
    pub width: usize,
    /// Board height.
    pub height: usize,
    /// The most plies the game can take.
    pub max_plies: u32,
}

/// One seat the arena can fill, as the seat picker renders it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArenaAgentSummary {
    /// Stable identifier, used to seat the agent.
    pub id: String,
    /// Name for the picker.
    pub name: String,
    /// What this policy is.
    pub description: String,
    /// The category label, so the panel can show it is a ceiling or a baseline.
    pub category_label: String,
    /// Whether the agent can also be seated as the arena's reference opponent.
    pub can_be_opponent: bool,
}

/// One seat in a match request.
///
/// Either a registry identifier or a live model configuration, tagged — because "seat a shipped policy"
/// and "seat a real model" are different capabilities with different failure modes, and a frontend
/// that conflated them would have no way to tell a typo from an unreachable provider.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ArenaSeatRequest {
    /// A policy from the agent registry.
    Policy {
        /// The registry identifier.
        id: String,
    },
    /// A real model reached over a provider adapter.
    Model {
        /// A short label for the seat, so a match between two models is readable in the archive.
        label: String,
        /// The provider configuration. Validated by the same code a benchmark run uses.
        config: ModelRunConfig,
    },
}

impl ArenaSeatRequest {
    /// The label for one seat, for the archive and for telemetry.
    pub fn seat_label(&self) -> String {
        match self {
            ArenaSeatRequest::Policy { id } => id.clone(),
            ArenaSeatRequest::Model { label, .. } => label.clone(),
        }
    }
}

/// A request to play one match.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArenaMatchRequest {
    /// Which game to play.
    pub game: String,
    /// The seat under test.
    pub tested_agent: ArenaSeatRequest,
    /// The opposing seat.
    pub opponent_agent: ArenaSeatRequest,
    /// Which side the tested seat plays, `first` or `second`.
    pub tested_side: String,
    /// The seed, so the match replays.
    pub seed: u64,
}

/// A refusal the panel can display.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArenaCommandError {
    /// A stable code for the UI to branch on.
    pub code: String,
    /// A concise technical description, safe to show a developer.
    pub message: String,
}

impl From<ArenaRunError> for ArenaCommandError {
    fn from(error: ArenaRunError) -> Self {
        let code = match &error {
            ArenaRunError::UnknownGame(_) => "unknown_game",
            ArenaRunError::UnknownAgent(_) => "unknown_agent",
            ArenaRunError::IncompatibleAgent { .. } => "incompatible_agent",
            ArenaRunError::Engine(_) => "engine_failed",
        };
        Self {
            code: code.to_owned(),
            message: error.to_string(),
        }
    }
}

/// The most recent matches, so a finished match stays inspectable.
///
/// Bounded for the same reason the Brain surface is: a forgotten match is better than a panel that
/// grows without limit. Twenty is more than anyone scrolls back through.
#[derive(Clone, Default)]
pub struct ArenaSurface {
    matches: Arc<Mutex<Vec<(String, ArenaMatchView)>>>,
    /// Matches currently running, and the flag that stops them.
    ///
    /// A registry rather than a single flag so two matches cannot cancel each other, and so a
    /// finished match's handle cannot cancel whatever started next. The handle is removed in the same
    /// scope that played the match, which is what makes the "stop this match" button safe to press
    /// twice.
    running: Arc<Mutex<Vec<RunningMatch>>>,
}

/// How many matches this process will retain.
const SESSION_MATCH_LIMIT: usize = 20;

/// One match currently executing, and the flag that stops it.
///
/// A named type rather than a tuple, because this value is pushed, searched, retained against, and
/// read in four places, and a two-element tuple of a string and an Arc is not a thing a reader can
/// check at a glance.
type RunningMatch = (String, Arc<AtomicBool>);

impl ArenaSurface {
    /// Registers a running match and returns the flag that stops it.
    pub(crate) fn begin(&self, match_id: &str) -> Result<Arc<AtomicBool>, ArenaCommandError> {
        let flag = Arc::new(AtomicBool::new(false));
        let mut running = self.running.lock().map_err(|_| ArenaCommandError {
            code: "surface_unavailable".to_owned(),
            message:
                "the arena's run registry was left locked by an earlier fault; matches cannot \
                      be started or stopped"
                    .to_owned(),
        })?;
        running.push((match_id.to_owned(), Arc::clone(&flag)));
        Ok(flag)
    }

    /// Removes a match's handle. Always called when a match finishes, whatever its outcome.
    pub(crate) fn finish(&self, match_id: &str) {
        if let Ok(mut running) = self.running.lock() {
            running.retain(|(id, _)| id != match_id);
        }
    }

    /// Signals a running match to stop at its next ply boundary.
    ///
    /// Returns `false` when nothing with that identifier is running, so a stop button cannot report
    /// success for a match that has already finished — the same rule the experiment cancel uses.
    pub fn cancel(&self, match_id: &str) -> bool {
        let Ok(running) = self.running.lock() else {
            return false;
        };
        let found = running
            .iter()
            .find(|(id, _)| id == match_id)
            .map(|(_, flag)| Arc::clone(flag));
        match found {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// Stores a completed match.
    ///
    /// Failure to store is reported rather than panicked on. A poisoned lock means some other thread
    /// panicked while holding it, which is a real fault worth surfacing -- but panicking here would
    /// turn one recovered fault into an unrecoverable crash loop, taking the whole panel with it.
    pub fn store(&self, match_id: String, view: ArenaMatchView) -> Result<(), ArenaCommandError> {
        let mut matches = self.matches.lock().map_err(|_| ArenaCommandError {
            code: "surface_unavailable".to_owned(),
            message: "the arena's match history was left locked by an earlier fault and cannot be \
                      written to; matches from this session will not be retained"
                .to_owned(),
        })?;
        if matches.len() >= SESSION_MATCH_LIMIT {
            matches.remove(0);
        }
        matches.push((match_id, view));
        Ok(())
    }

    /// The most recent match, or `None` when none has been played.
    ///
    /// A poisoned lock degrades to "no match" rather than aborting. The panel already renders an empty
    /// arena, so the failure is visible in the UI instead of being a process crash.
    pub fn latest(&self) -> Option<ArenaMatchView> {
        self.matches
            .lock()
            .ok()?
            .last()
            .map(|(_, view)| view.clone())
    }

    /// One match by identifier.
    pub fn get(&self, match_id: &str) -> Option<ArenaMatchView> {
        self.matches
            .lock()
            .ok()?
            .iter()
            .find(|(id, _)| id == match_id)
            .map(|(_, view)| view.clone())
    }

    /// How many matches are retained.
    pub fn len(&self) -> usize {
        self.matches
            .lock()
            .map(|matches| matches.len())
            .unwrap_or(0)
    }

    /// Whether any match has been played.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Returns every game the arena can be asked to play.
#[tauri::command(rename_all = "snake_case")]
pub fn list_arena_games() -> Vec<ArenaGameSummary> {
    probelab_lab::available_games()
        .into_iter()
        .map(|game| ArenaGameSummary {
            id: game.id,
            name: game.name,
            description: game.description,
            width: game.width,
            height: game.height,
            max_plies: game.max_plies,
        })
        .collect()
}

/// Returns every policy that can legally be seated in `game`.
///
/// The filter is the registry's own `supported_games`, decided in the backend. A frontend cannot offer
/// a pairing the arena would refuse, and adding a game later cannot surface a policy that has no idea
/// what its rules are.
///
/// A real model is *not* in this list, because a model is not a registry entry — it is a runtime
/// configuration. The panel seats one by sending a `ModelRunConfig`, and the list tells it which
/// policies are available to compare the model against.
#[tauri::command(rename_all = "snake_case")]
pub fn list_arena_agents(game: String) -> Result<Vec<ArenaAgentSummary>, ArenaCommandError> {
    let kind = parse_game(&game)?;
    let registry = probelab_lab::AgentRegistry::built_in();
    Ok(ArenaRunner::new()
        .agents_for(kind)
        .into_iter()
        .map(|spec| ArenaAgentSummary {
            // A policy can occupy the opponent seat only if it has a reference form, which the
            // registry knows and the UI does not.
            can_be_opponent: registry.build_reference(&spec.id, 0).is_ok(),
            id: spec.id,
            name: spec.name,
            description: spec.description,
            category_label: spec.category.label().to_owned(),
        })
        .collect())
}

/// The most recent match, or `None` when none has been played.
///
/// `None` is a real state — an empty arena — and the panel renders it as one rather than as a fault.
///
/// Reads the durable archive first and falls back to the session surface, so a match played before the
/// last restart is still the most recent match rather than an empty arena.
///
/// See [`latest_arena_match`] for why this takes `State<AppState>`.
#[tauri::command(rename_all = "snake_case")]
pub fn latest_arena_match(state: State<'_, AppState>) -> Option<ArenaMatchView> {
    if let Some(latest) = state
        .arena_matches
        .list(1)
        .ok()
        .and_then(|summaries| summaries.into_iter().next())
    {
        if let Ok(Some(view)) = state.arena_matches.get(&latest.match_id) {
            return Some(view);
        }
    }
    state.arena.latest()
}

/// Every stored match, newest first.
///
/// A summary list rather than full recordings: the history list draws one line per match and should
/// not have to decode every full board recording to do it. The full match is fetched on open.
///
/// Bounded on both sides. The caller's `limit` is clamped, so a malformed or hostile value cannot ask
/// the database for an unbounded scan, and the default of 50 is far more than the panel renders.
#[tauri::command(rename_all = "snake_case")]
pub fn list_arena_matches(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<probelab_lab::ArenaMatchSummary>, ArenaCommandError> {
    let limit = limit.unwrap_or(50).clamp(1, 200);
    state
        .arena_matches
        .list(limit)
        .map_err(|error| ArenaCommandError {
            code: "history_unavailable".to_owned(),
            message: format!("the match archive could not be read: {error}"),
        })
}

/// The full recording of one stored match.
///
/// Reads the durable archive, so a match can be reopened after a restart. See [`latest_arena_match`]
/// for why this takes `State<AppState>`.
#[tauri::command(rename_all = "snake_case")]
pub fn get_arena_match(
    state: State<'_, AppState>,
    match_id: String,
) -> Result<ArenaMatchView, ArenaCommandError> {
    if let Ok(Some(view)) = state.arena_matches.get(&match_id) {
        return Ok(view);
    }
    state.arena.get(&match_id).ok_or_else(|| ArenaCommandError {
        code: "not_found".to_owned(),
        message: format!("no match named {match_id:?}"),
    })
}

/// Reconstructs a stored match as a deterministic replay.
///
/// The reconstruction reads recorded data and nothing else: no engine, no provider, no broker. It
/// therefore cannot produce a different result than the recording, and cannot call a model even by
/// accident — which is what makes it a replay rather than a second run. This matters more now than it
/// did: a match against a real model costs real provider time, and "open a recording" must never mean
/// "pay for the same decisions again".
///
/// Reads the durable archive, so a match played in an earlier session can still be replayed.
///
/// See [`latest_arena_match`] for why this takes `State<AppState>`.
#[tauri::command(rename_all = "snake_case")]
pub fn replay_arena_match(
    state: State<'_, AppState>,
    match_id: String,
) -> Result<probelab_lab::Replay, ArenaCommandError> {
    let view = get_arena_match(state, match_id.clone()).map_err(|_| ArenaCommandError {
        code: "not_found".to_owned(),
        message: format!("no match named {match_id:?}"),
    })?;
    Ok(probelab_lab::replay_match(&view))
}

/// Stops a running match at its next ply boundary.
///
/// Returns `false` — not an error — when nothing with that identifier is running. That is the same
/// contract the experiment cancel uses, and for the same reason: a stop button that reports success
/// for a match that already finished teaches the operator nothing about whether their click worked.
#[tauri::command(rename_all = "snake_case")]
pub fn cancel_arena_match(state: State<'_, AppState>, match_id: String) -> bool {
    state.arena.cancel(&match_id)
}

/// Plays one match and returns what the engine produced.
///
/// The match runs on a blocking worker and the command's future resolves when it reaches a terminal
/// state, so a minutes-long real-model match does not freeze the window. Per-turn events are emitted
/// on [`ARENA_TURN_EVENT`] as they are played and the terminal view on [`ARENA_MATCH_EVENT`].
///
/// The engine is still the only thing that decides anything: this function builds two seats, hands them
/// to [`ArenaRunner::play`], and projects what comes back. It cannot steer a move, cannot declare a
/// winner, and cannot skip the environment's legality check.
#[tauri::command(rename_all = "snake_case")]
pub async fn play_arena_match(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ArenaMatchRequest,
) -> Result<ArenaMatchView, ArenaCommandError> {
    let game = parse_game(&request.game)?;
    let side = parse_side(&request.tested_side)?;
    let tested_label = request.tested_agent.seat_label();
    let opponent_label = request.opponent_agent.seat_label();
    let match_id = new_match_id(game, request.seed);

    // Both seats are built *before* anything runs, so a bad provider configuration is refused up
    // front rather than half-way through a match that has already been recorded. A model that cannot
    // be constructed has not answered anything, and a match that stopped on ply one because of a typo
    // in an endpoint would be indistinguishable from a model that lost on ply one.
    let spec = probelab_lab::ArenaMatchSpec::new(
        game,
        &tested_label,
        &opponent_label,
        side,
        request.seed,
        &match_id,
    );
    let runner = ArenaRunner::new();
    let (tested_seat, opponent_seat) = seats(&request.tested_agent, &request.opponent_agent, game)?;

    let cancel = state.arena.begin(&match_id)?;
    let recorder: Arc<dyn TelemetryRecorder> = state.recorder.clone();
    let app_for_turns = app.clone();
    let id_for_turns = match_id.clone();
    let archive = state.arena_matches.clone();

    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let mut on_turn = move |turn: &TurnTelemetry| {
            // The engine's own authoritative board, plus that turn's cost and identity. Emitted from
            // the worker thread, so the frontend can follow a live match rather than waiting minutes
            // for a frozen panel.
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
            tested_seat,
            opponent_seat,
            Some(recorder),
            Some(cancel.as_ref()),
            &mut on_turn,
        );
        (played, cancel)
    })
    .await;

    let (played, cancel) = match outcome {
        Ok(pair) => pair,
        Err(error) => {
            state.arena.finish(&match_id);
            return Err(ArenaCommandError {
                code: "engine_failed".to_owned(),
                message: format!("the arena worker thread could not be joined: {error}"),
            });
        }
    };
    // The handle is released whichever way the match ended, so a finished match can never stop
    // whatever starts next under the same identifier.
    drop(cancel);
    state.arena.finish(&match_id);

    let played = played.map_err(ArenaCommandError::from)?;
    // A match that played successfully but could not be retained in the session surface is still a
    // real result, and the caller is holding it. The failure to retain is reported through the
    // surface's own state rather than by discarding the match.
    state.arena.store(match_id.clone(), played.view.clone())?;
    // The durable archive is written second, and a failure there does not fail the match. The match
    // genuinely happened and the caller is holding the full recording; failing the whole command would
    // discard a real result because a cache could not be written. The loss is logged instead, so the
    // gap between what was played and what is archived is visible rather than silent.
    if let Err(error) = archive.put(&played.view, unix_milliseconds()) {
        tracing::error!(
            match_id = %played.view.match_id,
            error = %error,
            "a played arena match could not be written to the durable archive; it remains in this \
             session only and will be lost when the application closes"
        );
    }
    if let Err(error) = app.emit(ARENA_MATCH_EVENT, &played.view) {
        tracing::warn!(
            error = %error,
            "the completed arena match could not be emitted; the caller still receives it directly"
        );
    }
    Ok(played.view)
}

/// What one live turn carries to the frontend.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TurnEnvelope {
    pub(crate) match_id: String,
    pub(crate) turn: TurnTelemetry,
}

/// Checks that both seats are legal for `game`, then builds them.
///
/// A registry policy is checked against the registry's own `supported_games`, which is what keeps the
/// panel's list and the arena's capabilities the same set — the decision is made in the backend, not
/// by a frontend that could be wrong. A model seat is legal for connect four and for nothing else,
/// because a nine-ply tic-tac-toe board against a real model measures formatting rather than play.
///
/// The check runs *before* anything is built or played, so an illegal pairing is a refusal rather than
/// a match that fails on ply one.
fn seats(
    request: &ArenaSeatRequest,
    other: &ArenaSeatRequest,
    game: GameKind,
) -> Result<
    (
        probelab_engine::arena::BoxedSeat,
        probelab_engine::arena::BoxedSeat,
    ),
    ArenaCommandError,
> {
    for seat in [request, other] {
        match seat {
            ArenaSeatRequest::Policy { id } => {
                let registry = probelab_lab::AgentRegistry::built_in();
                let spec = registry.spec(id).ok_or_else(|| ArenaCommandError {
                    code: "unknown_agent".to_owned(),
                    message: format!("no agent is registered as {id:?}"),
                })?;
                if !spec.supported_games.contains(&game) {
                    return Err(ArenaCommandError {
                        code: "incompatible_agent".to_owned(),
                        message: format!(
                            "{} cannot play {}; it is registered for {}",
                            spec.name,
                            game.as_str(),
                            describe_games(&spec.supported_games)
                        ),
                    });
                }
            }
            ArenaSeatRequest::Model { .. } if game != GameKind::ConnectFour => {
                return Err(ArenaCommandError {
                    code: "incompatible_agent".to_owned(),
                    message: "a real model can only be seated in connect four; tic-tac-toe's \
                              nine-ply ceiling makes it a protocol probe rather than a measurement"
                        .to_owned(),
                });
            }
            ArenaSeatRequest::Model { .. } => {}
        }
    }
    Ok((build_seat(request)?, build_seat(other)?))
}

/// What an agent declares it can play, for a refusal message.
fn describe_games(games: &[GameKind]) -> String {
    if games.is_empty() {
        return "no arena games".to_owned();
    }
    games
        .iter()
        .map(|game| game.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Builds one seat.
///
/// A policy goes through the registry, so its per-episode reset and its non-privilege claim come from
/// the same code the grid benchmarks use. A model is constructed from its validated configuration by
/// the same code a benchmark run uses, so a model cannot behave differently in the arena than in a
/// grid-world run for reasons that have nothing to do with the game.
pub(crate) fn build_seat(
    seat: &ArenaSeatRequest,
) -> Result<probelab_engine::arena::BoxedSeat, ArenaCommandError> {
    match seat {
        ArenaSeatRequest::Policy { id } => probelab_lab::AgentRegistry::built_in()
            .build_seat(id, 0)
            .map_err(|error| ArenaCommandError {
                code: "unknown_agent".to_owned(),
                message: error.to_string(),
            }),
        ArenaSeatRequest::Model { label, config } => {
            let agent = config.build_agent(0).map_err(|error| ArenaCommandError {
                code: "model_unavailable".to_owned(),
                message: error.to_string(),
            })?;
            Ok(Box::new(probelab_engine::arena::ModelSeat::new(
                agent, label,
            )))
        }
    }
}

/// Now, in Unix milliseconds. `0` rather than a panic: a clock before the epoch is absurd, and it
/// only orders history, so refusing to record a match over it would be the wrong trade.
pub(crate) fn unix_milliseconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Parses a game identifier, refusing anything this build cannot play.
pub(crate) fn parse_game(game: &str) -> Result<GameKind, ArenaCommandError> {
    match game {
        "tic_tac_toe" => Ok(GameKind::TicTacToe),
        "connect_four" => Ok(GameKind::ConnectFour),
        other => Err(ArenaCommandError {
            code: "unknown_game".to_owned(),
            message: format!("this build cannot play {other:?}"),
        }),
    }
}

/// Parses a side.
pub(crate) fn parse_side(side: &str) -> Result<GameSide, ArenaCommandError> {
    match side {
        "first" => Ok(GameSide::First),
        "second" => Ok(GameSide::Second),
        other => Err(ArenaCommandError {
            code: "invalid_side".to_owned(),
            message: format!("{other:?} is not a side; use \"first\" or \"second\""),
        }),
    }
}

/// A match identifier: the game and seed it was played with, plus a timestamp and counter that make
/// each play unique.
///
/// The identifier doubles as the telemetry `run_id`, and the telemetry schema enforces a gap-free
/// sequence per run. A seed-derived id would therefore reuse a run that already holds events, and the
/// second play would be rejected before it started. The timestamp is taken from the wall clock, so it
/// stays unique across restarts — a counter alone would reset to 1 and collide with the run a
/// previous session already recorded under the same seed. The counter disambiguates two matches that
/// land in the same millisecond.
///
/// The game is part of the prefix, not a hard-coded `"ttt"`. A hard-coded prefix on a build that also
/// plays connect four would put both games' matches under a name that says tic-tac-toe, which is a lie
/// in the one field an operator uses to tell two recordings apart.
static MATCH_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub(crate) fn new_match_id(game: GameKind, seed: u64) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let ordinal = MATCH_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("arena-{}-{seed}-{millis}-{ordinal}", game.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_games_are_exposed() {
        let games = list_arena_games();
        let ids: Vec<&str> = games.iter().map(|game| game.id.as_str()).collect();
        assert_eq!(ids, vec!["tic_tac_toe", "connect_four"]);
        assert_eq!((games[0].width, games[0].height), (3, 3));
        assert_eq!((games[1].width, games[1].height), (7, 6));
    }

    #[test]
    fn an_unimplemented_game_is_refused_by_name() {
        for game in ["rps", "battleship", "not-a-game"] {
            let error = parse_game(game).expect_err("should be refused");
            assert_eq!(error.code, "unknown_game");
            assert!(
                error.message.contains(game),
                "the message should name what was asked for"
            );
        }
    }

    #[test]
    fn an_invalid_side_is_refused_with_the_accepted_values() {
        let error = parse_side("third").expect_err("should be refused");
        assert_eq!(error.code, "invalid_side");
        assert!(error.message.contains("first") && error.message.contains("second"));
        assert_eq!(parse_side("first").expect("first"), GameSide::First);
        assert_eq!(parse_side("second").expect("second"), GameSide::Second);
    }

    #[test]
    fn the_agent_catalog_is_filtered_by_the_backend() {
        let agents = list_arena_agents("tic_tac_toe".to_owned()).expect("catalog");
        let ids: Vec<&str> = agents.iter().map(|agent| agent.id.as_str()).collect();
        assert!(ids.contains(&"perfect_tictactoe"));
        assert!(ids.contains(&"first_legal_tictactoe"));
        assert!(
            !ids.contains(&"optimal"),
            "a grid policy must not be offered a board"
        );

        let connect_four = list_arena_agents("connect_four".to_owned()).expect("catalog");
        let ids: Vec<&str> = connect_four.iter().map(|agent| agent.id.as_str()).collect();
        assert!(ids.contains(&"depth_bounded_connect_four"));
        assert!(ids.contains(&"first_legal_connect_four"));
        assert!(
            !ids.contains(&"perfect_tictactoe"),
            "a tic-tac-toe policy must not be offered a connect-four board"
        );
    }

    #[test]
    fn a_policy_is_reported_as_able_to_take_the_opponent_seat() {
        let agents = list_arena_agents("tic_tac_toe".to_owned()).expect("catalog");
        for agent in &agents {
            assert!(
                agent.can_be_opponent,
                "{} is a policy and should be usable as the reference opponent",
                agent.id
            );
            assert!(!agent.category_label.is_empty());
        }
    }

    #[test]
    fn an_empty_surface_reports_empty_rather_than_failing() {
        let surface = ArenaSurface::default();
        assert!(surface.latest().is_none());
        assert!(surface.is_empty());
        assert_eq!(surface.len(), 0);
    }

    #[test]
    fn a_stored_match_stays_reachable_after_another_is_played() {
        let surface = ArenaSurface::default();
        let first = sample_view("arena-1");
        let second = sample_view("arena-2");
        surface
            .store("arena-1".to_owned(), first)
            .expect("store first");
        surface
            .store("arena-2".to_owned(), second)
            .expect("store second");
        assert_eq!(surface.latest().expect("latest").match_id, "arena-2");
        assert_eq!(surface.get("arena-1").expect("first").match_id, "arena-1");
    }

    #[test]
    fn the_match_history_is_bounded() {
        let surface = ArenaSurface::default();
        for index in 0..25 {
            surface
                .store(format!("m-{index}"), sample_view(&format!("m-{index}")))
                .expect("store");
        }
        assert!(
            surface.len() <= SESSION_MATCH_LIMIT,
            "history grew unbounded: {}",
            surface.len()
        );
    }

    /// A cancelled match stops, and only the match that was asked for.
    ///
    /// The second half is the point. Two matches running at once is not a hypothetical in a panel
    /// with a stop button, and a shared flag would let pressing stop on one silently abort the other.
    #[test]
    fn cancelling_signals_only_the_named_match() {
        let surface = ArenaSurface::default();
        let first = surface.begin("match-a").expect("begin a");
        let second = surface.begin("match-b").expect("begin b");
        assert!(!first.load(Ordering::SeqCst));
        assert!(!second.load(Ordering::SeqCst));

        assert!(surface.cancel("match-a"), "a running match reports success");
        assert!(first.load(Ordering::SeqCst));
        assert!(
            !second.load(Ordering::SeqCst),
            "stopping one match must not touch another"
        );
        // Pressing stop again while the match is still winding down still reports success: it *is*
        // running, and it has been asked to stop. Reporting failure here would tell the operator their
        // second click did nothing, which is true but not useful — the match is on its way out.
        assert!(surface.cancel("match-a"));

        surface.finish("match-a");
        assert!(
            !surface.cancel("match-a"),
            "a finished match cannot be cancelled again"
        );
        assert!(
            !surface.cancel("never-existed"),
            "an unknown identifier is not a success"
        );
        assert!(
            surface.cancel("match-b"),
            "the other match is still running"
        );
    }

    /// A poisoned lock is a runtime condition, and the panel must survive it.
    ///
    /// A `Mutex` is poisoned when a thread panics while holding it. Panicking on the way back in
    /// would convert one recovered fault into a permanent crash loop for the whole panel, because
    /// every subsequent acquisition panics too. So reads degrade to "empty" and writes report a
    /// typed refusal. This asserts that by actually poisoning the lock.
    #[test]
    fn a_poisoned_arena_surface_returns_errors_instead_of_crashing() {
        let surface = ArenaSurface::default();
        poison(&surface.matches);
        assert!(surface.latest().is_none(), "a read should degrade to empty");
        assert!(surface.get("anything").is_none());
        assert_eq!(surface.len(), 0);
        assert!(surface.is_empty());
        let error = surface
            .store("m-1".to_owned(), sample_view("m-1"))
            .expect_err("a write must be a typed refusal, not a panic");
        assert_eq!(error.code, "surface_unavailable");
        assert!(!error.message.is_empty());
        // And the surface stays usable afterwards, which is the point: one fault, not a dead panel.
        assert!(surface.latest().is_none());
    }

    /// Poisons a mutex the way a panicking thread would.
    ///
    /// The guard has to be acquired *inside* the unwinding closure. A `Mutex` is poisoned when a guard
    /// is dropped during unwinding, so a guard held in the enclosing scope is released normally when
    /// `catch_unwind` returns and the lock is never poisoned at all -- a test that looks like it
    /// poisons a lock and does not is worse than no test.
    fn poison(mutex: &Mutex<Vec<(String, ArenaMatchView)>>) {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = mutex.lock().expect("lock to poison");
            panic!("a thread that panicked while holding the arena surface lock");
        }));
        assert!(
            outcome.is_err(),
            "the poisoning panic did not unwind, so the lock was never poisoned"
        );
        assert!(
            mutex.lock().is_err(),
            "the lock should be poisoned; the test is not exercising what it claims"
        );
    }

    #[test]
    fn a_match_id_names_its_game_and_is_unique_per_play() {
        let first = new_match_id(GameKind::TicTacToe, 7);
        let second = new_match_id(GameKind::TicTacToe, 7);
        assert!(first.starts_with("arena-tic_tac_toe-7-"), "got {first}");
        assert_ne!(
            first, second,
            "two plays of the same seed must be two telemetry runs, or the second is rejected"
        );
        // Two games must not share an identifier prefix, or the archive cannot tell them apart.
        let other_game = new_match_id(GameKind::ConnectFour, 7);
        assert!(
            other_game.starts_with("arena-connect_four-7-"),
            "got {other_game}"
        );
    }

    #[test]
    fn an_error_carries_a_stable_code_rather_than_only_prose() {
        let error = ArenaCommandError::from(ArenaRunError::IncompatibleAgent {
            agent: "optimal".to_owned(),
            game: "tic_tac_toe".to_owned(),
        });
        assert_eq!(error.code, "incompatible_agent");
        assert!(error.message.contains("optimal"));
    }

    #[test]
    fn no_command_can_steer_an_authorization_or_a_result() {
        // The dangerous shape for a game panel is a command that writes a result, so this checks the
        // command names directly. If someone later adds a way to seed a fabricated outcome, this fails.
        let source = command_surface_source();
        for forbidden in [
            "fn force",
            "fn override",
            "fn set_result",
            "fn declare_winner",
            "fn submit_move",
        ] {
            assert!(
                !source.contains(forbidden),
                "the arena command surface contains {forbidden:?}; a game panel must never be able \
                 to write a result or inject a turn"
            );
        }
    }

    /// A minimal stored view, for tests about the surface rather than about the game.
    fn sample_view(match_id: &str) -> ArenaMatchView {
        ArenaMatchView {
            match_id: match_id.to_owned(),
            game: "tic_tac_toe".to_owned(),
            game_name: "Tic-Tac-Toe".to_owned(),
            outcome: probelab_lab::ArenaOutcome::Draw,
            tested_agent: "perfect_tictactoe".to_owned(),
            opponent_agent: "perfect_tictactoe".to_owned(),
            tested_seat: probelab_engine::arena::SeatKind::ReferencePolicy,
            opponent_seat: probelab_engine::arena::SeatKind::ReferencePolicy,
            tested_side: GameSide::First,
            seed: 1,
            plies: 9,
            final_board: probelab_core::GameBoard::TicTacToe {
                cells: vec![probelab_core::TicTacToeCell::Empty; 9],
            },
            winner: None,
            winning_line: None,
            illegal_moves: Vec::new(),
            history: Vec::new(),
            turns: Vec::new(),
            timeline: Vec::new(),
            failure: None,
            cancelled: false,
        }
    }

    /// The frontend parses the board and move from the adjacent-tagged wire shape. If serde ever
    /// changes the tag or content keys, this test catches it before the UI silently stops drawing.
    #[test]
    fn game_board_and_move_serialize_with_adjacent_tags() {
        let board = probelab_core::GameBoard::TicTacToe {
            cells: vec![probelab_core::TicTacToeCell::First; 9],
        };
        let value = serde_json::to_value(&board).expect("board serializes");
        assert_eq!(value["game"], "tic_tac_toe");
        assert!(value["state"]["cells"].as_array().is_some());

        let mov = probelab_core::GameMove::TicTacToe(4);
        let value = serde_json::to_value(mov).expect("move serializes");
        assert_eq!(value["game"], "tic_tac_toe");
        assert_eq!(value["move"], 4);
    }

    /// The connect-four wire shape, pinned for the same reason and with the column-major detail the
    /// frontend has to get right.
    ///
    /// The frontend indexes this vector to draw the board, and the schema stores it column-major. If
    /// that ever changed — or if the dimension fields were dropped — the panel would render a
    /// correctly-shaped grid of the wrong cells, which is a silent and very confusing failure. The
    /// `cells` length is asserted against `columns * rows` rather than hard-coded, so the two cannot
    /// disagree.
    #[test]
    fn a_connect_four_board_serializes_column_major_with_consistent_dimensions() {
        let mut cells = vec![probelab_core::ConnectFourCell::Empty; 42];
        cells[0] = probelab_core::ConnectFourCell::First;
        let board = probelab_core::GameBoard::ConnectFour {
            columns: 7,
            rows: 6,
            cells: cells.clone(),
        };
        let value = serde_json::to_value(&board).expect("board serializes");
        assert_eq!(value["game"], "connect_four");
        let state = &value["state"];
        assert_eq!(state["columns"], 7);
        assert_eq!(state["rows"], 6);
        let serialized = state["cells"].as_array().expect("cells is an array");
        assert_eq!(
            serialized.len(),
            7 * 6,
            "the cell count must equal columns times rows, or the frontend cannot index it safely"
        );
        // Column-major: the first cell is the bottom of the leftmost column.
        assert_eq!(serialized[0], "first");

        let mov = probelab_core::GameMove::ConnectFour(3);
        let value = serde_json::to_value(mov).expect("move serializes");
        assert_eq!(value["game"], "connect_four");
        assert_eq!(
            value["move"], 3,
            "a connect-four move names a column index, not a row-major cell index"
        );
    }

    const SURFACE_SOURCE: &str = include_str!("arena.rs");

    fn command_surface_source() -> &'static str {
        SURFACE_SOURCE
            .split("#[cfg(test)]")
            .next()
            .expect("the file has content before its test module")
    }
}
