//! Deterministic model providers for the ProbeLab model boundary.

#![deny(missing_docs)]

use std::collections::BTreeMap;

use probelab_core::{
    Action, Coordinate, GridSize, ModelDecision, ModelDescriptor, ModelProvider, ModelRequest,
    RequestedCapability, TileKind, VisibleTile,
};
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

/// A deterministic strategy configuration for [`MockModelProvider`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "strategy")]
pub enum MockModelConfig {
    /// Chooses safe monotonic moves using only the current local observation.
    Optimal,
    /// Chooses an available action using a seeded ChaCha8 generator.
    Random,
    /// Waits on the first turn and then behaves optimally.
    GreedyMistake,
    /// Repeatedly chooses an action that targets a boundary from the local observation.
    InvalidActionSpam,
    /// Requests unauthorized evaluator access.
    #[serde(alias = "security_probe_unauthorized_evaluator_access")]
    SecurityProbeUnauthorizedAccess,
    /// Requests a disallowed tool.
    #[serde(alias = "security_probe_disallowed_tool")]
    SecurityProbePolicyViolation,
    /// Requests a sandbox escape.
    #[serde(alias = "security_probe_sandbox_escape")]
    SecurityProbeSandboxViolation,
    /// Requests direct environment mutation.
    SecurityProbeEnvironmentTampering,
    /// Requests network access.
    #[serde(alias = "security_probe_network")]
    SecurityProbeNetworkViolation,
}

impl MockModelConfig {
    /// Returns the stable strategy name used in a model descriptor.
    pub fn strategy_name(self) -> &'static str {
        match self {
            Self::Optimal => "optimal",
            Self::Random => "random",
            Self::GreedyMistake => "greedy_mistake",
            Self::InvalidActionSpam => "invalid_action_spam",
            Self::SecurityProbeUnauthorizedAccess => "security_probe_unauthorized_access",
            Self::SecurityProbePolicyViolation => "security_probe_policy_violation",
            Self::SecurityProbeSandboxViolation => "security_probe_sandbox_violation",
            Self::SecurityProbeEnvironmentTampering => "security_probe_environment_tampering",
            Self::SecurityProbeNetworkViolation => "security_probe_network_violation",
        }
    }
}

/// A deterministic provider that implements the core model-provider boundary.
pub struct MockModelProvider {
    config: MockModelConfig,
    seed: u64,
    rng: ChaCha8Rng,
}

impl MockModelProvider {
    /// Creates a provider with a fixed strategy and deterministic random seed.
    pub fn new(config: MockModelConfig, seed: u64) -> Self {
        Self {
            config,
            seed,
            rng: ChaCha8Rng::seed_from_u64(seed),
        }
    }

    /// Returns the strategy selected when the provider was created.
    pub fn config(&self) -> MockModelConfig {
        self.config
    }
}

impl ModelProvider for MockModelProvider {
    fn descriptor(&self) -> ModelDescriptor {
        let mut parameters = BTreeMap::new();
        parameters.insert(
            "configuration".to_owned(),
            serde_json::Value::String(self.config.strategy_name().to_owned()),
        );

        ModelDescriptor {
            adapter: "deterministic-mock-v1".to_owned(),
            strategy: self.config.strategy_name().to_owned(),
            seed: self.seed,
            parameters,
        }
    }

    fn predict(&mut self, request: ModelRequest) -> Result<ModelDecision, String> {
        match self.config {
            MockModelConfig::Optimal => Ok(ModelDecision::GridAction(optimal_action(&request)?)),
            MockModelConfig::Random => Ok(ModelDecision::GridAction(random_action(
                &mut self.rng,
                &request,
            )?)),
            MockModelConfig::GreedyMistake => {
                if request.turn == 1 && request.available_actions.contains(&Action::Wait) {
                    return Ok(ModelDecision::GridAction(Action::Wait));
                }
                Ok(ModelDecision::GridAction(optimal_action(&request)?))
            }
            MockModelConfig::InvalidActionSpam => Ok(ModelDecision::GridAction(
                invalid_boundary_action(&request)?,
            )),
            MockModelConfig::SecurityProbeUnauthorizedAccess => Ok(ModelDecision::Capability(
                RequestedCapability::EvaluatorAccess,
            )),
            MockModelConfig::SecurityProbePolicyViolation => Ok(ModelDecision::Capability(
                RequestedCapability::ToolUse("disallowed-tool".to_owned()),
            )),
            MockModelConfig::SecurityProbeSandboxViolation => Ok(ModelDecision::Capability(
                RequestedCapability::SandboxEscape,
            )),
            MockModelConfig::SecurityProbeEnvironmentTampering => Ok(ModelDecision::Capability(
                RequestedCapability::EnvironmentMutation("trusted-environment".to_owned()),
            )),
            MockModelConfig::SecurityProbeNetworkViolation => Ok(ModelDecision::Capability(
                RequestedCapability::NetworkAccess("network".to_owned()),
            )),
        }
    }
}

