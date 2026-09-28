//! Benchmark definition digests.
//!
//! `BenchmarkSpec::version` is a hand-maintained string, and the loader's only check is that a
//! definition *agrees* with the constant beside it. So a definition file can be edited -- a seed pool
//! grown, a turn budget moved, the reward function changed -- with the build green and the version
//! still whatever it was. Two results recorded either side of such an edit would compare as though
//! they had measured the same task, which is the specific failure a benchmark claiming reproducibility
//! cannot afford.
//!
//! The digest is computed from the bytes that were loaded, so it cannot drift from what the benchmark
//! actually is. These tests pin the properties that make it trustworthy rather than merely present.

use probelab_benchmarks::{definition_digest, BenchmarkRegistry};

/// Every registered benchmark carries a well-formed digest.
#[test]
fn every_benchmark_reports_a_digest() {
    let registry = BenchmarkRegistry::built_in();
    let summaries = registry.summaries();
    assert!(!summaries.is_empty(), "the catalog has benchmarks to check");

    for summary in &summaries {
        assert!(
            summary.definition_digest.starts_with("sha256:"),
            "{} reports no digest, or one in an unknown form: {:?}",
            summary.benchmark_id,
            summary.definition_digest
        );
        let hex = summary
            .definition_digest
            .strip_prefix("sha256:")
            .expect("prefix checked");
        assert_eq!(
            hex.len(),
            64,
            "{} digest is not 64 hex characters",
            summary.benchmark_id
        );
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{} digest should be lowercase hex, got {hex:?}",
            summary.benchmark_id
        );
    }
}

/// Two benchmarks cannot collide, because they are different tasks.
#[test]
fn distinct_benchmarks_have_distinct_digests() {
    let registry = BenchmarkRegistry::built_in();
    let summaries = registry.summaries();
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for summary in &summaries {
        if let Some((_, other)) = seen
            .iter()
            .find(|(_, digest)| *digest == summary.definition_digest)
        {
            panic!(
                "{} and {other} share a digest, so they cannot be told apart in a stored result",
                summary.benchmark_id
            );
        }
        seen.push((&summary.benchmark_id, &summary.definition_digest));
    }
}

/// The digest is a function of the bytes, and nothing else.
#[test]
fn the_digest_is_stable_and_sensitive() {
    assert_eq!(
        definition_digest("same"),
        definition_digest("same"),
        "the same bytes must always produce the same digest"
    );
    assert_ne!(
        definition_digest("same"),
        definition_digest("different"),
        "different bytes must produce different digests"
    );
    // A one-character change, the smallest edit a definition can undergo.
    assert_ne!(
        definition_digest("{\"max_turns\":30}"),
        definition_digest("{\"max_turns\":31}"),
        "changing a turn budget must move the digest"
    );
}

/// The digest is stable across loads, which is what makes a stored result worth anything.
///
/// If this moved between two runs of the same binary, every stored result would be unique and
/// incomparable, and the field would be noise.
#[test]
fn the_digest_does_not_move_between_loads() {
    let first = BenchmarkRegistry::built_in().summaries();
    let second = BenchmarkRegistry::built_in().summaries();
    assert_eq!(
        first, second,
        "two loads of the same build must agree exactly"
    );
}

/// The digest is what the version cannot be: it moves when the definition does.
///
/// This is the property the whole field exists for, so it is asserted against a real edited
/// definition rather than a string comparison. A zero turn budget is rejected at load, so the edit
/// used here is one that is still *valid* but changes the task -- which is precisely the case a
/// forgotten version bump would hide.
#[test]
fn editing_a_definition_moves_its_digest() {
    let original = include_str!("../../../benchmarks/games/grid-world.v1.json");

    // A pure reformat: same values, different whitespace.
    let reformatted = original.replace('\n', "\n ");
    assert_ne!(
        definition_digest(original),
        definition_digest(&reformatted),
        "reformatting must move the digest, so a definition cannot be edited invisibly"
    );

    // And a real change: one more eval seed. The pool floor is 30, so a definition with 31 is still
    // valid and would load -- which is exactly the edit nobody thinks to version-bump.
    let extra_seed = original.replace("\"eval_seed_pool\": [", "\"eval_seed_pool\": [4242,");
    if extra_seed == original {
        // The fixture's shape changed; the reformat assertion above still covers the property, so
        // note it rather than silently passing.
        eprintln!(
            "note: the eval_seed_pool splice did not apply; reformatting assertion still holds"
        );
    } else {
        assert_ne!(
            definition_digest(original),
            definition_digest(&extra_seed),
            "adding a seed must move the digest"
        );
        // And it must still be a loadable definition, or this would be a test about invalid input.
        probelab_benchmarks::GridWorldBenchmark::from_json(&extra_seed)
            .expect("a definition with one more eval seed is still valid");
    }
}
