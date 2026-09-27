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
//! is that the environment is the only thing that moves the board. A match is agent against agent, it
//! runs to completion, and the panel shows the recorded result.
//!
//! The commands are also strictly read-mostly. There is no force, no override, and no way to seed a
//! fabricated result: `play_arena_match` runs a real match or returns a typed refusal.

use std::sync::{Arc, Mutex};

use probelab_core::{GameKind, GameSide, TelemetryRecorder};
use probelab_lab::{ArenaMatchView, ArenaRunError, ArenaRunner};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::AppState;

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

/// One agent, as the arena's seat picker renders it.
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

/// A request to play one match.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArenaMatchRequest {
    /// Which game to play.
    pub game: String,
    /// The agent under test.
    pub tested_agent: String,
    /// The opponent policy.
    pub opponent_agent: String,
    /// Which side the tested agent plays, `first` or `second`.
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
}

impl ArenaSurface {
    /// Stores a completed match.
    pub fn store(&self, match_id: String, view: ArenaMatchView) {
        let mut matches = self.matches.lock().expect("arena surface lock");
        if matches.len() >= 20 {
            matches.remove(0);
        }
        matches.push((match_id, view));
    }

    /// The most recent match, or `None` when none has been played.
    pub fn latest(&self) -> Option<ArenaMatchView> {
        self.matches
            .lock()
            .expect("arena surface lock")
            .last()
            .map(|(_, view)| view.clone())
    }

    /// One match by identifier.
    pub fn get(&self, match_id: &str) -> Option<ArenaMatchView> {
        self.matches
            .lock()
            .expect("arena surface lock")
            .iter()
            .find(|(id, _)| id == match_id)
            .map(|(_, view)| view.clone())
    }

    /// How many matches are retained.
    pub fn len(&self) -> usize {
        self.matches.lock().expect("arena surface lock").len()
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

/// Returns every agent that can legally be seated in `game`.
///
/// The filter is the registry's own `supported_games`, decided in the backend. A frontend cannot offer
/// a pairing the arena would refuse, and adding a game later cannot surface a policy that has no idea
/// what its rules are.
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
#[tauri::command(rename_all = "snake_case")]
pub fn latest_arena_match(surface: State<'_, ArenaSurface>) -> Option<ArenaMatchView> {
    surface.latest()
}

/// Reconstructs a stored match as a deterministic replay.
///
/// The reconstruction reads recorded data and nothing else: no engine, no provider, no broker. It
/// therefore cannot produce a different result than the recording, and cannot call a model even by
/// accident — which is what makes it a replay rather than a second run.
#[tauri::command(rename_all = "snake_case")]
pub fn replay_arena_match(
    surface: State<'_, ArenaSurface>,
    match_id: String,
) -> Result<probelab_lab::Replay, ArenaCommandError> {
    let view = surface.get(&match_id).ok_or_else(|| ArenaCommandError {
        code: "not_found".to_owned(),
        message: format!("no match named {match_id:?}"),
    })?;
    Ok(probelab_lab::replay_match(&view))
}

/// One stored match by identifier.
#[tauri::command(rename_all = "snake_case")]
pub fn get_arena_match(
    surface: State<'_, ArenaSurface>,
    match_id: String,
) -> Result<ArenaMatchView, ArenaCommandError> {
    surface.get(&match_id).ok_or_else(|| ArenaCommandError {
        code: "not_found".to_owned(),
        message: format!("no match named {match_id:?}"),
    })
}

/// Plays one match and returns what the engine produced.
///
/// The whole match runs here and comes back projected: there is no partial state to get out of step
/// with the environment, because the environment is the only thing that ever had state.
#[tauri::command(rename_all = "snake_case")]
pub fn play_arena_match(
    state: State<'_, AppState>,
    request: ArenaMatchRequest,
) -> Result<ArenaMatchView, ArenaCommandError> {
    let game = parse_game(&request.game)?;
    let side = parse_side(&request.tested_side)?;
    let match_id = new_match_id(request.seed);
    let recorder: Arc<dyn TelemetryRecorder> = state.recorder.clone();

    let played = ArenaRunner::new()
        .play(
            &probelab_lab::ArenaMatchSpec::new(
                game,
                &request.tested_agent,
                &request.opponent_agent,
                side,
                request.seed,
                &match_id,
            ),
            Some(recorder),
        )
        .map_err(ArenaCommandError::from)?;

    state.arena.store(match_id, played.view.clone());
    Ok(played.view)
}

/// Parses a game identifier, refusing anything this build cannot play.
fn parse_game(game: &str) -> Result<GameKind, ArenaCommandError> {
    match game {
        "tic_tac_toe" => Ok(GameKind::TicTacToe),
        other => Err(ArenaCommandError {
            code: "unknown_game".to_owned(),
            message: format!("this build cannot play {other:?}"),
        }),
    }
}

/// Parses a side.
fn parse_side(side: &str) -> Result<GameSide, ArenaCommandError> {
    match side {
        "first" => Ok(GameSide::First),
        "second" => Ok(GameSide::Second),
        other => Err(ArenaCommandError {
            code: "invalid_side".to_owned(),
            message: format!("{other:?} is not a side; use \"first\" or \"second\""),
        }),
    }
}

/// A match identifier derived from the seed, so the same seed names the same match.
fn new_match_id(seed: u64) -> String {
    format!("arena-ttt-{seed}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_tic_tac_toe_is_exposed() {
        let games = list_arena_games();
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].id, "tic_tac_toe");
        assert_eq!(games[0].width, 3);
    }

    #[test]
    fn an_unimplemented_game_is_refused_by_name() {
        for game in ["connect_four", "rps", "battleship", "not-a-game"] {
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
        let first = sample_view("arena-ttt-1");
        let second = sample_view("arena-ttt-2");
        surface.store("arena-ttt-1".to_owned(), first);
        surface.store("arena-ttt-2".to_owned(), second);
        assert_eq!(surface.latest().expect("latest").match_id, "arena-ttt-2");
        assert_eq!(
            surface.get("arena-ttt-1").expect("first").match_id,
            "arena-ttt-1"
        );
    }

    #[test]
    fn the_match_history_is_bounded() {
        let surface = ArenaSurface::default();
        for index in 0..25 {
            surface.store(format!("m-{index}"), sample_view(&format!("m-{index}")));
        }
        assert!(
            surface.len() <= 20,
            "history grew unbounded: {}",
            surface.len()
        );
    }

    #[test]
    fn a_match_id_is_derived_from_the_seed_so_a_seed_names_its_match() {
        assert_eq!(new_match_id(7), "arena-ttt-7");
        assert_eq!(new_match_id(7), new_match_id(7));
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
            tested_side: GameSide::First,
            seed: 1,
            plies: 9,
            final_board: probelab_core::GameBoard::TicTacToe {
                cells: vec![probelab_core::TicTacToeCell::Empty; 9],
            },
            winner: None,
            illegal_moves: Vec::new(),
            history: Vec::new(),
            timeline: Vec::new(),
            failure: None,
        }
    }

    const SURFACE_SOURCE: &str = include_str!("arena.rs");

    fn command_surface_source() -> &'static str {
        SURFACE_SOURCE
            .split("#[cfg(test)]")
            .next()
            .expect("the file has content before its test module")
    }
}
