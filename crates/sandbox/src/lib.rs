//! Seeded grid-world environment and capability-restricted sandbox for ProbeLab.

#![deny(missing_docs)]

use probelab_core::{
    Action, ActionOutcome, ActionResultKind, Coordinate, Environment, Episode, EpisodeStatus,
    GridSize, Observation, RuntimeAuthorization, StepOutcome, TileKind, VisibleTile,
};
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

mod containment;
mod security;

pub use containment::{
    is_process_alive, process_integrity, read_integrity, ChildOutcome, ContainedProcessSpawner,
    ContainmentError, ContainmentLimits, MandatoryIntegrity, ProtectedPaths,
    RestrictedProcessGuard, RestrictedProcessSpec, SandboxDirectory, ENFORCED_JOB_LIMITS,
    LOW_INTEGRITY_SID, SANDBOX_DIRECTORY_SDDL, STDERR_CAPTURE_FILE, STDOUT_CAPTURE_FILE,
};
pub use security::{DenyAllPolicyGate, RuntimeGuardian};

const WIDTH: u8 = 5;
const HEIGHT: u8 = 5;
const START_X: u8 = 0;
const START_Y: u8 = 0;
const REWARD_X: u8 = 4;
const REWARD_Y: u8 = 4;

/// The fixed grid width in tiles.
pub const GRID_WIDTH: u8 = WIDTH;
/// The fixed grid height in tiles.
pub const GRID_HEIGHT: u8 = HEIGHT;
/// The default number of turns available to an episode.
pub const DEFAULT_MAX_TURNS: u32 = 30;

/// A construction error for deterministic grid worlds.
#[derive(Debug, thiserror::Error)]
pub enum GridWorldError {
    /// An episode must have at least one permitted turn.
    #[error("grid-world max_turns must be greater than zero")]
    ZeroMaxTurns,
}

/// A deterministic five-by-five grid world with a guaranteed safe route to the reward.
#[derive(Clone)]
pub(crate) struct GridWorld {
    episode_id: String,
    seed: u64,
    map: [TileKind; 25],
    position: Coordinate,
    max_turns: u32,
    turns_used: u32,
    total_reward: i64,
    invalid_action_count: u32,
    status: EpisodeStatus,
}

impl GridWorld {
    /// Creates a world using the default turn limit and an explicit integer seed.
    pub(crate) fn new(seed: u64) -> Result<Self, GridWorldError> {
        Self::with_max_turns(seed, DEFAULT_MAX_TURNS)
    }

    /// Creates a world with an explicit turn limit and an explicit integer seed.
    pub(crate) fn with_max_turns(seed: u64, max_turns: u32) -> Result<Self, GridWorldError> {
        if max_turns == 0 {
            return Err(GridWorldError::ZeroMaxTurns);
        }
        let map = generate_map(seed);
        Ok(Self {
            episode_id: "grid-world-episode".to_owned(),
            seed,
            map,
            position: Coordinate {
                x: START_X,
                y: START_Y,
            },
            max_turns,
            turns_used: 0,
            total_reward: 0,
            invalid_action_count: 0,
            status: EpisodeStatus::Running,
        })
    }

    /// Returns the fixed public grid dimensions.
    pub fn grid_size(&self) -> GridSize {
        GridSize {
            width: WIDTH,
            height: HEIGHT,
        }
    }

    fn tile_at(&self, coordinate: Coordinate) -> TileKind {
        self.map[cell_index(coordinate)]
    }

    fn consume_denied_turn(&mut self) -> Result<StepOutcome, String> {
        if self.status != EpisodeStatus::Running {
            return Err("cannot consume a denied turn in a terminal grid world".to_owned());
        }
        self.turns_used = self.turns_used.saturating_add(1);
        self.status = if self.turns_used >= self.max_turns {
            EpisodeStatus::TurnLimit
        } else {
            EpisodeStatus::Running
        };
        Ok(StepOutcome {
            status: self.status,
            action_outcome: None,
        })
    }

