//! Embedded versioned benchmark definitions and a small built-in registry.

#![deny(missing_docs)]

use std::collections::BTreeMap;

use probelab_core::{Benchmark, BenchmarkSummary, Sandbox};
use probelab_sandbox::{GridWorldError, GridWorldSandbox};
use serde::{Deserialize, Serialize};

const GRID_WORLD_DEFINITION_JSON: &str =
    include_str!("../../../benchmarks/games/grid-world.v1.json");

/// Stable identifier of the built-in grid-world benchmark.
pub const GRID_WORLD_BENCHMARK_ID: &str = "grid-world";
/// Version of the built-in grid-world benchmark definition.
pub const GRID_WORLD_BENCHMARK_VERSION: &str = "1";
/// The only capability dimension represented by the grid-world benchmark.
pub const GRID_WORLD_CAPABILITY_DIMENSION: &str = "grid-world planning";

/// A validated, versioned grid-world definition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GridWorldDefinition {
    /// Stable benchmark identifier.
    pub id: String,
    /// Version of the benchmark definition.
    pub version: String,
    /// Human-readable benchmark title.
    pub title: String,
    /// Short description of the benchmark task.
    pub description: String,
    /// Single capability dimension measured by the benchmark.
    pub capability_dimension: String,
    /// Deterministic seed used when no explicit seed is selected.
    pub default_seed: u64,
    /// Maximum number of environment turns per episode.
    pub max_turns: u32,
    /// Frontend-safe descriptive metadata.
    pub metadata: BTreeMap<String, String>,
}

/// A typed grid-world definition validation failure.
#[derive(Debug, thiserror::Error)]
pub enum BenchmarkDefinitionError {
    /// The benchmark definition was not valid JSON.
    #[error("grid-world definition JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// The sandbox rejected the validated turn configuration.
    #[error(transparent)]
    Sandbox(#[from] GridWorldError),
    /// A required identity or presentation field was empty.
    #[error("grid-world definition field {field} must not be empty")]
    MissingIdentity {
        /// Name of the missing field.
        field: &'static str,
    },
    /// The identifier did not match the stable built-in identity.
    #[error("grid-world definition id must be {expected:?}, got {actual:?}")]
    UnexpectedId {
        /// Required stable identifier.
        expected: &'static str,
        /// Identifier supplied by the definition.
        actual: String,
    },
    /// The version did not match this embedded grid-world definition.
    #[error("grid-world definition version must be {expected:?}, got {actual:?}")]
    UnexpectedVersion {
        /// Required definition version.
        expected: &'static str,
        /// Version supplied by the definition.
        actual: String,
    },
    /// The turn limit was zero.
    #[error("grid-world definition max_turns must be greater than zero")]
    ZeroMaxTurns,
    /// The definition claimed more than the implemented capability dimension.
    #[error("grid-world capability_dimension must be {expected:?}, got {actual:?}")]
    CapabilityDimensionMismatch {
        /// Required capability dimension.
        expected: &'static str,
        /// Capability dimension supplied by the definition.
        actual: String,
    },
}

impl GridWorldDefinition {
    /// Parses and validates a serialized grid-world definition.
    pub fn from_json(json: &str) -> Result<Self, BenchmarkDefinitionError> {
        let definition: Self = serde_json::from_str(json)?;
        definition.validate()?;
        Ok(definition)
    }

