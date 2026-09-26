//! Embedded versioned benchmark definitions and a small built-in registry.

#![deny(missing_docs)]

use std::collections::BTreeMap;

use probelab_core::{Benchmark, BenchmarkSummary, Sandbox};
use probelab_sandbox::{grid_world_layout, GridWorldError, GridWorldSandbox};
use serde::{Deserialize, Serialize};

/// The fewest evaluation seeds a grid-world definition may declare.
///
/// A success rate over fewer than 30 episodes has a confidence interval too wide to distinguish
/// anything worth arguing about, so the definition refuses to be that small. This is a floor on the
/// declared pool, not a substitute for actually running the episodes.
pub const MINIMUM_EVAL_SEEDS: usize = 30;

const GRID_WORLD_DEFINITION_JSON: &str =
    include_str!("../../../benchmarks/games/grid-world.v1.json");

/// Stable identifier of the built-in grid-world benchmark.
pub const GRID_WORLD_BENCHMARK_ID: &str = "grid-world";
/// Version of the built-in grid-world benchmark definition.
///
/// Bumped to `2` because the layout, the observation history window, the reward function, and the
/// turn-budget policy all changed. A v1 replay digest cannot describe a v2 episode, and the two are
/// not comparable.
pub const GRID_WORLD_BENCHMARK_VERSION: &str = "2";
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
    /// Catalog ceiling on episode length, in turns.
    ///
    /// This is an upper bound over every seed, not any one seed's budget. A seed's real budget is
    /// `ceil(turn_budget_numerator/turn_budget_denominator * its own shortest safe route)` and is
    /// always at most this value.
    pub max_turns: u32,
    /// Numerator of the per-seed turn budget ratio.
    pub turn_budget_numerator: u32,
    /// Denominator of the per-seed turn budget ratio.
    pub turn_budget_denominator: u32,
    /// The disjoint seed pool reserved for building and tuning this benchmark.
    ///
    /// Every seed here is development data. A capability number computed on it is not a measurement.
    pub dev_seed_pool: Vec<u64>,
    /// The disjoint seed pool reserved for capability measurement.
    pub eval_seed_pool: Vec<u64>,
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
    /// The turn budget ratio was not a positive fraction.
    #[error("grid-world turn budget ratio must be a positive fraction")]
    InvalidTurnBudgetRatio,
    /// The declared ceiling was below what some seed in a declared pool actually needs.
    #[error("grid-world max_turns {declared} is below the {needed} turns seed {seed} needs")]
    TurnCeilingTooLow {
        /// The ceiling the definition declared.
        declared: u32,
        /// The budget the seed requires.
        needed: u32,
        /// The offending seed.
        seed: u64,
    },
    /// A seed pool was empty.
    #[error("grid-world {pool} seed pool must not be empty")]
    EmptySeedPool {
        /// Name of the empty pool.
        pool: &'static str,
    },
    /// The two seed pools overlapped, which would make every measurement contaminated.
    #[error("grid-world seed {seed} appears in both the dev and eval pools")]
    OverlappingSeedPools {
        /// The seed present in both pools.
        seed: u64,
    },
    /// A seed pool repeated a seed, which would silently over-weight it.
    #[error("grid-world {pool} seed pool contains {seed} more than once")]
    DuplicateSeed {
        /// Name of the offending pool.
        pool: &'static str,
        /// The repeated seed.
        seed: u64,
    },
    /// Fewer than the required number of evaluation episodes.
    #[error("grid-world eval seed pool must hold at least {required} seeds, got {actual}")]
    EvalPoolTooSmall {
        /// Minimum number of evaluation seeds.
        required: usize,
        /// Number actually declared.
        actual: usize,
    },
    /// The default seed was not a member of any declared pool.
    #[error("grid-world default_seed {seed} is in neither the dev nor the eval pool")]
    DefaultSeedUnpooled {
        /// The unpooled default seed.
        seed: u64,
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
        if self.turn_budget_denominator == 0 || self.turn_budget_numerator == 0 {
            return Err(BenchmarkDefinitionError::InvalidTurnBudgetRatio);
        }
        for (pool, seeds) in [("dev", &self.dev_seed_pool), ("eval", &self.eval_seed_pool)] {
            if seeds.is_empty() {
                return Err(BenchmarkDefinitionError::EmptySeedPool { pool });
            }
            let mut seen = BTreeMap::new();
            for seed in seeds {
                if seen.insert(*seed, ()).is_some() {
                    return Err(BenchmarkDefinitionError::DuplicateSeed { pool, seed: *seed });
                }
            }
        }
        // The pools must be disjoint, and that is checked here rather than trusted to convention:
        // a single seed shared between them means every "held-out" number has already been tuned
        // against, and no amount of downstream care can undo that.
        for seed in &self.eval_seed_pool {
            if self.dev_seed_pool.contains(seed) {
                return Err(BenchmarkDefinitionError::OverlappingSeedPools { seed: *seed });
            }
        }
        if self.eval_seed_pool.len() < MINIMUM_EVAL_SEEDS {
            return Err(BenchmarkDefinitionError::EvalPoolTooSmall {
                required: MINIMUM_EVAL_SEEDS,
                actual: self.eval_seed_pool.len(),
            });
        }
        if !self.dev_seed_pool.contains(&self.default_seed)
            && !self.eval_seed_pool.contains(&self.default_seed)
        {
            return Err(BenchmarkDefinitionError::DefaultSeedUnpooled {
                seed: self.default_seed,
            });
        }
        // The declared ceiling has to actually bound every seed the benchmark admits, or the
        // catalog would advertise a budget some episode could exceed.
        for seed in self.dev_seed_pool.iter().chain(self.eval_seed_pool.iter()) {
            let needed = self.max_turns_for(*seed);
            if needed > self.max_turns {
                return Err(BenchmarkDefinitionError::TurnCeilingTooLow {
                    declared: self.max_turns,
                    needed,
                    seed: *seed,
                });
            }
        }
        Ok(())
    }

    /// The turn budget one seed's episode runs with.
    ///
    /// Derived from the seed's own shortest hazard-free route, so a harder seed gets a proportionally
    /// larger budget and an easy one is not handed 22 free turns to wander. Deterministic in the
    /// seed, which is what lets the harness plan N episodes in advance.
    pub fn max_turns_for(&self, seed: u64) -> u32 {
        let layout = grid_world_layout(seed);
        let scaled = u64::from(layout.shortest_safe_path()) * u64::from(self.turn_budget_numerator);
        let budget = scaled.div_ceil(u64::from(self.turn_budget_denominator));
        u32::try_from(budget.max(1)).unwrap_or(u32::MAX)
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
    ///
    /// The turn budget is this seed's own, derived from its shortest safe route, rather than the
    /// catalog ceiling. The ceiling exists so a summary can describe the benchmark without knowing
    /// the seed; the episode runs on the tighter number.
    pub fn create_sandbox(&self, seed: u64) -> Result<Box<dyn Sandbox>, BenchmarkDefinitionError> {
        let sandbox = GridWorldSandbox::with_max_turns(seed, self.definition.max_turns_for(seed))?;
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
        grid_world_layout, BenchmarkCatalog, BenchmarkDefinitionError, GridWorldBenchmark,
        GRID_WORLD_BENCHMARK_ID, GRID_WORLD_BENCHMARK_VERSION, GRID_WORLD_CAPABILITY_DIMENSION,
        MINIMUM_EVAL_SEEDS,
    };
    use probelab_core::Benchmark;

    #[test]
    fn built_in_definition_parses_and_validates() {
        let benchmark = GridWorldBenchmark::built_in().expect("built-in benchmark");
        assert_eq!(benchmark.definition().id, GRID_WORLD_BENCHMARK_ID);
        assert_eq!(benchmark.definition().version, GRID_WORLD_BENCHMARK_VERSION);
        assert_eq!(benchmark.definition().max_turns, 12);
        assert_eq!(benchmark.definition().turn_budget_numerator, 3);
        assert_eq!(benchmark.definition().turn_budget_denominator, 2);
        assert_eq!(
            benchmark.definition().capability_dimension,
            GRID_WORLD_CAPABILITY_DIMENSION
        );
        assert_eq!(benchmark.summary().seed, 2000);
    }

    #[test]
    fn catalog_lists_and_resolves_the_single_real_benchmark() {
        let catalog = BenchmarkCatalog::new().expect("built-in catalog");
        assert_eq!(catalog.summaries().len(), 1);
        assert_eq!(catalog.summaries()[0].benchmark_id, "grid-world");
        let resolved = catalog.resolve("grid-world").expect("grid-world resolves");
        assert_eq!(resolved.summary().benchmark_version, "2");
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

    /// The disjointness of the two pools is the whole basis for calling an eval number held out, so
    /// it is asserted rather than assumed. A single shared seed would silently turn every future
    /// measurement into a development measurement, and nothing downstream could detect it.
    #[test]
    fn the_dev_and_eval_seed_pools_are_disjoint_and_large_enough() {
        let definition = GridWorldBenchmark::built_in()
            .expect("built-in benchmark")
            .definition()
            .clone();
        for seed in &definition.dev_seed_pool {
            assert!(
                !definition.eval_seed_pool.contains(seed),
                "seed {seed} is in both pools"
            );
        }
        assert!(
            definition.eval_seed_pool.len() >= MINIMUM_EVAL_SEEDS,
            "eval pool must hold at least {MINIMUM_EVAL_SEEDS} seeds, got {}",
            definition.eval_seed_pool.len()
        );
        assert!(!definition.dev_seed_pool.is_empty());
    }

    /// A definition that admits an overlapping pool must be refused at load time.
    #[test]
    fn an_overlapping_seed_pool_is_rejected() {
        let mut definition = GridWorldBenchmark::built_in()
            .expect("built-in benchmark")
            .definition()
            .clone();
        let shared = definition.eval_seed_pool[0];
        definition.dev_seed_pool.push(shared);
        assert!(matches!(
            GridWorldBenchmark::from_definition(definition),
            Err(BenchmarkDefinitionError::OverlappingSeedPools { seed }) if seed == shared
        ));
    }

    /// An eval pool too small to support a confidence interval is refused rather than measured.
    #[test]
    fn an_undersized_eval_pool_is_rejected() {
        let mut definition = GridWorldBenchmark::built_in()
            .expect("built-in benchmark")
            .definition()
            .clone();
        definition.eval_seed_pool.truncate(4);
        assert!(matches!(
            GridWorldBenchmark::from_definition(definition),
            Err(BenchmarkDefinitionError::EvalPoolTooSmall { .. })
        ));
    }

    /// The catalog ceiling has to actually bound every seed the benchmark admits.
    #[test]
    fn the_declared_ceiling_bounds_every_admitted_seed() {
        let benchmark = GridWorldBenchmark::built_in().expect("built-in benchmark");
        let definition = benchmark.definition();
        for seed in definition
            .dev_seed_pool
            .iter()
            .chain(&definition.eval_seed_pool)
        {
            assert!(
                definition.max_turns_for(*seed) <= definition.max_turns,
                "seed {seed} needs more than the declared ceiling"
            );
        }
    }

    /// The per-seed budget must be derived from the seed, not copied from a constant.
    #[test]
    fn the_turn_budget_varies_by_seed_and_always_admits_the_shortest_route() {
        let benchmark = GridWorldBenchmark::built_in().expect("built-in benchmark");
        let definition = benchmark.definition();
        let mut budgets = std::collections::BTreeSet::new();
        for seed in definition.eval_seed_pool.iter().take(20) {
            let layout = grid_world_layout(*seed);
            let budget = definition.max_turns_for(*seed);
            assert!(
                budget >= layout.shortest_safe_path(),
                "seed {seed} is given {budget} turns for a {}-move route",
                layout.shortest_safe_path()
            );
            budgets.insert(budget);
        }
        assert!(
            budgets.len() > 1,
            "every seed resolved to the same turn budget, so the budget is not seed-derived"
        );
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