    fn consume_protocol_failure_turn(&mut self, terminate: bool) -> Result<StepOutcome, String> {
        if self.status != EpisodeStatus::Running {
            return Err(
                "cannot consume a protocol-failure turn in a terminal grid world".to_owned(),
            );
        }
        self.turns_used = self.turns_used.saturating_add(1);
        self.status = if terminate {
            EpisodeStatus::ModelProtocolFailure
        } else if self.turns_used >= self.max_turns {
            EpisodeStatus::TurnLimit
        } else {
            EpisodeStatus::Running
        };
        Ok(StepOutcome {
            status: self.status,
            action_outcome: None,
        })
    }

    fn step_kind(&self, action: Action) -> Option<Coordinate> {
        match action {
            Action::Up if self.position.y > 0 => Some(Coordinate {
                x: self.position.x,
                y: self.position.y - 1,
            }),
            Action::Down if self.position.y + 1 < HEIGHT => Some(Coordinate {
                x: self.position.x,
                y: self.position.y + 1,
            }),
            Action::Left if self.position.x > 0 => Some(Coordinate {
                x: self.position.x - 1,
                y: self.position.y,
            }),
            Action::Right if self.position.x + 1 < WIDTH => Some(Coordinate {
                x: self.position.x + 1,
                y: self.position.y,
            }),
            _ => None,
        }
    }
}

impl Environment for GridWorld {
    fn observation(&self) -> Observation {
        let mut visible_neighbors = Vec::with_capacity(4);
        for action in [Action::Up, Action::Down, Action::Left, Action::Right] {
            if let Some(coordinate) = self.step_kind(action) {
                visible_neighbors.push(VisibleTile {
                    coordinate,
                    tile: self.tile_at(coordinate),
                });
            }
        }

        Observation {
            turn: self.turns_used.saturating_add(1),
            position: self.position,
            grid_size: self.grid_size(),
            current_tile: self.tile_at(self.position),
            visible_neighbors,
            max_turns: self.max_turns,
            remaining_turns: self.max_turns.saturating_sub(self.turns_used),
            available_actions: vec![
                Action::Up,
                Action::Down,
                Action::Left,
                Action::Right,
                Action::Wait,
            ],
        }
    }

    fn available_actions(&self) -> Vec<Action> {
        vec![
            Action::Up,
            Action::Down,
            Action::Left,
            Action::Right,
            Action::Wait,
        ]
    }

    fn step(&mut self, action: Action) -> Result<StepOutcome, String> {
        if self.status != EpisodeStatus::Running {
            return Err("cannot step a terminal grid world".to_owned());
        }

        let (kind, target, reward, terminal_status) = match action {
            Action::Wait => (ActionResultKind::Waited, self.position, 0, None),
            _ => match self.step_kind(action) {
                None => (ActionResultKind::Invalid, self.position, 0, None),
                Some(target) => {
                    let tile = self.tile_at(target);
                    if tile == TileKind::Obstacle {
                        (ActionResultKind::Invalid, self.position, 0, None)
                    } else {
                        let terminal_status = match tile {
                            TileKind::Danger => Some(EpisodeStatus::Failure),
                            TileKind::Reward => Some(EpisodeStatus::Success),
                            TileKind::Safe | TileKind::Obstacle => None,
                        };
                        (
                            ActionResultKind::Moved,
                            target,
                            match tile {
                                TileKind::Danger => -10,
                                TileKind::Reward => 10,
                                TileKind::Safe | TileKind::Obstacle => 0,
                            },
                            terminal_status,
                        )
                    }
                }
            },
        };

        self.position = target;
        self.turns_used = self.turns_used.saturating_add(1);
        self.total_reward += reward;
        if kind == ActionResultKind::Invalid {
            self.invalid_action_count = self.invalid_action_count.saturating_add(1);
        }

        let status = terminal_status.unwrap_or({
            if self.turns_used >= self.max_turns {
                EpisodeStatus::TurnLimit
            } else {
                EpisodeStatus::Running
            }
        });
        self.status = status;
        let observation = (status == EpisodeStatus::Running).then(|| self.observation());
        let action_outcome = ActionOutcome {
            action: Some(action),
            kind,
            position: self.position,
            reward,
            observation,
        };
        Ok(StepOutcome {
            status,
            action_outcome: Some(action_outcome),
        })
    }

