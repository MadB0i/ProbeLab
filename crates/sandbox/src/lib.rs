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

/// Derives the complete layout for a seed, without running an episode.
///
/// This is the generator's public face. The benchmark uses it to size a seed's turn budget before
/// the episode starts, and the harness and tests use it to reason about a seed offline. It is
/// deliberately not reachable by a model: a model is told what it can see, never what the layout is.
pub fn grid_world_layout(seed: u64) -> GridWorldLayout {
    generate_layout(seed)
}

const WIDTH: u8 = 5;
const HEIGHT: u8 = 5;
const START_X: u8 = 0;
const START_Y: u8 = 0;

/// The fixed grid width in tiles.
pub const GRID_WIDTH: u8 = WIDTH;
/// The fixed grid height in tiles.
pub const GRID_HEIGHT: u8 = HEIGHT;

/// The catalog ceiling on episode length, in turns.
///
/// This is **not** the turn budget of any particular seed. A seed's real budget is derived from its
/// own shortest safe route and is usually far smaller; this value is the upper bound the catalog
/// advertises, so a summary can describe the benchmark without knowing which seed a run will use.
pub const DEFAULT_MAX_TURNS: u32 = 12;

/// The Manhattan diameter of the grid: the longest possible shortest route between two tiles.
const GRID_DIAMETER: u32 = (WIDTH - 1) as u32 + (HEIGHT - 1) as u32;

/// Numerator of the per-seed turn budget ratio.
pub const TURN_BUDGET_NUMERATOR: u32 = 3;
/// Denominator of the per-seed turn budget ratio.
pub const TURN_BUDGET_DENOMINATOR: u32 = 2;

/// A generation failure for deterministic grid worlds.
#[derive(Debug, thiserror::Error)]
pub enum GridWorldError {
    /// An episode must have at least one permitted turn.
    #[error("grid-world max_turns must be greater than zero")]
    ZeroMaxTurns,
}

/// A fully resolved, seed-specific grid world layout.
///
/// The layout is computed once from the seed and then never changes, so an episode's terrain cannot
/// drift under it. Everything the benchmark needs to reason about a seed — where the goal is, how
/// long the shortest safe route is, and therefore how many turns the episode gets — is derived here
/// and exposed for the harness and the tests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GridWorldLayout {
    tiles: [TileKind; 25],
    start: Coordinate,
    goal: Coordinate,
    shortest_safe_path: u32,
    max_turns: u32,
}

impl GridWorldLayout {
    /// Returns the tile at a coordinate.
    pub fn tile(&self, coordinate: Coordinate) -> TileKind {
        self.tiles[cell_index(coordinate)]
    }

    /// Returns every tile, row-major.
    pub fn tiles(&self) -> &[TileKind; 25] {
        &self.tiles
    }

    /// Returns the per-seed start coordinate.
    pub fn start(&self) -> Coordinate {
        self.start
    }

    /// Returns the per-seed goal coordinate.
    pub fn goal(&self) -> Coordinate {
        self.goal
    }

    /// Returns the number of moves on the shortest hazard-free route from start to goal.
    pub fn shortest_safe_path(&self) -> u32 {
        self.shortest_safe_path
    }

    /// Manhattan distance from a coordinate to this seed's goal.
    pub fn manhattan_to_goal(&self, coordinate: Coordinate) -> u32 {
        manhattan(coordinate, self.goal)
    }

    /// Returns the turn budget this seed's episode runs with.
    pub fn max_turns(&self) -> u32 {
        self.max_turns
    }

    /// Returns every hazard tile, row-major. For tests and the harness, never for a model.
    pub fn hazards(&self) -> Vec<Coordinate> {
        (0..25)
            .map(coordinate_from_index)
            .filter(|coordinate| self.tile(*coordinate) == TileKind::Danger)
            .collect()
    }

    /// Returns every traversable tile as coordinates, row-major.
    pub fn traversable_tiles(&self) -> Vec<Coordinate> {
        (0..25)
            .map(coordinate_from_index)
            .filter(|coordinate| {
                matches!(self.tile(*coordinate), TileKind::Safe | TileKind::Reward)
            })
            .collect()
    }
}

/// The fixed start coordinate, identical for every seed.
pub const START: Coordinate = Coordinate {
    x: START_X,
    y: START_Y,
};

/// A deterministic five-by-five grid world with a per-seed goal, a branching safe region, and a
/// seed-specific turn budget.
#[derive(Clone)]
pub(crate) struct GridWorld {
    episode_id: String,
    seed: u64,
    layout: GridWorldLayout,
    position: Coordinate,
    turns_used: u32,
    total_reward: i64,
    invalid_action_count: u32,
    status: EpisodeStatus,
}

