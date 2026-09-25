//! Provider-backed agent implementation for the ProbeLab model boundary.

#![deny(missing_docs)]

use probelab_core::{Agent, ModelDecision, ModelDescriptor, ModelProvider, ModelRequest};

/// An agent that owns a model provider and delegates model-facing decisions to it.
pub struct ProviderAgent {
    provider: Box<dyn ModelProvider>,
}

impl ProviderAgent {
    /// Creates an agent from an owned model provider trait object.
    pub fn new(provider: Box<dyn ModelProvider>) -> Self {
        Self { provider }
    }

    /// Creates an agent from a concrete model provider.
    pub fn from_provider<P>(provider: P) -> Self
    where
        P: ModelProvider + 'static,
    {
        Self::new(Box::new(provider))
    }
}

impl Agent for ProviderAgent {
    fn descriptor(&self) -> ModelDescriptor {
        self.provider.descriptor()
    }

    fn decide(&mut self, request: ModelRequest) -> Result<ModelDecision, String> {
        request.validate()?;
        self.provider.predict(request)
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderAgent;
    use probelab_core::{
        Action, Agent, ModelDecision, ModelDescriptor, ModelProvider, ModelRequest, Observation,
    };

    struct TestProvider;

    impl ModelProvider for TestProvider {
        fn descriptor(&self) -> ModelDescriptor {
            ModelDescriptor {
                adapter: "test".to_owned(),
                strategy: "fixed".to_owned(),
                seed: 7,
                parameters: Default::default(),
            }
        }

        fn predict(&mut self, _request: ModelRequest) -> Result<ModelDecision, String> {
            Ok(ModelDecision::GridAction(Action::Wait))
        }
    }

    #[test]
    fn delegates_descriptor_and_decision() {
        let mut agent = ProviderAgent::new(Box::new(TestProvider));
        let observation = Observation {
            turn: 1,
            position: probelab_core::Coordinate { x: 0, y: 0 },
            grid_size: probelab_core::GridSize {
                width: 1,
                height: 1,
            },
            current_tile: probelab_core::TileKind::Safe,
            visible_neighbors: Vec::new(),
            max_turns: 1,
            remaining_turns: 1,
            available_actions: vec![Action::Wait],
        };
        let request = ModelRequest {
            episode_id: "episode".to_owned(),
            turn: 1,
            observation,
            available_actions: vec![Action::Wait],
        };

        assert_eq!(agent.descriptor().strategy, "fixed");
        assert_eq!(
            agent.decide(request),
            Ok(ModelDecision::GridAction(Action::Wait))
        );
    }
}