    /// Validates all identity, version, turn-limit, and capability requirements.
    pub fn validate(&self) -> Result<(), BenchmarkDefinitionError> {
        for (field, value) in [
            ("id", &self.id),
            ("version", &self.version),
            ("title", &self.title),
            ("description", &self.description),
        ] {
            if value.trim().is_empty() {
                return Err(BenchmarkDefinitionError::MissingIdentity { field });
            }
        }
        if self.id != GRID_WORLD_BENCHMARK_ID {
            return Err(BenchmarkDefinitionError::UnexpectedId {
                expected: GRID_WORLD_BENCHMARK_ID,
                actual: self.id.clone(),
            });
        }
        if self.version != GRID_WORLD_BENCHMARK_VERSION {
            return Err(BenchmarkDefinitionError::UnexpectedVersion {
                expected: GRID_WORLD_BENCHMARK_VERSION,
                actual: self.version.clone(),
            });
        }
        if self.max_turns == 0 {
            return Err(BenchmarkDefinitionError::ZeroMaxTurns);
        }
        if self.capability_dimension != GRID_WORLD_CAPABILITY_DIMENSION {
            return Err(BenchmarkDefinitionError::CapabilityDimensionMismatch {
                expected: GRID_WORLD_CAPABILITY_DIMENSION,
                actual: self.capability_dimension.clone(),
            });
        }
        Ok(())
    }

    fn summary(&self) -> BenchmarkSummary {
        BenchmarkSummary {
            benchmark_id: self.id.clone(),
            benchmark_version: self.version.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            capability_dimension: self.capability_dimension.clone(),
            seed: self.default_seed,
            max_turns: self.max_turns,
            metadata: self.metadata.clone(),
        }
    }
}

/// The executable built-in grid-world benchmark.
#[derive(Clone, Debug)]
pub struct GridWorldBenchmark {
    definition: GridWorldDefinition,
}

impl GridWorldBenchmark {
    /// Loads the embedded grid-world definition and validates it.
    pub fn built_in() -> Result<Self, BenchmarkDefinitionError> {
        Self::from_json(GRID_WORLD_DEFINITION_JSON)
    }

    /// Parses and validates a serialized grid-world definition.
    pub fn from_json(json: &str) -> Result<Self, BenchmarkDefinitionError> {
        Self::from_definition(GridWorldDefinition::from_json(json)?)
    }

    /// Validates and owns a grid-world definition.
    pub fn from_definition(
        definition: GridWorldDefinition,
    ) -> Result<Self, BenchmarkDefinitionError> {
        definition.validate()?;
        Ok(Self { definition })
    }

    /// Returns the validated embedded definition.
    pub fn definition(&self) -> &GridWorldDefinition {
        &self.definition
    }

    /// Creates a private grid world wrapped by its capability-restricted sandbox.
    pub fn create_sandbox(&self, seed: u64) -> Result<Box<dyn Sandbox>, BenchmarkDefinitionError> {
        let sandbox = GridWorldSandbox::with_max_turns(seed, self.definition.max_turns)?;
        Ok(Box::new(sandbox))
    }
}

impl Benchmark for GridWorldBenchmark {
    fn summary(&self) -> BenchmarkSummary {
        self.definition.summary()
    }

    /// Grants nothing at all: the grid-world benchmark has no filesystem, process, network, or
    /// environment capability to declare, so its policy stays deny-all.
    fn capability_policy(&self) -> probelab_core::CapabilityPolicy {
        probelab_core::CapabilityPolicy::deny_all()
    }

    /// Declares the evaluation-critical source tree as unreadable to any contained child.
    ///
    /// These paths hold the benchmark definitions and the evaluator, which together are the answer
    /// key. They are named here, by trusted compiled-in code, and are never influenced by a model
    /// response. The grid-world benchmark grants no process capability, so today no child exists to
    /// protect them from; declaring them anyway means a future grant cannot forget to.
    fn protected_paths(&self) -> &[std::path::PathBuf] {
        use std::sync::OnceLock;
        static PATHS: OnceLock<Vec<std::path::PathBuf>> = OnceLock::new();
        PATHS.get_or_init(|| {
            let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(std::path::Path::parent)
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default();
            vec![
                workspace.join("crates").join("evaluator"),
                workspace.join("crates").join("benchmarks"),
                workspace.join("benchmarks"),
            ]
        })
    }