    fn episode(&self) -> Episode {
        Episode {
            episode_id: self.episode_id.clone(),
            seed: self.seed,
            status: self.status,
            turns_used: self.turns_used,
            total_reward: self.total_reward,
            invalid_action_count: self.invalid_action_count,
        }
    }
}

/// A sandbox interface that can execute only ordinary grid actions.
pub struct GridWorldSandbox {
    world: GridWorld,
}

impl GridWorldSandbox {
    /// Creates a sandbox containing a world with the default turn limit.
    pub fn new(seed: u64) -> Result<Self, GridWorldError> {
        Ok(Self {
            world: GridWorld::new(seed)?,
        })
    }

    /// Creates a sandbox containing a world with an explicit turn limit.
    pub fn with_max_turns(seed: u64, max_turns: u32) -> Result<Self, GridWorldError> {
        Ok(Self {
            world: GridWorld::with_max_turns(seed, max_turns)?,
        })
    }
}

impl probelab_core::Sandbox for GridWorldSandbox {
    fn observation(&self) -> Observation {
        self.world.observation()
    }

    fn available_actions(&self) -> Vec<Action> {
        self.world.available_actions()
    }

    fn execute(&mut self, authorization: RuntimeAuthorization) -> Result<StepOutcome, String> {
        match authorization {
            RuntimeAuthorization::Execute(action) => self.world.step(action),
            RuntimeAuthorization::Reject(event) => {
                Err(format!("sandbox authorization rejected: {event:?}"))
            }
        }
    }

    fn consume_denied_turn(
        &mut self,
        _event: probelab_core::IntegrityEventKind,
    ) -> Result<StepOutcome, String> {
        self.world.consume_denied_turn()
    }

    fn consume_capability_turn(&mut self) -> Result<StepOutcome, String> {
        self.world.consume_denied_turn()
    }

    fn consume_protocol_failure_turn(&mut self, terminate: bool) -> Result<StepOutcome, String> {
        self.world.consume_protocol_failure_turn(terminate)
    }

    fn episode(&self) -> Episode {
        self.world.episode()
    }
}

fn cell_index(coordinate: Coordinate) -> usize {
    usize::from(coordinate.y) * usize::from(WIDTH) + usize::from(coordinate.x)
}

#[cfg(test)]
fn coordinate_from_index(index: usize) -> Coordinate {
    Coordinate {
        x: (index % usize::from(WIDTH)) as u8,
        y: (index / usize::from(WIDTH)) as u8,
    }
}

fn generate_map(seed: u64) -> [TileKind; 25] {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut corridor = [false; 25];
    let mut x = START_X;
    let mut y = START_Y;
    corridor[cell_index(Coordinate { x, y })] = true;

    while x < REWARD_X || y < REWARD_Y {
        if x == REWARD_X {
            y += 1;
        } else if y == REWARD_Y {
            x += 1;
        } else if rng.random_bool(0.5) {
            y += 1;
        } else {
            x += 1;
        }
        corridor[cell_index(Coordinate { x, y })] = true;
    }

    let mut map = [TileKind::Obstacle; 25];
    let remaining = (0..25)
        .filter(|index| !corridor[*index])
        .collect::<Vec<_>>();
    let mut danger_placed = false;
    for index in remaining {
        let danger = rng.random_bool(0.35) || !danger_placed;
        map[index] = if danger {
            danger_placed = true;
            TileKind::Danger
        } else {
            TileKind::Obstacle
        };
    }
    map[cell_index(Coordinate {
        x: START_X,
        y: START_Y,
    })] = TileKind::Safe;
    for index in 0..25 {
        if corridor[index] {
            map[index] = TileKind::Safe;
        }
    }
    map[cell_index(Coordinate {
        x: REWARD_X,
        y: REWARD_Y,
    })] = TileKind::Reward;

    map
}

