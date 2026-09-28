//! The authoritative benchmark registry.
//!
//! # Why this exists
//!
//! Before this, `BenchmarkCatalog` knew about exactly one benchmark, the UI rendered a hardcoded list
//! from it, and `sandbox-process` — a shipped, fully tested benchmark, and the only one that grants a
//! capability — was never registered at all. That made the whole of phase 1's containment work
//! unreachable from the product, and it is the exact failure the master prompt names: a benchmark that
//! exists in the repository but cannot be selected by a user.
//!
//! This module is the single place a benchmark is described. The UI renders from it, the experiment
//! manager resolves through it, and adding a benchmark is one `register` call rather than an edit in
//! Rust *and* an edit in React.
//!
//! # What the metadata is for
//!
//! Each field is there because a user or the Lab Brain needs it to make a decision, not to fill out a
//! form. Two are load-bearing for honesty rather than presentation:
//!
//! * [`BenchmarkSpec::capability_execution`] distinguishes *authorized* from *executed*. A benchmark
//!   that offers `filesystem.read` does not thereby have a reader, and a UI that showed a green tick
//!   for it would be lying about the lab's own capability surface.
//! * [`BenchmarkSpec::platform_requirements`] is why the app can say "this benchmark needs Windows"
//!   instead of discovering it when a spawn fails on the user's machine.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use probelab_core::{BenchmarkSummary, ProcessSpawnPlan};

use crate::{
    grid_world_definition_digest, sandbox_process_definition_digest, Benchmark,
    BenchmarkCatalogError, GridWorldBenchmark, SandboxProcessBenchmark, GRID_WORLD_BENCHMARK_ID,
    GRID_WORLD_BENCHMARK_VERSION, SANDBOX_PROCESS_BENCHMARK_ID, SANDBOX_PROCESS_BENCHMARK_VERSION,
};

/// How completely a benchmark's declared capabilities are implemented.
///
/// The distinction is the whole point. The broker can *authorize* four permissions it cannot
/// *execute*, and a lab that reported those as working would be overstating its own sandbox.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityExecution {
    /// Every declared capability has a real executor, real enforcement, and tests.
    Executed,
    /// The capability is authorized by the broker and refused at execution time, which stops the run
    /// explicitly. Reported as `AUTHORIZED / EXECUTION DEFERRED` rather than as a working feature.
    AuthorizedDeferred,
    /// The benchmark grants nothing beyond ordinary grid movement.
    NotApplicable,
}

/// Whether a benchmark can be replayed, and in which sense.
///
/// Two different claims get two different labels. "Deterministic" means a re-run reproduces the
/// digest; "projected" means only the recorded view can be inspected. Conflating them would let
/// someone trust a projection as if it were a re-execution.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplaySupport {
    /// A replay digest is recorded and excludes volatile fields, so a re-run can be compared.
    Deterministic,
    /// Only the recorded event stream can be inspected; no re-execution is claimed.
    ProjectedOnly,
}

/// Platforms a benchmark can run on.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformRequirement {
    /// Any platform the engine supports.
    Any,
    /// Windows only, because the containment it declares is Windows-only.
    Windows,
}

/// A benchmark's family, for UI grouping.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkCategory {
    /// Solo planning on a partially observable grid.
    Planning,
    /// Task completion gated on a declared capability.
    CapabilityGated,
    /// Head-to-head play against a reference policy.
    Arena,
}