    fn create_sandbox(&self, seed: u64) -> Result<Box<dyn Sandbox>, String> {
        Self::create_sandbox(self, seed).map_err(|error| error.to_string())
    }
}

/// A typed benchmark catalog resolution failure.
#[derive(Debug, thiserror::Error)]
pub enum BenchmarkCatalogError {
    /// The built-in benchmark definition could not be loaded.
    #[error("built-in benchmark definition is invalid: {0}")]
    Definition(#[from] BenchmarkDefinitionError),
    /// No built-in benchmark matched the requested identifier.
    #[error("unknown benchmark id {id:?}")]
    UnknownBenchmark {
        /// Unresolved benchmark identifier.
        id: String,
    },
}

/// A deterministic registry of the benchmarks compiled into ProbeLab.
pub struct BenchmarkCatalog {
    summaries: Vec<BenchmarkSummary>,
    grid_world: GridWorldBenchmark,
}

impl BenchmarkCatalog {
    /// Builds the catalog from validated, statically registered definitions.
    pub fn new() -> Result<Self, BenchmarkCatalogError> {
        let grid_world = GridWorldBenchmark::built_in()?;
        let summaries = vec![grid_world.summary()];
        Ok(Self {
            summaries,
            grid_world,
        })
    }

    /// Lists summaries for every statically registered benchmark.
    pub fn summaries(&self) -> &[BenchmarkSummary] {
        &self.summaries
    }

    /// Resolves one built-in benchmark by its stable identifier.
    pub fn resolve(&self, id: &str) -> Result<Box<dyn Benchmark>, BenchmarkCatalogError> {
        if id == GRID_WORLD_BENCHMARK_ID {
            Ok(Box::new(self.grid_world.clone()))
        } else {
            Err(BenchmarkCatalogError::UnknownBenchmark { id: id.to_owned() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BenchmarkCatalog, BenchmarkDefinitionError, GridWorldBenchmark, GRID_WORLD_BENCHMARK_ID,
        GRID_WORLD_BENCHMARK_VERSION, GRID_WORLD_CAPABILITY_DIMENSION,
    };
    use probelab_core::Benchmark;

    #[test]
    fn built_in_definition_parses_and_validates() {
        let benchmark = GridWorldBenchmark::built_in().expect("built-in benchmark");
        assert_eq!(benchmark.definition().id, GRID_WORLD_BENCHMARK_ID);
        assert_eq!(benchmark.definition().version, GRID_WORLD_BENCHMARK_VERSION);
        assert_eq!(benchmark.definition().max_turns, 30);
        assert_eq!(
            benchmark.definition().capability_dimension,
            GRID_WORLD_CAPABILITY_DIMENSION
        );
        assert_eq!(benchmark.summary().seed, 123);
    }

    #[test]
    fn catalog_lists_and_resolves_the_single_real_benchmark() {
        let catalog = BenchmarkCatalog::new().expect("built-in catalog");
        assert_eq!(catalog.summaries().len(), 1);
        assert_eq!(catalog.summaries()[0].benchmark_id, "grid-world");
        let resolved = catalog.resolve("grid-world").expect("grid-world resolves");
        assert_eq!(resolved.summary().benchmark_version, "1");
        assert!(catalog.resolve("missing").is_err());
    }

    #[test]
    fn zero_turn_definition_is_rejected() {
        let mut definition = GridWorldBenchmark::built_in()
            .expect("built-in benchmark")
            .definition()
            .clone();
        definition.max_turns = 0;
        assert!(matches!(
            GridWorldBenchmark::from_definition(definition),
            Err(BenchmarkDefinitionError::ZeroMaxTurns)
        ));
    }

    #[test]
    fn grid_world_grants_no_capability() {
        let benchmark = GridWorldBenchmark::built_in().expect("built-in benchmark");
        let policy = benchmark.capability_policy();
        assert!(policy.is_deny_all());
        assert!(policy.filesystem_read_roots().is_empty());
        assert!(policy.filesystem_write_roots().is_empty());
        assert!(policy.allowed_programs().is_empty());
        assert!(policy.allowed_endpoints().is_empty());
    }
}