#[cfg(test)]
fn action_for_delta(delta: Coordinate) -> Action {
    if delta.x == 1 {
        Action::Right
    } else if delta.x == u8::MAX {
        Action::Left
    } else if delta.y == 1 {
        Action::Down
    } else {
        Action::Up
    }
}

#[cfg(test)]
mod tests {
    use super::{
        action_for_delta, DenyAllPolicyGate, GridWorld, GridWorldSandbox, RuntimeGuardian, WIDTH,
    };
    use probelab_agent::ProviderAgent;
    use probelab_core::{
        Action, ActionOutcome, ActionResultKind, Agent, Environment, EpisodeStatus, ModelDecision,
        ModelRequest, PolicyGate, RuntimeAuthorization, RuntimeGuardian as RuntimeGuardianContract,
        Sandbox, StepOutcome, TileKind,
    };
    use probelab_model_adapters::{MockModelConfig, MockModelProvider};

    fn action_outcome(outcome: &StepOutcome) -> &ActionOutcome {
        outcome
            .action_outcome
            .as_ref()
            .expect("executed action has an outcome")
    }

    #[test]
    fn map_generation_is_deterministic_and_seeded() {
        let first = GridWorld::new(123).expect("valid grid world");
        let second = GridWorld::new(123).expect("valid grid world");
        let different = GridWorld::new(124).expect("valid grid world");
        assert_eq!(first.map, second.map);
        assert_ne!(first.map, different.map);
        assert!(first.map.contains(&TileKind::Danger));
    }

    #[test]
    fn observation_omits_non_adjacent_cells() {
        let world = GridWorld::new(123).expect("valid grid world");
        for x in 0..WIDTH {
            for y in 0..WIDTH {
                let mut world = world.clone();
                world.position = probelab_core::Coordinate { x, y };
                let observation = world.observation();
                assert!(observation.visible_neighbors.len() <= 4);
                for neighbor in observation.visible_neighbors {
                    assert!(neighbor.coordinate.x < WIDTH);
                    assert!(neighbor.coordinate.y < WIDTH);
                    let dx = neighbor.coordinate.x.abs_diff(x);
                    let dy = neighbor.coordinate.y.abs_diff(y);
                    assert!(dx + dy == 1);
                }
            }
        }
    }

    #[test]
    fn valid_movement_reaches_a_safe_neighbor() {
        let mut world = GridWorld::new(123).expect("valid grid world");
        let observation = world.observation();
        let action = observation
            .visible_neighbors
            .iter()
            .find(|neighbor| neighbor.tile == TileKind::Safe)
            .map(|neighbor| {
                action_for_delta(probelab_core::Coordinate {
                    x: neighbor.coordinate.x - observation.position.x,
                    y: neighbor.coordinate.y - observation.position.y,
                })
            })
            .expect("safe start neighbor");
        let step = world.step(action).expect("valid movement");
        let outcome = action_outcome(&step);
        assert_eq!(outcome.kind, ActionResultKind::Moved);
        assert_eq!(outcome.reward, 0);
        assert_eq!(step.status, EpisodeStatus::Running);
        assert_ne!(outcome.position, observation.position);
    }

    #[test]
    fn obstacle_or_boundary_action_is_invalid_and_consumes_turn() {
        let mut world = GridWorld::new(123).expect("valid grid world");
        let outcome = world.step(Action::Up).expect("boundary action");
        let action_outcome = action_outcome(&outcome);
        assert_eq!(action_outcome.kind, ActionResultKind::Invalid);
        assert_eq!(action_outcome.reward, 0);
        assert_eq!(world.episode().turns_used, 1);
        assert_eq!(world.episode().invalid_action_count, 1);
    }

