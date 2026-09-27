//! Tests for the authoritative benchmark registry.
//!
//! Written against the properties a user depends on, not against the shape of the code. The one that
//! matters most is the first: every benchmark the repository ships must be reachable through the
//! registry. `sandbox-process` was fully implemented and simply not registered, which made the whole of
//! phase 1's containment work invisible from the product — the exact failure this registry exists to
//! prevent, and the reason a test pins it rather than trusting the registration list.

use probelab_benchmarks::{
    BenchmarkCatalog, BenchmarkCategory, BenchmarkRegistry, BenchmarkSpec, CapabilityExecution,
    PlatformRequirement, ReplaySupport, GRID_WORLD_BENCHMARK_ID, MINIMUM_EVAL_SEEDS,
    SANDBOX_PROCESS_BENCHMARK_ID,
};

#[test]
fn the_registry_is_not_empty() {
    let registry = BenchmarkRegistry::built_in();
    assert!(
        !registry.is_empty(),
        "an empty registry would look identical to a working one from the outside"
    );
    assert!(
        registry.len() >= 2,
        "grid-world and sandbox-process must both be registered"
    );
}

/// Every shipped benchmark is reachable. This is the regression the registry was built for.
#[test]
fn every_shipped_benchmark_is_registered_and_buildable() {
    let registry = BenchmarkRegistry::built_in();
    for id in [GRID_WORLD_BENCHMARK_ID, SANDBOX_PROCESS_BENCHMARK_ID] {
        assert!(
            registry.spec(id).is_some(),
            "{id} is implemented and tested but is not in the registry, so no user can select it"
        );
        let benchmark = registry
            .build(id)
            .unwrap_or_else(|error| panic!("{id} should build: {error}"));
        assert_eq!(
            benchmark.summary().benchmark_id,
            id,
            "a benchmark reported a different id than the registry resolved it under"
        );
    }
}

/// A build failure must be reported, never silently swallowed into a shorter list.
#[test]
fn an_unknown_benchmark_is_refused_rather_than_defaulted() {
    let registry = BenchmarkRegistry::built_in();
    assert!(
        registry.build("no-such-benchmark").is_err(),
        "an unknown id must not resolve to some other benchmark"
    );
    assert!(registry.spec("no-such-benchmark").is_none());
}

/// Identifiers are unique, because a stored result records only the id and the version.
#[test]
fn benchmark_identifiers_are_unique() {
    let registry = BenchmarkRegistry::built_in();
    let ids: Vec<String> = registry.specs().into_iter().map(|spec| spec.id).collect();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "two benchmarks share an id, so a stored result could not say which produced it"
    );
}

/// Every benchmark's metadata is complete enough for the UI to render it without guessing.
#[test]
fn every_spec_carries_the_metadata_the_ui_needs() {
    for spec in BenchmarkRegistry::built_in().specs() {
        assert!(!spec.id.is_empty(), "a spec has no id");
        assert!(!spec.name.is_empty(), "{} has no name", spec.id);
        assert!(
            !spec.description.is_empty(),
            "{} has no description",
            spec.id
        );
        assert!(!spec.version.is_empty(), "{} has no version", spec.id);
        assert!(
            !spec.capability_dimension.is_empty(),
            "{} has no capability dimension",
            spec.id
        );
        assert!(
            !spec.scoring.is_empty(),
            "{} has no scoring description",
            spec.id
        );
        assert!(
            !spec.observation_schema.is_empty(),
            "{} declares no observation schema",
            spec.id
        );
        assert!(
            !spec.action_schema.is_empty(),
            "{} declares no action schema",
            spec.id
        );
        assert!(
            !spec.limits.is_empty(),
            "{} declares no limits, so a user cannot see what bounds it",
            spec.id
        );
        assert!(
            spec.eval_seeds >= MINIMUM_EVAL_SEEDS,
            "{} has {} evaluation seeds, below the project minimum",
            spec.id,
            spec.eval_seeds
        );
        assert!(spec.dev_seeds > 0, "{} has no development seeds", spec.id);
    }
}

/// The capability claims must match reality, or the lab overstates its own sandbox.
///
/// This is the honesty check the master prompt asks for by name. `sandbox-process` is the only
/// benchmark whose declared capability is genuinely executed, and only on Windows; every other
/// capability in the system is authorized and then refused at execution time.
#[test]
fn capability_execution_claims_match_what_is_actually_implemented() {
    let registry = BenchmarkRegistry::built_in();

    let sandbox = registry
        .spec(SANDBOX_PROCESS_BENCHMARK_ID)
        .expect("sandbox-process is registered");
    assert_eq!(
        sandbox.capabilities,
        vec!["process.spawn".to_owned()],
        "sandbox-process grants exactly one capability and it must be reported exactly"
    );
    assert_eq!(
        sandbox.capability_execution,
        CapabilityExecution::Executed,
        "process.spawn has a real executor, so claiming otherwise understates the lab"
    );
    assert_eq!(
        sandbox.platform_requirements,
        PlatformRequirement::Windows,
        "the containment sandbox-process relies on is Windows-only, and the UI must be able to say so \
         before a user's run fails"
    );
    assert!(
        !sandbox.runs_on(PlatformRequirement::Any),
        "a Windows-only benchmark must not claim to run on any platform"
    );
    assert!(
        sandbox.runs_on(PlatformRequirement::Windows),
        "a Windows-only benchmark must run on Windows"
    );

    let grid = registry
        .spec(GRID_WORLD_BENCHMARK_ID)
        .expect("grid-world is registered");
    assert!(
        grid.capabilities.is_empty(),
        "grid-world grants nothing, so listing a capability for it would be a false claim"
    );
    assert_eq!(
        grid.capability_execution,
        CapabilityExecution::NotApplicable,
        "a benchmark that grants nothing should not report deferred execution"
    );
    assert_eq!(grid.platform_requirements, PlatformRequirement::Any);
    assert!(grid.runs_on(PlatformRequirement::Any));
}