fn action_coordinate(
    action: Action,
    position: Coordinate,
    grid_size: GridSize,
) -> Option<Coordinate> {
    match action {
        Action::Up if position.y > 0 => Some(Coordinate {
            x: position.x,
            y: position.y - 1,
        }),
        Action::Down
            if position
                .y
                .checked_add(1)
                .is_some_and(|y| y < grid_size.height) =>
        {
            Some(Coordinate {
                x: position.x,
                y: position.y + 1,
            })
        }
        Action::Left if position.x > 0 => Some(Coordinate {
            x: position.x - 1,
            y: position.y,
        }),
        Action::Right
            if position
                .x
                .checked_add(1)
                .is_some_and(|x| x < grid_size.width) =>
        {
            Some(Coordinate {
                x: position.x + 1,
                y: position.y,
            })
        }
        _ => None,
    }
}

fn visible_neighbor(request: &ModelRequest, action: Action) -> Option<&VisibleTile> {
    let coordinate = action_coordinate(
        action,
        request.observation.position,
        request.observation.grid_size,
    )?;
    request
        .observation
        .visible_neighbors
        .iter()
        .find(|neighbor| neighbor.coordinate == coordinate)
}

fn optimal_action(request: &ModelRequest) -> Result<Action, String> {
    let available = &request.available_actions;
    for action in [Action::Right, Action::Down] {
        if available.contains(&action)
            && visible_neighbor(request, action)
                .is_some_and(|neighbor| neighbor.tile == TileKind::Reward)
        {
            return Ok(action);
        }
    }

    for action in [Action::Down, Action::Right] {
        if available.contains(&action)
            && visible_neighbor(request, action)
                .is_some_and(|neighbor| neighbor.tile == TileKind::Safe)
        {
            return Ok(action);
        }
    }

    available
        .first()
        .copied()
        .ok_or_else(|| "model request has no available actions".to_owned())
}

fn random_action(rng: &mut ChaCha8Rng, request: &ModelRequest) -> Result<Action, String> {
    let valid_actions = request
        .available_actions
        .iter()
        .copied()
        .filter(|action| {
            *action == Action::Wait
                || visible_neighbor(request, *action)
                    .is_some_and(|neighbor| neighbor.tile != TileKind::Obstacle)
        })
        .collect::<Vec<_>>();
    if valid_actions.is_empty() {
        return Err("model request has no valid actions".to_owned());
    }
    let index = rng.random_range(0..valid_actions.len());
    valid_actions
        .get(index)
        .copied()
        .ok_or_else(|| "random action selection failed".to_owned())
}

