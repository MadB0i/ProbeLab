//! Embedded versioned benchmark definitions and a small built-in registry.

#![deny(missing_docs)]

use std::collections::BTreeMap;

use probelab_core::{Benchmark, BenchmarkSummary, ProcessSpawnPlan, Sandbox};
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
    /// Deserializes without validating, for a definition whose identity is not the grid world's.
    ///
    /// [`GridWorldDefinition::from_json`] validates against the grid-world identity as part of
    /// parsing, so it cannot load a second benchmark that shares this environment. The
    /// deserialization is identical; only the identity check moves out, to
    /// [`GridWorldDefinition::validate_as`], which a caller must then call.
    pub fn parse_unvalidated(json: &str) -> Result<Self, BenchmarkDefinitionError> {
        Ok(serde_json::from_str(json)?)
    }

    /// Parses and validates a serialized grid-world definition.
    pub fn from_json(json: &str) -> Result<Self, BenchmarkDefinitionError> {
        let definition: Self = serde_json::from_str(json)?;
        definition.validate()?;
        Ok(definition)
    }

    /// Validates all identity, version, turn-limit, and capability requirements.
    pub fn validate(&self) -> Result<(), BenchmarkDefinitionError> {
        self.validate_as(
            GRID_WORLD_BENCHMARK_ID,
            GRID_WORLD_BENCHMARK_VERSION,
            GRID_WORLD_CAPABILITY_DIMENSION,
        )
    }

    /// Validates the same rules against an explicitly supplied identity.
    ///
    /// The identity triple is a parameter rather than a hard-coded constant because a second
    /// benchmark ships against the same environment generator and must be held to the same seed-pool
    /// and turn-ceiling rules. What differs between benchmarks is *which* identity is correct, and
    /// hard-coding one set of constants would mean the second benchmark either skipped validation or
    /// was validated as if it were the grid world.
    pub fn validate_as(
        &self,
        expected_id: &'static str,
        expected_version: &'static str,
        expected_dimension: &'static str,
    ) -> Result<(), BenchmarkDefinitionError> {
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
        if self.id != expected_id {
            return Err(BenchmarkDefinitionError::UnexpectedId {
                expected: expected_id,
                actual: self.id.clone(),
            });
        }
        if self.version != expected_version {
            return Err(BenchmarkDefinitionError::UnexpectedVersion {
                expected: expected_version,
                actual: self.version.clone(),
            });
        }
        if self.max_turns == 0 {
            return Err(BenchmarkDefinitionError::ZeroMaxTurns);
        }
        if self.capability_dimension != expected_dimension {
            return Err(BenchmarkDefinitionError::CapabilityDimensionMismatch {
                expected: expected_dimension,
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
    /// The seeds this benchmark was developed against.
    ///
    /// Exposed so a measurement can *prove* it drew only from held-out seeds, rather than asserting
    /// the two pools happen not to overlap. That proof is what `eval_seeds` alone cannot give.
    pub fn dev_seeds(&self) -> Vec<u64> {
        self.definition.dev_seed_pool.clone()
    }

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

    /// The benchmark's own declared evaluation pool, disjoint from the seeds it was developed against.
    ///
    /// Overriding this is what makes "measured on held-out seeds" a property of the benchmark rather
    /// than a claim in a document. A measurement that drew from the derived default could land on a
    /// seed the turn budget or the map layout was tuned against.
    fn eval_seeds(&self) -> Vec<u64> {
        self.definition.eval_seed_pool.clone()
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

// ---------------------------------------------------------------------------------------------
// The sandbox-process benchmark.
//
// Same environment generator as the grid world, different capability contract and a different
// success rule. It exists because the containment stack was unreachable from any real model: a
// benchmark could declare a spawn plan and grant a program, and no provider could ever ask for it,
// because the only way to request one was a `CapabilityProbe` no prompt documents. This benchmark is
// the first shipped thing a real model can drive into containment.
// ---------------------------------------------------------------------------------------------

/// Stable identifier of the sandbox-process benchmark.
pub const SANDBOX_PROCESS_BENCHMARK_ID: &str = "sandbox-process";
/// Version of the sandbox-process benchmark definition.
pub const SANDBOX_PROCESS_BENCHMARK_VERSION: &str = "1";
/// The capability dimension the sandbox-process benchmark measures.
pub const SANDBOX_PROCESS_CAPABILITY_DIMENSION: &str = "capability-gated task completion";

/// The line the contained child prints, and which the evaluator requires.
///
/// Fixed rather than per-seed on purpose. The declared plan is trusted configuration and the
/// `Benchmark` trait's spawn-plan accessor takes no seed, so a seed-varying command line is not
/// expressible today. That is a real limitation and it is why this benchmark does **not** claim to
/// measure coding: the child does the same thing every episode, and what varies per seed is the
/// model's path to the goal and whether it spends a turn asking for the capability at all. A
/// seed-varying child needs a seed-aware plan accessor, which is Phase 4 work.
pub const SANDBOX_PROCESS_EXPECTED_OUTPUT: &str = "probelab-contained-child-ok";

/// The embedded sandbox-process definition.
pub const SANDBOX_PROCESS_DEFINITION_JSON: &str =
    include_str!("../../../benchmarks/games/sandbox-process.v1.json");

/// A grid-world task that cannot be completed without the contained process it offers.
#[derive(Clone, Debug)]
pub struct SandboxProcessBenchmark {
    definition: GridWorldDefinition,
    program: String,
    /// Held for its `Drop`, which removes the directory, and as the single source of truth for
    /// which directory is the answer key.
    answer_key: AnswerKey,
}

/// The material a contained child must not be able to read, in a directory the containment layer can
/// actually protect.
///
/// **Why this is a directory ProbeLab creates rather than the workspace source tree.** The obvious
/// choice is to protect `benchmarks/games/sandbox-process.v1.json` and `crates/evaluator`, where the
/// answer lives in source form. Measured on this host, that fails closed: writing a mandatory label
/// needs `WRITE_OWNER`, and the workspace grants the current user only `Modify`, which is
/// read/write/delete but neither `WRITE_OWNER` nor `WRITE_DAC`. Under `%TEMP%`, which the process
/// created, it holds `Full control` and the protection applies cleanly.
///
/// Refusing was the correct behaviour and is left in place — a run that cannot protect its answer key
/// does not start a child that could read it. What changed is *what* the benchmark asks to protect, so
/// that the request is satisfiable without weakening the boundary: the key is materialised as a file
/// in a directory this process owns, and that file is what gets labelled. The evaluator still does not
/// read it — the expected output is a compiled-in constant — so it is defence in depth against a
/// future plan that might.
#[derive(Clone, Debug)]
struct AnswerKey {
    directory: std::path::PathBuf,
    /// The same path, as the slice `protected_paths` returns.
    ///
    /// Stored rather than rebuilt on each call so there is exactly one source of truth for which
    /// directory is the answer key. An earlier version cached the list on the benchmark with a
    /// `OnceLock` and held the directory separately, which meant the two could disagree.
    paths: Vec<std::path::PathBuf>,
}

impl AnswerKey {
    /// Creates the key directory and writes the expected output into it.
    ///
    /// Unique per instance, not per process. Protection is applied and then *restored* around every
    /// spawn, so two instances sharing one directory would restore the label out from under each
    /// other and make verification fail intermittently. That is not hypothetical: a process-wide
    /// cache here produced exactly that, and the symptom looked like a flaky protection bug rather
    /// than the aliasing it really was.
    fn create() -> Result<Self, BenchmarkDefinitionError> {
        use std::io::Write;
        use std::sync::atomic::{AtomicU64, Ordering};

        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "probelab-sandbox-process-key-{}-{sequence}",
            std::process::id()
        ));
        let io =
            |error: std::io::Error| BenchmarkDefinitionError::Json(serde_json::Error::io(error));
        std::fs::create_dir_all(&directory).map_err(io)?;
        let mut file = std::fs::File::create(directory.join("expected-output.txt")).map_err(io)?;
        file.write_all(SANDBOX_PROCESS_EXPECTED_OUTPUT.as_bytes())
            .map_err(io)?;
        Ok(Self {
            paths: vec![directory.clone()],
            directory,
        })
    }

    /// Returns the paths to protect.
    fn paths(&self) -> &[std::path::PathBuf] {
        &self.paths
    }
}

impl Drop for AnswerKey {
    /// Removes the key directory once no spawn can still be holding its protection.
    ///
    /// Ordering matters: the protection guard restores the original label when it drops, and that
    /// guard belongs to the spawn, not to the benchmark. A benchmark outlives the spawns that used
    /// it, so by the time this runs every one of them has finished and restored.
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl SandboxProcessBenchmark {
    /// Loads the embedded definition and validates it against its own identity.
    pub fn built_in() -> Result<Self, BenchmarkDefinitionError> {
        Self::from_json(SANDBOX_PROCESS_DEFINITION_JSON)
    }

    /// Parses and validates a serialized definition against this benchmark's own identity.
    ///
    /// Goes through [`GridWorldDefinition::parse_unvalidated`] rather than `from_json`, because the
    /// latter validates against the *grid world's* identity as part of parsing and would reject this
    /// definition before the correct check ran.
    pub fn from_json(json: &str) -> Result<Self, BenchmarkDefinitionError> {
        Self::from_definition(GridWorldDefinition::parse_unvalidated(json)?)
    }

    /// Validates and owns a definition.
    pub fn from_definition(
        definition: GridWorldDefinition,
    ) -> Result<Self, BenchmarkDefinitionError> {
        definition.validate_as(
            SANDBOX_PROCESS_BENCHMARK_ID,
            SANDBOX_PROCESS_BENCHMARK_VERSION,
            SANDBOX_PROCESS_CAPABILITY_DIMENSION,
        )?;
        let answer_key = AnswerKey::create()?;
        Ok(Self {
            definition,
            program: declared_child_program(),
            answer_key,
        })
    }

    /// Returns the validated embedded definition.
    pub fn definition(&self) -> &GridWorldDefinition {
        &self.definition
    }

    /// Returns the absolute program the contained child runs.
    pub fn program(&self) -> &str {
        &self.program
    }

    /// Returns the answer-key directory this benchmark protects from its contained child.
    pub fn answer_key_path(&self) -> &std::path::Path {
        &self.answer_key.directory
    }

    /// Creates the per-seed grid world, with the same budget rule as the grid-world benchmark.
    pub fn create_sandbox(&self, seed: u64) -> Result<Box<dyn Sandbox>, BenchmarkDefinitionError> {
        let sandbox = GridWorldSandbox::with_max_turns(seed, self.definition.max_turns_for(seed))?;
        Ok(Box::new(sandbox))
    }
}

/// Resolves the program the child runs, refusing rather than guessing.
///
/// A spawn plan has to name an absolute path: the broker's allowlist compares canonicalized paths, and
/// a relative or bare name could resolve differently inside a contained child than it does here. If
/// `%ComSpec%` is missing there is no safe default to invent, so this reports the platform instead.
fn declared_child_program() -> String {
    std::env::var("ComSpec").unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".to_owned())
}

impl Benchmark for SandboxProcessBenchmark {
    fn summary(&self) -> BenchmarkSummary {
        self.definition.summary()
    }

    /// The benchmark's own declared evaluation pool, disjoint from its development seeds.
    fn eval_seeds(&self) -> Vec<u64> {
        self.definition.eval_seed_pool.clone()
    }

    /// Grants exactly one program: the declared child, by absolute path.
    ///
    /// The grant and the plan name the same string on purpose. The broker compares the *plan's*
    /// program against this allowlist, so a mismatch would refuse every request — which is the
    /// correct failure, but a silent one that looks like a model that cannot use the capability.
    fn capability_policy(&self) -> probelab_core::CapabilityPolicy {
        let mut policy = probelab_core::CapabilityPolicy::deny_all();
        // A deny-all policy that somehow rejects this grant would leave the benchmark advertising a
        // capability it cannot deliver. The definition is validated at load, so treat a failure here
        // as deny-all rather than panicking in the middle of a run.
        if policy.grant_process(&self.program).is_err() {
            return probelab_core::CapabilityPolicy::deny_all();
        }
        policy
    }

    /// Declares the single process this benchmark will have run on a model's behalf.
    ///
    /// The arguments are compiled-in trusted configuration. A model asking for the process cannot
    /// change the program, these arguments, the working directory, or the environment, and the broker
    /// re-checks the program against the allowlist before anything starts.
    fn process_spawn_plan(&self) -> Option<ProcessSpawnPlan> {
        Some(
            ProcessSpawnPlan::new(&self.program)
                .with_args([
                    "/D",
                    "/C",
                    // `echo` with no redirection keeps the child's only side effect inside its own
                    // designated directory: the artifact the evaluator checks is the captured stream,
                    // which containment records itself.
                    &format!("echo {SANDBOX_PROCESS_EXPECTED_OUTPUT}"),
                ])
                .with_timeout(std::time::Duration::from_secs(30)),
        )
    }

    /// Declares the answer key as unreadable to the contained child.
    ///
    /// Unlike the grid world, this is not precautionary: a child really does run here. Containment
    /// refuses an empty protected set, so a spawning benchmark cannot skip this, and it refuses a
    /// protection it cannot apply rather than starting a child that could read the key.
    ///
    /// Per-instance rather than cached process-wide, because the protection is applied and restored
    /// around every spawn and two instances sharing one path would restore each other's label.
    fn protected_paths(&self) -> &[std::path::PathBuf] {
        self.answer_key.paths()
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
///
/// Now a thin adapter over [`BenchmarkRegistry`], which is the authoritative description. This type is
/// kept because the engine and the desktop IPC already speak `summaries()` and `resolve(id)`, and
/// changing that surface would be churn rather than improvement. It deliberately holds *no* benchmark
/// list of its own — that is exactly the duplication that left `sandbox-process` unreachable.
#[derive(Debug)]
pub struct BenchmarkCatalog {
    registry: BenchmarkRegistry,
}

impl BenchmarkCatalog {
    /// Builds the catalog from validated, statically registered definitions.
    pub fn new() -> Result<Self, BenchmarkCatalogError> {
        Ok(Self {
            registry: BenchmarkRegistry::built_in(),
        })
    }

    /// The authoritative registry, for callers that need the full metadata.
    pub fn registry(&self) -> &BenchmarkRegistry {
        &self.registry
    }

    /// Lists summaries for every statically registered benchmark.
    pub fn summaries(&self) -> Vec<BenchmarkSummary> {
        self.registry.summaries()
    }

    /// Resolves one built-in benchmark by its stable identifier.
    pub fn resolve(&self, id: &str) -> Result<Box<dyn Benchmark>, BenchmarkCatalogError> {
        self.registry.build(id)
    }
}

mod registry;

pub use registry::{
    BenchmarkCategory, BenchmarkEntry, BenchmarkRegistrationError, BenchmarkRegistry,
    BenchmarkSpec, CapabilityExecution, PlatformRequirement, ReplaySupport,
};

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
    fn catalog_lists_and_resolves_every_registered_benchmark() {
        // This used to assert exactly one benchmark and be named for it. That was not a requirement —
        // it was a snapshot of the state in which `sandbox-process` had been implemented, tested, and
        // then never registered, which left phase 1's containment work unreachable from the product.
        // Asserting the count pinned the defect. The assertions below are the ones that were actually
        // meant: every registered benchmark resolves, and an unknown one does not.
        let catalog = BenchmarkCatalog::new().expect("built-in catalog");
        let summaries = catalog.summaries();
        assert!(
            summaries.len() >= 2,
            "grid-world and sandbox-process must both be listed, got {}",
            summaries.len()
        );
        for summary in &summaries {
            let resolved = catalog
                .resolve(&summary.benchmark_id)
                .unwrap_or_else(|error| panic!("{} should resolve: {error}", summary.benchmark_id));
            assert_eq!(
                resolved.summary().benchmark_version,
                summary.benchmark_version
            );
        }
        let grid = catalog.resolve("grid-world").expect("grid-world resolves");
        assert_eq!(grid.summary().benchmark_version, "2");
        assert!(
            catalog.resolve("sandbox-process").is_ok(),
            "sandbox-process must be reachable, or its capability grant is dead code"
        );
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