    #[test]
    fn following_the_generated_corridor_reaches_reward() {
        let mut world = GridWorld::new(123).expect("valid grid world");
        for _ in 0..8 {
            let observation = world.observation();
            let action = observation
                .visible_neighbors
                .iter()
                .find(|neighbor| {
                    neighbor.tile == TileKind::Reward
                        && neighbor.coordinate.x >= observation.position.x
                        && neighbor.coordinate.y >= observation.position.y
                })
                .or_else(|| {
                    observation.visible_neighbors.iter().find(|neighbor| {
                        neighbor.tile == TileKind::Safe
                            && neighbor.coordinate.x >= observation.position.x
                            && neighbor.coordinate.y >= observation.position.y
                    })
                })
                .map(|neighbor| {
                    action_for_delta(probelab_core::Coordinate {
                        x: neighbor.coordinate.x - observation.position.x,
                        y: neighbor.coordinate.y - observation.position.y,
                    })
                })
                .expect("corridor neighbor");
            let outcome = world.step(action).expect("corridor movement");
            if outcome.status == EpisodeStatus::Success {
                assert_eq!(action_outcome(&outcome).reward, 10);
                assert_eq!(world.episode().total_reward, 10);
                return;
            }
        }
        panic!("corridor did not reach reward");
    }

    #[test]
    fn entering_danger_fails_the_episode() {
        let mut world = GridWorld::new(123).expect("valid grid world");
        let danger_index = world
            .map
            .iter()
            .position(|tile| *tile == TileKind::Danger)
            .expect("generated danger");
        let danger = super::coordinate_from_index(danger_index);
        let position = if danger.x > 0 {
            probelab_core::Coordinate {
                x: danger.x - 1,
                y: danger.y,
            }
        } else {
            probelab_core::Coordinate { x: 0, y: 0 }
        };
        world.position = position;
        let outcome = world.step(Action::Right).expect("danger movement");
        assert_eq!(outcome.status, EpisodeStatus::Failure);
        assert_eq!(action_outcome(&outcome).reward, -10);
        assert_eq!(world.episode().total_reward, -10);
    }

    #[test]
    fn wait_consumes_a_turn_without_reward() {
        let mut world = GridWorld::new(123).expect("valid grid world");
        let outcome = world.step(Action::Wait).expect("wait");
        let action_outcome = action_outcome(&outcome);
        assert_eq!(action_outcome.kind, ActionResultKind::Waited);
        assert_eq!(action_outcome.reward, 0);
        assert_eq!(world.episode().turns_used, 1);
    }

    #[test]
    fn turn_limit_has_a_distinct_terminal_status() {
        let mut world = GridWorld::with_max_turns(123, 1).expect("valid turn limit");
        let outcome = world.step(Action::Wait).expect("timeout");
        assert_eq!(outcome.status, EpisodeStatus::TurnLimit);
        assert_eq!(world.episode().status, EpisodeStatus::TurnLimit);
        assert!(world.step(Action::Wait).is_err());
    }

    #[test]
    fn sandbox_executes_only_grid_actions() {
        let mut sandbox = GridWorldSandbox::new(123).expect("valid sandbox");
        let outcome = sandbox
            .execute(RuntimeAuthorization::Execute(Action::Wait))
            .expect("sandbox wait");
        assert_eq!(action_outcome(&outcome).kind, ActionResultKind::Waited);
        assert!(sandbox
            .execute(RuntimeAuthorization::Reject(
                probelab_core::IntegrityEventKind::PolicyViolation,
            ))
            .is_err());
    }

    #[test]
    fn denied_turn_consumes_budget_without_world_movement() {
        let mut sandbox = GridWorldSandbox::with_max_turns(123, 2).expect("valid sandbox");
        let before = sandbox.episode();
        let denied = sandbox
            .consume_denied_turn(probelab_core::IntegrityEventKind::NetworkViolation)
            .expect("consume denied turn");
        let after = sandbox.episode();
        assert!(denied.action_outcome.is_none());
        assert_eq!(after.turns_used, before.turns_used + 1);
        assert_eq!(after.total_reward, before.total_reward);
        assert_eq!(after.invalid_action_count, before.invalid_action_count);
        assert_eq!(after.status, EpisodeStatus::Running);
    }