impl GridWorld {
    /// Creates a world using the seed's own derived turn budget.
    pub(crate) fn new(seed: u64) -> Result<Self, GridWorldError> {
        let layout = generate_layout(seed);
        Ok(Self::with_layout(seed, layout))
    }

    /// Creates a world with an explicit turn limit and an explicit integer seed.
    ///
    /// The explicit limit overrides the seed's derived budget. A caller that has computed a budget
    /// from the seed's own shortest route passes it here, which is how the benchmark keeps the
    /// published ceiling and the per-episode budget the same number.
    pub(crate) fn with_max_turns(seed: u64, max_turns: u32) -> Result<Self, GridWorldError> {
        if max_turns == 0 {
            return Err(GridWorldError::ZeroMaxTurns);
        }
        let mut layout = generate_layout(seed);
        layout.max_turns = max_turns;
        Ok(Self::with_layout(seed, layout))
    }

    fn with_layout(seed: u64, layout: GridWorldLayout) -> Self {
        let position = layout.start();
        Self {
            episode_id: "grid-world-episode".to_owned(),
            seed,
            layout,
            position,
            turns_used: 0,
            total_reward: 0,
            invalid_action_count: 0,
            status: EpisodeStatus::Running,
        }
    }

    /// Returns the fixed public grid dimensions.
    pub fn grid_size(&self) -> GridSize {
        GridSize {
            width: WIDTH,
            height: HEIGHT,
        }
    }

    fn tile_at(&self, coordinate: Coordinate) -> TileKind {
        self.layout.tile(coordinate)
    }

    /// Manhattan distance from a coordinate to this seed's goal.
    fn distance_to_goal(&self, coordinate: Coordinate) -> i64 {
        i64::from(coordinate.x.abs_diff(self.layout.goal.x))
            + i64::from(coordinate.y.abs_diff(self.layout.goal.y))
    }