/// The complete, serializable description of one registered benchmark.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BenchmarkSpec {
    /// Stable identifier, unique in the registry.
    pub id: String,
    /// Definition version. A version bump invalidates comparability with older results.
    pub version: String,
    /// A content digest of the definition's bytes, computed rather than maintained.
    ///
    /// `version` is hand-maintained, and the loader only checks that a definition *agrees* with it --
    /// so the definition can be edited (a seed pool grown, a turn budget moved, the reward function
    /// changed) with the build still green and the string still whatever it was. Results recorded
    /// either side of such an edit would then compare as though they had measured the same task.
    ///
    /// The digest cannot drift from what was actually loaded, which makes it the identifier a
    /// comparison can trust. See `probelab_benchmarks::definition_digest`.
    pub definition_digest: String,
    /// Human-readable name.
    pub name: String,
    /// One-paragraph description of what the benchmark measures.
    pub description: String,
    /// Family, used for UI grouping.
    pub category: BenchmarkCategory,
    /// The single capability dimension measured.
    pub capability_dimension: String,
    /// Capability permissions the benchmark declares.
    pub capabilities: Vec<String>,
    /// How completely those capabilities are implemented.
    pub capability_execution: CapabilityExecution,
    /// Names of the fields the model can observe.
    pub observation_schema: Vec<String>,
    /// Names of the actions the model may emit.
    pub action_schema: Vec<String>,
    /// Human-readable limit summary.
    pub limits: BTreeMap<String, String>,
    /// How an episode is scored, in words rather than a number.
    pub scoring: String,
    /// The reference policy that establishes this benchmark's ceiling, if it has one.
    pub reference_agent: Option<String>,
    /// What replay means for this benchmark.
    pub replay_support: ReplaySupport,
    /// Platforms this benchmark can run on.
    pub platform_requirements: PlatformRequirement,
    /// Evaluation seed count.
    pub eval_seeds: usize,
    /// Development seed count, disjoint from the evaluation pool.
    pub dev_seeds: usize,
}

impl BenchmarkSpec {
    /// Whether this benchmark can run on the current platform, given what the caller supports.
    ///
    /// A `target` argument rather than a compile-time check, because the same binary runs on
    /// platforms the *user* may be on while the *lab host* differs in a hosted deployment.
    pub fn runs_on(&self, target: PlatformRequirement) -> bool {
        self.platform_requirements == PlatformRequirement::Any
            || target == self.platform_requirements
    }
}

/// A registered benchmark: its public description plus a way to build a fresh instance.
///
/// The factory is what makes registration the *only* place a benchmark is named. Resolving by id
/// elsewhere would reintroduce the scattered lists this module replaces.
pub struct BenchmarkEntry {
    /// The public description the UI renders.
    pub spec: BenchmarkSpec,
    factory: Box<dyn Fn() -> Result<Box<dyn Benchmark>, BenchmarkCatalogError> + Send + Sync>,
}

impl std::fmt::Debug for BenchmarkEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BenchmarkEntry")
            .field("spec", &self.spec)
            .finish_non_exhaustive()
    }
}

impl BenchmarkEntry {
    /// Builds a fresh instance of the benchmark.
    pub fn build(&self) -> Result<Box<dyn Benchmark>, BenchmarkCatalogError> {
        (self.factory)()
    }
}

/// Every benchmark ProbeLab can run, keyed by stable identifier.
#[derive(Debug)]
pub struct BenchmarkRegistry {
    entries: BTreeMap<String, BenchmarkEntry>,
}

impl Default for BenchmarkRegistry {
    fn default() -> Self {
        Self::built_in()
    }
}

impl BenchmarkRegistry {
    /// Builds the registry from every benchmark compiled into this binary.
    pub fn built_in() -> Self {
        let mut registry = Self {
            entries: BTreeMap::new(),
        };
        // A registration failure means a shipped benchmark's own definition is invalid, which is a
        // build-time defect rather than a runtime condition. Panicking here is deliberate: shipping a
        // lab that silently omits a benchmark it claims to have would be the exact dishonesty this
        // project has refused everywhere else.
        registry
            .register(grid_world_entry())
            .expect("the built-in grid-world definition must be valid");
        registry
            .register(sandbox_process_entry())
            .expect("the built-in sandbox-process definition must be valid");
        registry
    }