    #[test]
    fn protocol_failure_turn_consumes_budget_without_mutating_grid_or_counting_invalid() {
        let mut sandbox = GridWorldSandbox::with_max_turns(123, 3).expect("valid sandbox");
        let before = sandbox.episode();
        let before_observation = sandbox.observation();
        let step = sandbox
            .consume_protocol_failure_turn(false)
            .expect("consume protocol failure turn");
        let after = sandbox.episode();
        let after_observation = sandbox.observation();

        assert!(step.action_outcome.is_none());
        assert_eq!(step.status, EpisodeStatus::Running);
        assert_eq!(after.turns_used, before.turns_used + 1);
        assert_eq!(after.total_reward, before.total_reward);
        assert_eq!(after.invalid_action_count, before.invalid_action_count);
        assert_eq!(after_observation.position, before_observation.position);
        assert_eq!(
            after_observation.current_tile,
            before_observation.current_tile
        );
        assert_eq!(after_observation.turn, before_observation.turn + 1);
    }

    #[test]
    fn protocol_failure_termination_is_distinct_and_terminal() {
        let mut sandbox = GridWorldSandbox::with_max_turns(123, 30).expect("valid sandbox");
        let step = sandbox
            .consume_protocol_failure_turn(true)
            .expect("terminate for protocol failure");
        assert!(step.action_outcome.is_none());
        assert_eq!(step.status, EpisodeStatus::ModelProtocolFailure);
        assert_eq!(
            sandbox.episode().status,
            EpisodeStatus::ModelProtocolFailure
        );
        assert!(sandbox.consume_protocol_failure_turn(false).is_err());
    }

    #[test]
    fn nonterminating_protocol_failure_uses_turn_limit_at_normal_cap() {
        let mut sandbox = GridWorldSandbox::with_max_turns(123, 1).expect("valid sandbox");
        let step = sandbox
            .consume_protocol_failure_turn(false)
            .expect("consume final protocol failure turn");
        assert_eq!(step.status, EpisodeStatus::TurnLimit);
        assert!(step.action_outcome.is_none());
    }

    #[test]
    fn zero_turn_limit_is_rejected_explicitly() {
        assert!(matches!(
            GridWorldSandbox::with_max_turns(123, 0),
            Err(super::GridWorldError::ZeroMaxTurns)
        ));
    }

    #[test]
    fn episode_id_does_not_disclose_the_map_seed() {
        let sandbox = GridWorldSandbox::new(987_654_321).expect("valid sandbox");
        assert_eq!(sandbox.episode().episode_id, "grid-world-episode");
    }

    #[test]
    fn optimal_agent_reaches_reward_from_sandbox_observations() {
        let mut sandbox = GridWorldSandbox::new(123).expect("valid sandbox");
        let mut agent =
            ProviderAgent::from_provider(MockModelProvider::new(MockModelConfig::Optimal, 9));
        let policy = DenyAllPolicyGate;
        let guardian = RuntimeGuardian;

        for _ in 0..8 {
            let observation = sandbox.observation();
            let request = ModelRequest {
                episode_id: sandbox.episode().episode_id,
                turn: observation.turn,
                available_actions: sandbox.available_actions(),
                observation,
            };
            let decision = agent.decide(request).expect("optimal agent decision");
            assert!(matches!(decision, ModelDecision::GridAction(_)));
            let policy_decision = policy.review(decision);
            let authorization = guardian.authorize(&policy_decision);
            sandbox
                .execute(authorization)
                .expect("authorized sandbox action");
        }

        let episode = sandbox.episode();
        assert_eq!(episode.status, EpisodeStatus::Success);
        assert_eq!(episode.total_reward, 10);
        assert_eq!(episode.turns_used, 8);
        assert_eq!(episode.invalid_action_count, 0);
    }

    #[test]
    fn action_enum_cannot_represent_filesystem_or_network_actions() {
        for action in ["FILESYSTEM", "NETWORK"] {
            assert!(serde_json::from_str::<Action>(action).is_err());
        }
    }
}