    fn consume_denied_turn(&mut self) -> Result<StepOutcome, String> {
        if self.status != EpisodeStatus::Running {
            return Err("cannot consume a denied turn in a terminal grid world".to_owned());
        }
        self.turns_used = self.turns_used.saturating_add(1);
        self.status = if self.turns_used >= self.layout.max_turns {
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
        } else if self.turns_used >= self.layout.max_turns {
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
            distance_to_goal: u32::try_from(self.distance_to_goal(self.position))
                .unwrap_or(u32::MAX),
            max_turns: self.layout.max_turns,
            remaining_turns: self.layout.max_turns.saturating_sub(self.turns_used),
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

        let origin = self.position;
        let (kind, target, terminal_bonus, terminal_status) = match action {
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
                        let terminal_bonus = match tile {
                            TileKind::Danger => -HAZARD_PENALTY,
                            TileKind::Reward => GOAL_BONUS,
                            TileKind::Safe | TileKind::Obstacle => 0,
                        };
                        (
                            ActionResultKind::Moved,
                            target,
                            terminal_bonus,
                            terminal_status,
                        )
                    }
                }
            },
        };

        // Shaped credit for progress. Moving one tile closer to this seed's goal is worth +1 and one
        // tile further is worth -1, whether or not the episode ends this turn. A wait or a refused
        // move produces no progress and no credit. This is what makes a partial run informative: two
        // runs that both end in `turn_limit` can now be told apart by how far they actually got,
        // which the old terminal-only score could not express at all.
        let progress = self.distance_to_goal(origin) - self.distance_to_goal(target);
        let reward = progress + terminal_bonus;

        self.position = target;
        self.turns_used = self.turns_used.saturating_add(1);
        self.total_reward += reward;
        if kind == ActionResultKind::Invalid {
            self.invalid_action_count = self.invalid_action_count.saturating_add(1);
        }

        let status = terminal_status.unwrap_or({
            if self.turns_used >= self.layout.max_turns {
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

    /// Moves the agent without going through the policy gate.
    ///
    /// Test support only. Every production path reaches the world through
    /// [`probelab_core::Sandbox::execute`], which is what applies authorization. This exists so a test
    /// can place the agent somewhere specific and assert what it observes from there.
    pub fn teleport_for_test(&mut self, position: Coordinate) {
        self.world.position = position;
    }

    /// Steps the world directly, bypassing the sandbox's authorization layer.
    ///
    /// Test support only, for the same reason as [`GridWorldSandbox::teleport_for_test`].
    pub fn step_for_test(&mut self, action: Action) -> Result<StepOutcome, String> {
        self.world.step(action)
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

/// Row-major index to coordinate.
fn coordinate_from_index(index: usize) -> Coordinate {
    Coordinate {
        x: (index % usize::from(WIDTH)) as u8,
        y: (index / usize::from(WIDTH)) as u8,
    }
}

/// Reward added on top of any progress credit for reaching the goal.
const GOAL_BONUS: i64 = 10;
/// Penalty added on top of any progress credit for stepping into a hazard.
const HAZARD_PENALTY: i64 = 10;

/// Derives the complete, seed-specific layout.
///
/// The generator is built so that a one-step-lookahead policy with no memory cannot reliably solve
/// it, while a search over the revealed map can. Three properties do that work together:
///
/// 1. **The goal moves.** It is drawn per seed, so "walk to the far corner" is not a winning
///    strategy and a model cannot hardcode a coordinate.
/// 2. **The safe region branches.** A monotone spine guarantees a route, and seeded spurs hang off
///    it, so the safe tiles form a tree with real forks rather than the single corridor the old
///    generator produced. At a fork the two branches are locally indistinguishable; only one of them
///    leads anywhere.
/// 3. **The turn budget is tight.** It is `ceil(1.5 x shortest safe route)` for *this* seed, so
///    probing a dead end is not free. Under the old fixed 30-turn budget an agent could waste 22
///    turns wandering and still win, which is why the old benchmark could not tell a search policy
///    from a coin flip.
///
/// Hazards are only ever placed where they cannot make the goal unreachable, and never adjacent to
/// the start, so every hazard is genuinely hidden until the agent is one step from it. That is what
/// makes remembering a hazard matter: by the time the agent needs to avoid it, it may be two cells
/// away and out of sight.
fn generate_layout(seed: u64) -> GridWorldLayout {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let (start, goal) = choose_endpoints(&mut rng);
    let spine = monotone_spine(&mut rng, start, goal);
    let safe = build_safe_region(&mut rng, &spine, start, goal);
    let mut tiles = [TileKind::Obstacle; 25];
    for index in 0..25 {
        if safe[cell_index(coordinate_from_index(index))] {
            tiles[index] = TileKind::Safe;
        }
    }
    tiles[cell_index(start)] = TileKind::Safe;
    tiles[cell_index(goal)] = TileKind::Reward;

    // Obstacle-field hazards punish wandering without ever blocking the spine.
    let field_candidates = obstacle_candidates(&tiles, &spine, start, goal);
    let field_hazards = 1 + usize::from(rng.random_bool(0.6));
    for coordinate in choose_distinct(&mut rng, &field_candidates, field_hazards) {
        tiles[cell_index(coordinate)] = TileKind::Danger;
    }

    // Branch hazards sit on safe cells that are neither on the shortest route nor a structural
    // branch, so they punish taking the wrong fork while leaving both the route and the forks
    // intact. Placing them anywhere else is self-defeating: an earlier version of this generator
    // could drop a hazard onto a dead-end spur and erase the very branch it was meant to guard,
    // silently returning the seed to a bare corridor.
    let branch_budget = 1 + usize::from(rng.random_bool(0.5));
    let mut placed_on_branches = 0;
    let mut guard = 0;
    let route = shortest_safe_route_cells(&tiles, start, goal);
    let branch_candidates = branch_candidates(&tiles, &spine, start, goal, &route);
    while placed_on_branches < branch_budget && guard < 24 {
        guard += 1;
        let Some(coordinate) = pick_branch_candidate(&mut rng, &tiles, &branch_candidates) else {
            break;
        };
        tiles[cell_index(coordinate)] = TileKind::Danger;
        if shortest_safe_route_length(&tiles, start, goal).is_none() {
            tiles[cell_index(coordinate)] = TileKind::Safe;
        } else {
            placed_on_branches += 1;
        }
    }

    let shortest_safe_path = shortest_safe_route_length(&tiles, start, goal)
        .expect("the spine guarantees a hazard-free route to the goal");
    let max_turns = turn_budget_for(shortest_safe_path);
    GridWorldLayout {
        tiles,
        start,
        goal,
        shortest_safe_path,
        max_turns,
    }
}

/// The per-seed turn budget: `ceil(TURN_BUDGET_NUMERATOR/DENOMINATOR * shortest_safe_path)`.
///
/// The 1.5 factor is the whole calibration. It is deliberately ungenerous: it leaves room for one
/// wrong turn and a correction, and nothing more. A policy that has to backtrack out of a dead end
/// can afford exactly one such mistake.
pub fn turn_budget_for(shortest_safe_path: u32) -> u32 {
    let scaled = u64::from(shortest_safe_path) * u64::from(TURN_BUDGET_NUMERATOR);
    let denominator = u64::from(TURN_BUDGET_DENOMINATOR);
    let rounded = scaled.div_ceil(denominator);
    u32::try_from(rounded.max(1)).unwrap_or(u32::MAX)
}

/// The largest turn budget any seed on this grid can require.
pub fn maximum_turn_budget() -> u32 {
    turn_budget_for(GRID_DIAMETER)
}

/// Draws the start and the goal, both per seed.
///
/// Moving the **start** is the change that matters most. With the start pinned at a corner, every
/// other tile lies down-and-to-the-right of it, so "prefer DOWN, then RIGHT" is not a heuristic at
/// all — it is an exact distance descent, and it solves the grid without ever reasoning. Measured on
/// the first version of this generator, the greedy baseline scored 60 of 70 evaluation seeds *for
/// that reason alone*. With the start drawn per seed the goal is as often up-and-to-the-left as down
/// and to the right, so no fixed direction is correct and a policy has to work out which way is
/// actually closer.
fn choose_endpoints(rng: &mut ChaCha8Rng) -> (Coordinate, Coordinate) {
    let all = (0..25).map(coordinate_from_index).collect::<Vec<_>>();
    let start = all[rng.random_range(0..all.len())];
    let far_enough = all
        .iter()
        .copied()
        .filter(|cell| *cell != start && manhattan(*cell, start) >= 5)
        .collect::<Vec<_>>();
    let goal = if far_enough.is_empty() {
        all[(rng.random_range(0..all.len()) + 1) % all.len()]
    } else {
        far_enough[rng.random_range(0..far_enough.len())]
    };
    (start, goal)
}

/// A seeded monotone staircase from the start to the goal.
///
/// Because every step increases `x` or `y`, the spine is always exactly `goal.x + goal.y` moves long
/// and always exists. Its only job is to guarantee solvability; the branching happens next.
fn monotone_spine(rng: &mut ChaCha8Rng, start: Coordinate, goal: Coordinate) -> Vec<Coordinate> {
    let mut path = vec![start];
    let mut x = start.x;
    let mut y = start.y;
    while x != goal.x || y != goal.y {
        let horizontal_first =
            (x != goal.x) && (y == goal.y || (x == goal.x) || rng.random_bool(0.5));
        if horizontal_first {
            x = if x < goal.x {
                x + 1
            } else {
                x.saturating_sub(1)
            };
        } else {
            y = if y < goal.y {
                y + 1
            } else {
                y.saturating_sub(1)
            };
        }
        path.push(Coordinate { x, y });
    }
    path
}

/// Grows the safe region into a tree with real forks.
///
/// A spur is hung off **every** eligible spine cell rather than off a seeded subset. The first
/// version of this generator picked one or two anchors at random and silently skipped any that had
/// no free neighbour, which meant a large fraction of seeds came out as a bare path with no branch
/// anywhere — the exact defect this redesign exists to remove, reintroduced through a different
/// route. A benchmark whose safe region is a path is a corridor, and a corridor is solvable by
/// preferring one direction.
///
/// Each spur is one or two cells deep and dead-ends, which is the trap that defeats a policy with no
/// memory: at the fork both branches look safe, and walking the wrong one has to be undone.
fn build_safe_region(
    rng: &mut ChaCha8Rng,
    spine: &[Coordinate],
    start: Coordinate,
    goal: Coordinate,
) -> [bool; 25] {
    let mut safe = [false; 25];
    for coordinate in spine {
        safe[cell_index(*coordinate)] = true;
    }
    // Two candidates per anchor, tried in a seeded order, so the shape still varies by seed while
    // the *existence* of a branch does not.
    for anchor in spine
        .iter()
        .copied()
        .skip(1)
        .take(spine.len().saturating_sub(2))
    {
        if anchor == start {
            continue;
        }
        let mut options = open_neighbours(anchor)
            .into_iter()
            .filter(|cell| {
                !safe[cell_index(*cell)] && *cell != goal && manhattan(*cell, start) >= 2
            })
            .collect::<Vec<_>>();
        if options.is_empty() {
            continue;
        }
        // Rotate the candidate list by a seeded offset instead of sampling, so every option is
        // reachable and the choice is still seed-dependent.
        let offset = rng.random_range(0..options.len());
        options.rotate_left(offset);
        let Some(mut cursor) = options.first().copied() else {
            continue;
        };
        safe[cell_index(cursor)] = true;
        let spur_length = 1 + usize::from(rng.random_bool(0.5));
        for _ in 1..spur_length {
            let next = open_neighbours(cursor)
                .into_iter()
                .filter(|cell| !safe[cell_index(*cell)] && *cell != goal)
                .collect::<Vec<_>>();
            let Some(cell) = next.first().copied() else {
                break;
            };
            safe[cell_index(cell)] = true;
            cursor = cell;
        }
    }
    safe
}

/// Non-spine, non-goal cells that may hold an obstacle-field hazard.
fn obstacle_candidates(
    tiles: &[TileKind; 25],
    spine: &[Coordinate],
    start: Coordinate,
    goal: Coordinate,
) -> Vec<Coordinate> {
    let mut candidates = (0..25)
        .map(coordinate_from_index)
        .filter(|coordinate| *coordinate != start && *coordinate != goal)
        .filter(|coordinate| tiles[cell_index(*coordinate)] == TileKind::Obstacle)
        .filter(|coordinate| {
            manhattan(*coordinate, start) >= 2
                && !spine.contains(coordinate)
                && manhattan(*coordinate, goal) >= 2
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|coordinate| (coordinate.y, coordinate.x));
    candidates
}

/// Route cells that may hold a branch hazard.
///
/// Candidates are restricted to the **spine** and to cells off the shortest route. That separation is
/// the point: a hazard on a spine cell punishes committing to the wrong way along the main route,
/// while a dead-end spur is deliberately left safe so it remains a trap. An earlier version picked
/// from every safe cell, and routinely dropped a hazard onto a spur tip — deleting the branch it was
/// meant to guard and quietly returning the seed to a bare corridor.
fn branch_candidates(
    tiles: &[TileKind; 25],
    spine: &[Coordinate],
    start: Coordinate,
    goal: Coordinate,
    route: &[Coordinate],
) -> Vec<Coordinate> {
    let mut candidates = spine
        .iter()
        .copied()
        .filter(|coordinate| {
            tiles[cell_index(*coordinate)] == TileKind::Safe
                && *coordinate != start
                && *coordinate != goal
                && !route.contains(coordinate)
                && traversable_neighbours(tiles, *coordinate) < 3
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|coordinate| (coordinate.y, coordinate.x));
    candidates
}

/// How many of a cell's orthogonal neighbours are traversable in this tile array.
fn traversable_neighbours(tiles: &[TileKind; 25], coordinate: Coordinate) -> usize {
    open_neighbours(coordinate)
        .into_iter()
        .filter(|cell| matches!(tiles[cell_index(*cell)], TileKind::Safe | TileKind::Reward))
        .count()
}

/// The cells on one shortest hazard-free route from the start to the goal.
fn shortest_safe_route_cells(
    tiles: &[TileKind; 25],
    start: Coordinate,
    goal: Coordinate,
) -> Vec<Coordinate> {
    let mut seen = [false; 25];
    let mut came_from = [usize::MAX; 25];
    let mut queue = std::collections::VecDeque::from([start]);
    seen[cell_index(start)] = true;
    while let Some(cell) = queue.pop_front() {
        if cell == goal {
            break;
        }
        for next in open_neighbours(cell) {
            let index = cell_index(next);
            if seen[index] || !matches!(tiles[index], TileKind::Safe | TileKind::Reward) {
                continue;
            }
            seen[index] = true;
            came_from[index] = cell_index(cell);
            queue.push_back(next);
        }
    }
    if !seen[cell_index(goal)] {
        return Vec::new();
    }
    let mut path = vec![goal];
    let mut index = cell_index(goal);
    while index != cell_index(start) {
        let parent = came_from[index];
        if parent == usize::MAX {
            return Vec::new();
        }
        index = parent;
        path.push(coordinate_from_index(index));
    }
    path.reverse();
    path
}

fn pick_branch_candidate(
    rng: &mut ChaCha8Rng,
    tiles: &[TileKind; 25],
    candidates: &[Coordinate],
) -> Option<Coordinate> {
    let open = candidates
        .iter()
        .copied()
        .filter(|coordinate| tiles[cell_index(*coordinate)] == TileKind::Safe)
        .collect::<Vec<_>>();
    choose_one(rng, &open)
}

/// Picks `count` distinct entries, deterministically.
fn choose_distinct(
    rng: &mut ChaCha8Rng,
    candidates: &[Coordinate],
    count: usize,
) -> Vec<Coordinate> {
    let mut remaining = candidates.to_vec();
    let mut chosen = Vec::new();
    while chosen.len() < count && !remaining.is_empty() {
        let index = rng.random_range(0..remaining.len());
        chosen.push(remaining.remove(index));
    }
    chosen
}

/// The length in moves of the shortest route from the start to the goal that never enters a hazard.
///
/// Obstacles and hazards are both impassable here, which is the same rule the environment applies.
pub fn shortest_safe_route_length(
    tiles: &[TileKind; 25],
    start: Coordinate,
    goal: Coordinate,
) -> Option<u32> {
    let mut seen = [false; 25];
    let mut frontier = std::collections::VecDeque::from([(start, 0u32)]);
    seen[cell_index(start)] = true;
    while let Some((coordinate, distance)) = frontier.pop_front() {
        if coordinate == goal {
            return Some(distance);
        }
        for next in open_neighbours(coordinate) {
            let index = cell_index(next);
            if seen[index] {
                continue;
            }
            if !matches!(tiles[index], TileKind::Safe | TileKind::Reward) {
                continue;
            }
            seen[index] = true;
            frontier.push_back((next, distance.saturating_add(1)));
        }
    }
    None
}

/// The full coordinate route along a seed's shortest hazard-free path, start to goal.
///
/// Test- and harness-facing: a model is never handed this, it only ever sees its own observation.
#[cfg(test)]
fn shortest_safe_route(layout: &GridWorldLayout) -> Option<Vec<Coordinate>> {
    use std::collections::BTreeMap;
    let start = (layout.start().x, layout.start().y);
    let goal = (layout.goal().x, layout.goal().y);
    let mut previous: BTreeMap<(u8, u8), (u8, u8)> = BTreeMap::new();
    let mut seen: std::collections::BTreeSet<(u8, u8)> = std::collections::BTreeSet::new();
    let mut queue = std::collections::VecDeque::from([start]);
    seen.insert(start);
    while let Some(cell) = queue.pop_front() {
        if cell == goal {
            break;
        }
        for next in open_neighbours(Coordinate {
            x: cell.0,
            y: cell.1,
        }) {
            let key = (next.x, next.y);
            if seen.contains(&key) {
                continue;
            }
            if !matches!(layout.tile(next), TileKind::Safe | TileKind::Reward) {
                continue;
            }
            seen.insert(key);
            previous.insert(key, cell);
            queue.push_back(key);
        }
    }
    let mut path = vec![goal];
    let mut cursor = goal;
    while cursor != start {
        cursor = *previous.get(&cursor)?;
        path.push(cursor);
    }
    path.reverse();
    Some(path.into_iter().map(|(x, y)| Coordinate { x, y }).collect())
}

/// The in-bounds orthogonal neighbours of a coordinate.
///
/// Public because the benchmark's tests need to reason about adjacency against a layout, and a
/// model never receives this: it only ever sees its own `Observation`.
pub fn orthogonal_coordinates(coordinate: Coordinate) -> Vec<Coordinate> {
    open_neighbours(coordinate)
}

/// How many of a cell's orthogonal neighbours are traversable in this layout.
pub fn neighbour_count(layout: &GridWorldLayout, coordinate: Coordinate) -> usize {
    open_neighbours(coordinate)
        .into_iter()
        .filter(|cell| matches!(layout.tile(*cell), TileKind::Safe | TileKind::Reward))
        .count()
}

fn open_neighbours(coordinate: Coordinate) -> Vec<Coordinate> {
    let mut cells = Vec::with_capacity(4);
    if coordinate.x > 0 {
        cells.push(Coordinate {
            x: coordinate.x - 1,
            y: coordinate.y,
        });
    }
    if coordinate.y > 0 {
        cells.push(Coordinate {
            x: coordinate.x,
            y: coordinate.y - 1,
        });
    }
    if coordinate.x + 1 < WIDTH {
        cells.push(Coordinate {
            x: coordinate.x + 1,
            y: coordinate.y,
        });
    }
    if coordinate.y + 1 < HEIGHT {
        cells.push(Coordinate {
            x: coordinate.x,
            y: coordinate.y + 1,
        });
    }
    cells
}

/// Manhattan distance between two coordinates.
pub fn manhattan(left: Coordinate, right: Coordinate) -> u32 {
    u32::from(left.x.abs_diff(right.x)) + u32::from(left.y.abs_diff(right.y))
}

/// Deterministically picks one element, or `None` when there is nothing to pick.
fn choose_one<T: Copy>(rng: &mut ChaCha8Rng, items: &[T]) -> Option<T> {
    if items.is_empty() {
        None
    } else {
        Some(items[rng.random_range(0..items.len())])
    }
}
/// The action that moves from one cell to an orthogonally adjacent one.
///
/// Written from two absolute coordinates rather than an absolute difference, because `abs_diff`
/// cannot distinguish "one to the left" from "one to the right" and would silently send every
/// westward step east.
#[cfg(test)]
fn action_between(from: Coordinate, to: Coordinate) -> Action {
    if to.x > from.x {
        Action::Right
    } else if to.x < from.x {
        Action::Left
    } else if to.y > from.y {
        Action::Down
    } else {
        Action::Up
    }
}

/// Where an action would land, or `None` when it would leave the grid.
#[cfg(test)]
fn action_coordinate_for_test(action: Action, from: Coordinate) -> Option<Coordinate> {
    open_neighbours(from)
        .into_iter()
        .find(|cell| action_between(from, *cell) == action)
}

/// The action a one-step-lookahead policy picks, with no memory of anything it has already tried.
///
/// This is the reference "trivial heuristic" the benchmark is calibrated against: reward if adjacent,
/// otherwise any adjacent safe tile, preferring DOWN then RIGHT. It is exactly the policy the audit
/// found to be provably optimal on the old fixed-corridor generator, so holding it to the same
/// standard is what makes the new numbers mean something.
#[cfg(test)]
fn myopic_action(observation: &probelab_core::Observation) -> Action {
    myopic_for(observation)
}

#[cfg(test)]
fn myopic_for(observation: &probelab_core::Observation) -> Action {
    let here = observation.position;
    let targets = |action: Action, tile: TileKind| {
        observation.visible_neighbors.iter().any(|neighbor| {
            neighbor.tile == tile
                && match action {
                    Action::Up => {
                        neighbor.coordinate.x == here.x && neighbor.coordinate.y + 1 == here.y
                    }
                    Action::Down => {
                        neighbor.coordinate.x == here.x && here.y + 1 == neighbor.coordinate.y
                    }
                    Action::Left => {
                        neighbor.coordinate.y == here.y && neighbor.coordinate.x + 1 == here.x
                    }
                    Action::Right => {
                        neighbor.coordinate.y == here.y && here.x + 1 == neighbor.coordinate.x
                    }
                    Action::Wait => false,
                }
        })
    };
    for action in [Action::Right, Action::Down] {
        if targets(action, TileKind::Reward) {
            return action;
        }
    }
    // Any safe neighbour beats waiting: a myopic policy never suicides, it just cannot remember.
    for action in [Action::Down, Action::Right, Action::Up, Action::Left] {
        if targets(action, TileKind::Safe) {
            return action;
        }
    }
    Action::Wait
}

#[cfg(test)]
mod tests {
    use super::{
        action_between, action_coordinate_for_test, myopic_action, shortest_safe_route, GridWorld,
        GridWorldSandbox, HAZARD_PENALTY, WIDTH,
    };
    use probelab_core::{
        Action, ActionOutcome, ActionResultKind, Environment, EpisodeStatus, RuntimeAuthorization,
        Sandbox, StepOutcome, TileKind,
    };

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
        assert_eq!(first.layout, second.layout);
        assert_ne!(first.layout, different.layout);
        assert!(first.layout.tiles().contains(&TileKind::Danger));
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
            .map(|neighbor| action_between(observation.position, neighbor.coordinate))
            .expect("safe start neighbor");
        let before = observation.distance_to_goal;
        let step = world.step(action).expect("valid movement");
        let outcome = action_outcome(&step);
        assert_eq!(outcome.kind, ActionResultKind::Moved);
        assert_eq!(step.status, EpisodeStatus::Running);
        assert_ne!(outcome.position, observation.position);
        // Reward is now shaped: exactly the change in distance to the goal, so moving one tile
        // closer pays +1 and one tile further pays -1. The old terminal-only score gave 0 here,
        // which is why two failures used to be indistinguishable.
        let after = world.observation().distance_to_goal;
        assert_eq!(outcome.reward, i64::from(before) - i64::from(after));
    }

    #[test]
    fn obstacle_or_boundary_action_is_invalid_and_consumes_turn() {
        // The start is per seed now, so "Up" is not automatically a boundary any more. Find a move
        // that is genuinely refused — off the grid, or into an obstacle — and assert on that.
        let mut world = GridWorld::new(123).expect("valid grid world");
        let observation = world.observation();
        let here = observation.position;
        let refused = [Action::Up, Action::Down, Action::Left, Action::Right]
            .into_iter()
            .find(|action| {
                let Some(target) = action_coordinate_for_test(*action, here) else {
                    return true;
                };
                world.tile_at(target) == TileKind::Obstacle
            })
            .expect("some move is refused from the start");
        let outcome = world.step(refused).expect("refused movement");
        let action_outcome = action_outcome(&outcome);
        assert_eq!(action_outcome.kind, ActionResultKind::Invalid);
        assert_eq!(action_outcome.reward, 0);
        assert_eq!(world.episode().turns_used, 1);
        assert_eq!(world.episode().invalid_action_count, 1);
    }

    /// The generated layout must actually contain a walkable route from the start to the goal, and a
    /// policy that follows the safe tiles must arrive inside the seed's own turn budget.
    ///
    /// This is the property the old generator had by construction and the new one has to earn: a
    /// branching tree of safe tiles can easily be generated with no route through it at all.
    #[test]
    fn the_generated_layout_contains_a_route_that_fits_the_turn_budget() {
        for seed in [123_u64, 2_000, 2_031, 2_069] {
            let layout = super::generate_layout(seed);
            let route = shortest_safe_route(&layout).expect("a safe route exists");
            assert_eq!(
                route.len() as u32 - 1,
                layout.shortest_safe_path(),
                "seed {seed} route length disagrees with the recorded shortest path"
            );
            assert!(
                layout.max_turns() >= layout.shortest_safe_path(),
                "seed {seed} is given fewer turns than its own route needs"
            );

            let mut world = GridWorld::new(seed).expect("valid grid world");
            for step in route.windows(2) {
                world
                    .step(action_between(step[0], step[1]))
                    .expect("route movement");
            }
            assert_eq!(
                world.episode().status,
                EpisodeStatus::Success,
                "seed {seed} could not be solved by following its own safe route"
            );
        }
    }
    #[test]
    fn entering_danger_fails_the_episode() {
        let mut world = GridWorld::new(123).expect("valid grid world");
        let danger = world
            .layout
            .hazards()
            .into_iter()
            .next()
            .expect("generated danger");
        let position = if danger.x > 0 {
            probelab_core::Coordinate {
                x: danger.x - 1,
                y: danger.y,
            }
        } else {
            probelab_core::Coordinate { x: 0, y: 0 }
        };
        world.position = position;
        let before = world.observation().distance_to_goal;
        let outcome = world.step(Action::Right).expect("danger movement");
        assert_eq!(outcome.status, EpisodeStatus::Failure);
        // The hazard penalty is charged in full *plus* whatever progress the step made, so a death
        // that happened to move toward the goal still scores strictly worse than one that did not.
        let progress = i64::from(before) - i64::from(world.observation().distance_to_goal);
        assert_eq!(action_outcome(&outcome).reward, progress - HAZARD_PENALTY);
        assert!(action_outcome(&outcome).reward < 0);
        assert_eq!(
            world.episode().total_reward,
            action_outcome(&outcome).reward
        );
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

    /// The myopic one-step-lookahead policy does **not** solve every seed.
    ///
    /// This is the property the whole redesign exists to establish, asserted at the environment
    /// level rather than only in the statistical harness. The old benchmark had the opposite
    /// property, and that was the audit finding: a three-comparison rule that stepped toward a
    /// compile-time-constant goal solved every seed, so the benchmark could not tell a search policy
    /// from a coin flip.
    ///
    /// The rule below is deliberately the simplest thing that could beat a coin flip: take the
    /// reward if it is adjacent, otherwise take any adjacent safe tile, preferring DOWN then RIGHT.
    /// It has no memory and no lookahead beyond one tile.
    #[test]
    fn a_myopic_one_step_policy_fails_on_some_seeds() {
        let mut solved = 0;
        let mut examined = 0;
        for seed in 2_000..2_070u64 {
            let mut world = GridWorld::new(seed).expect("valid grid world");
            let budget = world.layout.max_turns();
            examined += 1;
            while world.episode().status == EpisodeStatus::Running {
                let observation = world.observation();
                world
                    .step(myopic_action(&observation))
                    .expect("myopic movement");
            }
            if world.episode().status == EpisodeStatus::Success {
                solved += 1;
            }
            assert!(
                world.episode().turns_used <= budget,
                "seed {seed} overran its own budget"
            );
        }
        assert!(
            solved < examined,
            "a myopic one-step policy solved all {examined} seeds, so the benchmark measures nothing \
             beyond picking a safe neighbouring tile"
        );
    }

    /// A policy that follows the seed's own safe route, given the route, always arrives in budget.
    ///
    /// This is the control for the test above: it shows the failures are a property of the *policy*
    /// and not of an unsolvable generator.
    #[test]
    fn following_the_known_safe_route_always_succeeds() {
        for seed in 2_000..2_070u64 {
            let layout = super::generate_layout(seed);
            let route = shortest_safe_route(&layout).expect("a safe route exists");
            let mut world = GridWorld::new(seed).expect("valid grid world");
            for step in route.windows(2) {
                world
                    .step(action_between(step[0], step[1]))
                    .expect("route movement");
            }
            assert_eq!(
                world.episode().status,
                EpisodeStatus::Success,
                "seed {seed} was not solvable along its own safe route"
            );
        }
    }
    #[test]
    fn action_enum_cannot_represent_filesystem_or_network_actions() {
        for action in ["FILESYSTEM", "NETWORK"] {
            assert!(serde_json::from_str::<Action>(action).is_err());
        }
    }
}