    /// Adds one benchmark.
    ///
    /// Panics on a duplicate identifier, because two benchmarks sharing an id would make every
    /// recorded result ambiguous: the stored `benchmark_id` could not say which definition produced
    /// it.
    pub fn register(&mut self, entry: BenchmarkEntry) -> Result<(), BenchmarkRegistrationError> {
        if self.entries.contains_key(&entry.spec.id) {
            return Err(BenchmarkRegistrationError::DuplicateId {
                id: entry.spec.id.clone(),
            });
        }
        self.entries.insert(entry.spec.id.clone(), entry);
        Ok(())
    }

    /// Every registered benchmark's description, in stable identifier order.
    pub fn specs(&self) -> Vec<BenchmarkSpec> {
        self.entries
            .values()
            .map(|entry| entry.spec.clone())
            .collect()
    }

    /// Looks up one description by identifier.
    pub fn spec(&self, id: &str) -> Option<&BenchmarkSpec> {
        self.entries.get(id).map(|entry| &entry.spec)
    }

    /// Builds a fresh instance of one registered benchmark.
    pub fn build(&self, id: &str) -> Result<Box<dyn Benchmark>, BenchmarkCatalogError> {
        self.entries
            .get(id)
            .ok_or_else(|| BenchmarkCatalogError::UnknownBenchmark { id: id.to_owned() })?
            .build()
    }

    /// The summary the engine and UI already use, for every registered benchmark.
    pub fn summaries(&self) -> Vec<BenchmarkSummary> {
        self.entries
            .values()
            .filter_map(|entry| entry.build().ok())
            .map(|benchmark| benchmark.summary())
            .collect()
    }

