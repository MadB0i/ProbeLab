//! Provider-backed agent implementation for the ProbeLab model boundary.

#![deny(missing_docs)]

mod reference;

pub use reference::{ReferenceAgent, ReferenceError, ReferenceKnowledge, ReferenceStrategy};

use std::sync::Arc;

use probelab_core::{
    Agent, AsyncModelProvider, ModelCallFailure, ModelCallFailureKind, ModelDecision,
    ModelDescriptor, ModelProvider, ModelRequest, ModelResponse,
};
use tokio::runtime::Runtime;

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

/// An agent that bridges an asynchronous provider to the synchronous v0.1 agent boundary.
pub struct AsyncProviderAgent {
    provider: Arc<dyn AsyncModelProvider>,
    runtime: Runtime,
}

impl AsyncProviderAgent {
    /// Creates an agent with a dedicated current-thread Tokio runtime.
    pub fn new(provider: Arc<dyn AsyncModelProvider>) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "failed to create the async provider runtime".to_owned())?;
        Ok(Self { provider, runtime })
    }

    /// Creates an agent from a concrete asynchronous provider.
    pub fn from_provider<P>(provider: P) -> Result<Self, String>
    where
        P: AsyncModelProvider + 'static,
    {
        Self::new(Arc::new(provider))
    }
}

impl Agent for AsyncProviderAgent {
    fn descriptor(&self) -> ModelDescriptor {
        self.provider.descriptor()
    }

    fn decide(&mut self, request: ModelRequest) -> Result<ModelDecision, String> {
        Agent::decide_with_metadata(self, request)
            .map(|response| response.decision)
            .map_err(|failure| failure.to_string())
    }

    fn decide_with_metadata(
        &mut self,
        request: ModelRequest,
    ) -> Result<ModelResponse, ModelCallFailure> {
        request.validate().map_err(|_| {
            ModelCallFailure::new(
                ModelCallFailureKind::Configuration,
                "model request validation failed",
            )
        })?;
        let provider = Arc::clone(&self.provider);
        let runtime = &self.runtime;
        let result = std::thread::scope(|scope| {
            scope
                .spawn(move || runtime.block_on(provider.predict(request)))
                .join()
        });

        match result {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(failure)) => Err(failure),
            Err(_) => Err(ModelCallFailure::new(
                ModelCallFailureKind::ProviderError,
                "async model provider worker thread failed",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AsyncProviderAgent, ProviderAgent};
    use probelab_core::{
        Action, Agent, AsyncModelProvider, ModelCallInfo, ModelDecision, ModelDescriptor,
        ModelProvider, ModelProviderFuture, ModelRequest, ModelResponse, Observation,
    };

    struct TestProvider;

    struct MetadataProvider;

    impl AsyncModelProvider for MetadataProvider {
        fn descriptor(&self) -> ModelDescriptor {
            ModelDescriptor {
                adapter: "async-test".to_owned(),
                strategy: "fixed".to_owned(),
                seed: 0,
                parameters: Default::default(),
            }
        }

        fn predict(&self, _request: ModelRequest) -> ModelProviderFuture<'_> {
            Box::pin(async {
                Ok(ModelResponse {
                    decision: ModelDecision::GridAction(Action::Wait),
                    call: Some(ModelCallInfo {
                        provider: "test".to_owned(),
                        model: "test-model".to_owned(),
                        prompt_template_version: "test-v1".to_owned(),
                        request_id: None,
                        finish_reason: Some("stop".to_owned()),
                        usage: None,
                        timeout_seconds: Some(5),
                        elapsed_ms: 1,
                        response_text: Some("WAIT".to_owned()),
                    }),
                })
            })
        }
    }

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
            distance_to_goal: 1,
            max_turns: 1,
            remaining_turns: 1,
            available_actions: vec![Action::Wait],
        };
        let request = ModelRequest {
            episode_id: "episode".to_owned(),
            turn: 1,
            observation,
            available_actions: vec![Action::Wait],
            process_spawn_available: false,
            game: None,
            history: Vec::new(),
        };

        assert_eq!(agent.descriptor().strategy, "fixed");
        assert_eq!(
            agent.decide(request),
            Ok(ModelDecision::GridAction(Action::Wait))
        );
    }

    #[test]
    fn async_agent_bridges_metadata_from_a_dedicated_runtime() {
        let mut agent = AsyncProviderAgent::from_provider(MetadataProvider).expect("async agent");
        let request = ModelRequest {
            episode_id: "episode".to_owned(),
            turn: 1,
            observation: Observation {
                turn: 1,
                position: probelab_core::Coordinate { x: 0, y: 0 },
                grid_size: probelab_core::GridSize {
                    width: 1,
                    height: 1,
                },
                current_tile: probelab_core::TileKind::Safe,
                visible_neighbors: Vec::new(),
                distance_to_goal: 1,
                max_turns: 1,
                remaining_turns: 1,
                available_actions: vec![Action::Wait],
            },
            available_actions: vec![Action::Wait],
            process_spawn_available: false,
            game: None,
            history: Vec::new(),
        };
        let response = agent.decide_with_metadata(request).expect("async decision");
        assert_eq!(response.decision, ModelDecision::GridAction(Action::Wait));
        assert_eq!(response.call.expect("call metadata").provider, "test");
    }
}