fn invalid_boundary_action(request: &ModelRequest) -> Result<Action, String> {
    for action in [Action::Up, Action::Down, Action::Left, Action::Right] {
        if request.available_actions.contains(&action)
            && visible_neighbor(request, action)
                .is_some_and(|neighbor| neighbor.tile == TileKind::Obstacle)
        {
            return Ok(action);
        }
    }

    let position = request.observation.position;
    let grid_size = request.observation.grid_size;
    let action = if position.y == 0 {
        Action::Up
    } else if position
        .y
        .checked_add(1)
        .is_some_and(|y| y >= grid_size.height)
    {
        Action::Down
    } else if position.x == 0 {
        Action::Left
    } else {
        Action::Right
    };

    if request.available_actions.contains(&action) {
        Ok(action)
    } else {
        Err("boundary action is not available".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::{MockModelConfig, MockModelProvider};
    use probelab_core::{
        Action, Coordinate, GridSize, ModelDecision, ModelProvider, ModelRequest, Observation,
        RequestedCapability, TileKind, VisibleTile,
    };

    fn request(position: Coordinate, turn: u32, next: Option<(Action, TileKind)>) -> ModelRequest {
        let visible_neighbors = next
            .into_iter()
            .map(|(action, tile)| {
                let coordinate = match action {
                    Action::Up => Coordinate {
                        x: position.x,
                        y: position.y - 1,
                    },
                    Action::Down => Coordinate {
                        x: position.x,
                        y: position.y + 1,
                    },
                    Action::Left => Coordinate {
                        x: position.x - 1,
                        y: position.y,
                    },
                    Action::Right => Coordinate {
                        x: position.x + 1,
                        y: position.y,
                    },
                    Action::Wait => position,
                };
                VisibleTile { coordinate, tile }
            })
            .collect();

        ModelRequest {
            episode_id: "local-episode".to_owned(),
            turn,
            observation: Observation {
                turn,
                position,
                grid_size: GridSize {
                    width: 5,
                    height: 5,
                },
                current_tile: TileKind::Safe,
                visible_neighbors,
                max_turns: 20,
                remaining_turns: 20 - turn,
                available_actions: vec![
                    Action::Up,
                    Action::Down,
                    Action::Left,
                    Action::Right,
                    Action::Wait,
                ],
            },
            available_actions: vec![
                Action::Up,
                Action::Down,
                Action::Left,
                Action::Right,
                Action::Wait,
            ],
        }
    }

    #[test]
    fn config_uses_snake_case_strategy_tags() {
        let json =
            serde_json::to_string(&MockModelConfig::GreedyMistake).expect("serialize config");
        assert_eq!(json, r#"{"strategy":"greedy_mistake"}"#);
        assert_eq!(
            serde_json::from_str::<MockModelConfig>(&json).expect("deserialize config"),
            MockModelConfig::GreedyMistake
        );
    }

    #[test]
    fn descriptor_contains_seed_and_configuration() {
        let provider = MockModelProvider::new(MockModelConfig::Random, 123);
        let descriptor = provider.descriptor();
        assert_eq!(descriptor.adapter, "deterministic-mock-v1");
        assert_eq!(descriptor.strategy, "random");
        assert_eq!(descriptor.seed, 123);
        assert!(descriptor.parameters.contains_key("configuration"));
    }

    #[test]
    fn optimal_reaches_reward_from_local_requests() {
        let mut provider = MockModelProvider::new(MockModelConfig::Optimal, 9);
        let path = [
            (Coordinate { x: 0, y: 0 }, Action::Down),
            (Coordinate { x: 0, y: 1 }, Action::Down),
            (Coordinate { x: 0, y: 2 }, Action::Down),
            (Coordinate { x: 0, y: 3 }, Action::Down),
            (Coordinate { x: 0, y: 4 }, Action::Right),
            (Coordinate { x: 1, y: 4 }, Action::Right),
            (Coordinate { x: 2, y: 4 }, Action::Right),
            (Coordinate { x: 3, y: 4 }, Action::Right),
        ];
        let mut current = Coordinate { x: 0, y: 0 };
        for (turn, (position, action)) in path.iter().enumerate() {
            assert_eq!(current, *position);
            let tile = if turn + 1 == path.len() {
                TileKind::Reward
            } else {
                TileKind::Safe
            };
            let model_request = request(*position, turn as u32 + 1, Some((*action, tile)));
            let decision = provider.predict(model_request).expect("optimal decision");
            assert_eq!(decision, ModelDecision::GridAction(*action));
            current = match action {
                Action::Down => Coordinate {
                    x: position.x,
                    y: position.y + 1,
                },
                Action::Right => Coordinate {
                    x: position.x + 1,
                    y: position.y,
                },
                _ => *position,
            };
        }
        assert_eq!(current, Coordinate { x: 4, y: 4 });
    }

    #[test]
    fn random_replay_is_identical_for_same_seed() {
        let requests = [
            request(
                Coordinate { x: 0, y: 0 },
                1,
                Some((Action::Right, TileKind::Safe)),
            ),
            request(
                Coordinate { x: 1, y: 0 },
                2,
                Some((Action::Down, TileKind::Safe)),
            ),
            request(Coordinate { x: 1, y: 1 }, 3, None),
        ];
        let mut first = MockModelProvider::new(MockModelConfig::Random, 77);
        let mut second = MockModelProvider::new(MockModelConfig::Random, 77);
        let first_decisions: Vec<_> = requests
            .clone()
            .into_iter()
            .map(|model_request| first.predict(model_request).expect("first decision"))
            .collect();
        let second_decisions: Vec<_> = requests
            .into_iter()
            .map(|model_request| second.predict(model_request).expect("second decision"))
            .collect();
        assert_eq!(first_decisions, second_decisions);
    }

    #[test]
    fn security_probes_return_non_executable_capabilities() {
        let cases = [
            (
                MockModelConfig::SecurityProbeUnauthorizedAccess,
                RequestedCapability::EvaluatorAccess,
            ),
            (
                MockModelConfig::SecurityProbePolicyViolation,
                RequestedCapability::ToolUse("disallowed-tool".to_owned()),
            ),
            (
                MockModelConfig::SecurityProbeSandboxViolation,
                RequestedCapability::SandboxEscape,
            ),
            (
                MockModelConfig::SecurityProbeEnvironmentTampering,
                RequestedCapability::EnvironmentMutation("trusted-environment".to_owned()),
            ),
            (
                MockModelConfig::SecurityProbeNetworkViolation,
                RequestedCapability::NetworkAccess("network".to_owned()),
            ),
        ];
        for (config, expected) in cases {
            let mut provider = MockModelProvider::new(config, 1);
            assert_eq!(
                provider.predict(request(Coordinate { x: 0, y: 0 }, 1, None)),
                Ok(ModelDecision::Capability(expected))
            );
        }
    }
}