    /// How many benchmarks are registered.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry is empty. Always false in practice, and checked anyway so an
    /// accidentally-empty registry cannot look like a working one.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A registration failure.
#[derive(Debug, thiserror::Error)]
pub enum BenchmarkRegistrationError {
    /// Two benchmarks claimed the same identifier.
    #[error("benchmark id {id:?} is already registered")]
    DuplicateId {
        /// The clashing identifier.
        id: String,
    },
    /// A benchmark's own definition was invalid.
    #[error(transparent)]
    Catalog(#[from] BenchmarkCatalogError),
}

fn grid_world_entry() -> BenchmarkEntry {
    let benchmark = GridWorldBenchmark::built_in().expect("built-in grid-world definition");
    let definition = benchmark.definition().clone();
    // Asserted rather than ignored: the metadata below claims this benchmark grants no capability, so
    // a spawn plan appearing here would mean the published spec and the real benchmark disagree.
    debug_assert!(
        benchmark.process_spawn_plan().is_none(),
        "grid-world is published as granting nothing, so it must declare no spawn plan"
    );
    debug_assert!(
        benchmark.capability_policy().is_deny_all(),
        "grid-world is published as granting nothing, so its policy must be deny-all"
    );
    BenchmarkEntry {
        spec: BenchmarkSpec {
            id: GRID_WORLD_BENCHMARK_ID.to_owned(),
            version: GRID_WORLD_BENCHMARK_VERSION.to_owned(),
            definition_digest: grid_world_definition_digest(),
            name: "Grid World".to_owned(),
            description: definition.description.clone(),
            category: BenchmarkCategory::Planning,
            capability_dimension: definition.capability_dimension.clone(),
            capabilities: Vec::new(),
            capability_execution: CapabilityExecution::NotApplicable,
            observation_schema: [
                "turn",
                "position",
                "grid_size",
                "current_tile",
                "visible_neighbors",
                "distance_to_goal",
                "max_turns",
                "remaining_turns",
                "available_actions",
                "history",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            action_schema: ["MOVE_UP", "MOVE_DOWN", "MOVE_LEFT", "MOVE_RIGHT", "WAIT"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            limits: BTreeMap::from([
                (
                    "turn_budget".to_owned(),
                    format!(
                        "ceil({}/{} * this seed's shortest hazard-free route)",
                        definition.turn_budget_numerator, definition.turn_budget_denominator
                    ),
                ),
                (
                    "catalog_ceiling".to_owned(),
                    definition.max_turns.to_string(),
                ),
                ("grid".to_owned(), "5x5".to_owned()),
            ]),
            scoring: "Success on reaching the per-seed reward tile. Reward is shaped but only the \
                      terminal status is scored; partial reward never counts as success."
                .to_owned(),
            reference_agent: Some("grid_world_reference".to_owned()),
            replay_support: ReplaySupport::Deterministic,
            platform_requirements: PlatformRequirement::Any,
            eval_seeds: definition.eval_seed_pool.len(),
            dev_seeds: definition.dev_seed_pool.len(),
        },
        factory: Box::new(|| {
            let benchmark = GridWorldBenchmark::built_in()?;
            Ok(Box::new(benchmark) as Box<dyn Benchmark>)
        }),
    }
}

fn sandbox_process_entry() -> BenchmarkEntry {
    let benchmark =
        SandboxProcessBenchmark::built_in().expect("built-in sandbox-process definition");
    let definition = benchmark.definition().clone();
    let spawn_plan: Option<ProcessSpawnPlan> = benchmark.process_spawn_plan();
    let program = benchmark.program().to_owned();
    // Recorded so a mismatch between what the metadata claims and what the benchmark actually grants
    // is a visible discrepancy rather than a documentation bug.
    debug_assert!(
        spawn_plan.is_some(),
        "sandbox-process must declare a spawn plan"
    );
    BenchmarkEntry {
        spec: BenchmarkSpec {
            id: SANDBOX_PROCESS_BENCHMARK_ID.to_owned(),
            version: SANDBOX_PROCESS_BENCHMARK_VERSION.to_owned(),
            definition_digest: sandbox_process_definition_digest(),
            name: "Sandbox Process".to_owned(),
            description: definition.description.clone(),
            category: BenchmarkCategory::CapabilityGated,
            capability_dimension: definition.capability_dimension.clone(),
            capabilities: vec!["process.spawn".to_owned()],
            // The one benchmark whose declared capability is genuinely executed, on the one platform
            // where the containment exists.
            capability_execution: CapabilityExecution::Executed,
            observation_schema: [
                "turn",
                "position",
                "grid_size",
                "current_tile",
                "visible_neighbors",
                "distance_to_goal",
                "max_turns",
                "remaining_turns",
                "available_actions",
                "history",
                "process_spawn_available",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            action_schema: [
                "MOVE_UP",
                "MOVE_DOWN",
                "MOVE_LEFT",
                "MOVE_RIGHT",
                "WAIT",
                "REQUEST_PROCESS",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            limits: BTreeMap::from([
                (
                    "turn_budget".to_owned(),
                    format!(
                        "ceil({}/{} * shortest safe route); the request itself costs one turn",
                        definition.turn_budget_numerator, definition.turn_budget_denominator
                    ),
                ),
                ("catalog_ceiling".to_owned(), definition.max_turns.to_string()),
                (
                    "spawn_timeout_seconds".to_owned(),
                    spawn_plan
                        .as_ref()
                        .and_then(|plan| plan.timeout)
                        .map(|timeout| timeout.as_secs())
                        .unwrap_or_default()
                        .to_string(),
                ),
            ]),
            scoring: "Success requires BOTH an authorized contained child that ran to completion AND \
                      the per-seed goal reached. An episode that reaches the goal without the child \
                      is a failure; the terminal status still reports what actually ended the episode."
                .to_owned(),
            reference_agent: Some("sandbox_process_reference".to_owned()),
            replay_support: ReplaySupport::ProjectedOnly,
            platform_requirements: PlatformRequirement::Windows,
            eval_seeds: definition.eval_seed_pool.len(),
            dev_seeds: definition.dev_seed_pool.len(),
        },
        factory: Box::new(move || {
            // The program is resolved per build so a benchmark constructed after a `%ComSpec%` change
            // still names the same program its policy allowlists.
            let benchmark = SandboxProcessBenchmark::built_in()?;
            debug_assert_eq!(benchmark.program(), program);
            Ok(Box::new(benchmark) as Box<dyn Benchmark>)
        }),
    }
}