/// A benchmark that names a reference policy must actually name one.
///
/// The full cross-layer check — that the name resolves to a registered `ReferenceStrategy` — lives in
/// `crates/engine`, because the agent registry is not visible from this crate and duplicating the
/// policy names here would create a second list to drift. What this crate can honestly assert is that
/// the field is either absent or a real, non-placeholder name, so the cross-layer test has something
/// to resolve.
#[test]
fn a_declared_reference_agent_is_named_rather_than_left_empty() {
    for spec in BenchmarkRegistry::built_in().specs() {
        match spec.reference_agent.as_deref() {
            None => continue,
            Some(name) => assert!(
                !name.trim().is_empty() && name != "TODO" && !name.contains("placeholder"),
                "{} declares a placeholder reference agent {name:?}",
                spec.id
            ),
        }
    }
    let grid = BenchmarkRegistry::built_in();
    assert!(
        grid.spec(GRID_WORLD_BENCHMARK_ID)
            .and_then(|spec| spec.reference_agent.clone())
            .is_some(),
        "grid-world has a shipped reference policy and must say so"
    );
}

/// Replay claims must not overstate what the repository can do.
#[test]
fn replay_claims_are_not_overstated() {
    let registry = BenchmarkRegistry::built_in();
    for spec in registry.specs() {
        assert!(
            matches!(
                spec.replay_support,
                ReplaySupport::Deterministic | ReplaySupport::ProjectedOnly
            ),
            "{} declares an unknown replay mode",
            spec.id
        );
    }
    // Grid-world has a recorded digest that excludes volatile fields; sandbox-process runs a real
    // child whose pid, exit code, and CPU time are inherently volatile, so it is projection only.
    let grid = registry.spec(GRID_WORLD_BENCHMARK_ID).expect("registered");
    assert_eq!(grid.replay_support, ReplaySupport::Deterministic);
    let sandbox = registry
        .spec(SANDBOX_PROCESS_BENCHMARK_ID)
        .expect("registered");
    assert_eq!(
        sandbox.replay_support,
        ReplaySupport::ProjectedOnly,
        "a benchmark that spawns a real process cannot claim deterministic replay"
    );
}

/// Categories are distinct, so the UI can group without a special case per benchmark.
#[test]
fn benchmark_categories_cover_the_registered_benchmarks() {
    let registry = BenchmarkRegistry::built_in();
    let grid = registry.spec(GRID_WORLD_BENCHMARK_ID).expect("registered");
    let sandbox = registry
        .spec(SANDBOX_PROCESS_BENCHMARK_ID)
        .expect("registered");
    assert_eq!(grid.category, BenchmarkCategory::Planning);
    assert_eq!(sandbox.category, BenchmarkCategory::CapabilityGated);
}

/// The registry is the only place a benchmark is named, and it survives serialization.
///
/// The UI consumes specs over IPC, so they must round-trip. A spec that could not serialize would
/// mean the command center silently falls back to a hardcoded list — the duplication this module
/// removed.
#[test]
fn specs_round_trip_through_serialization_for_the_ui() {
    for spec in BenchmarkRegistry::built_in().specs() {
        let json = serde_json::to_string(&spec).expect("a spec must serialize for IPC");
        let restored: BenchmarkSpec = serde_json::from_str(&json).expect("and parse back");
        assert_eq!(spec, restored, "a spec did not survive the IPC round trip");
    }
}

/// The legacy catalog must report the same set as the registry, or the two have drifted.
#[test]
fn the_legacy_catalog_agrees_with_the_registry() {
    let catalog = BenchmarkCatalog::new().expect("catalog builds");
    let registry = catalog.registry();
    assert_eq!(
        catalog.summaries().len(),
        registry.len(),
        "the catalog and the registry disagree about how many benchmarks exist"
    );
    for summary in catalog.summaries() {
        assert!(
            registry.spec(&summary.benchmark_id).is_some(),
            "the catalog reports {} but the registry does not know it",
            summary.benchmark_id
        );
    }
    // And the previously-unreachable benchmark is now reachable through the old surface too.
    assert!(
        catalog.resolve(SANDBOX_PROCESS_BENCHMARK_ID).is_ok(),
        "sandbox-process must resolve through the catalog, not only the registry"
    );
}
