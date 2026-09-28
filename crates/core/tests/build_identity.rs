//! The build identity a stored result carries.
//!
//! `ReproducibilityMetadata` records the subject of a run — provider, model, prompt version,
//! environment version. These tests cover the other half: the *instrument*. A result that does not say
//! which build produced it cannot be attributed to a change, and two results from the same benchmark
//! and seed are indistinguishable even when the code between them was rewritten.

use probelab_core::{ReproducibilityMetadata, BUILD_COMMIT, PROBELAB_VERSION};

/// The version is always known, because it is read from the manifest at compile time.
///
/// A build script that fails, a vendored tree, a published crate — none of those stop this. There is
/// no git involved and no environment to miss, so an empty version would mean the crate was built in
/// a way that does not exist.
#[test]
fn the_version_is_always_recorded() {
    assert!(!PROBELAB_VERSION.is_empty());
    // Reads as a version rather than a placeholder.
    assert!(
        PROBELAB_VERSION
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.'),
        "the version should look like a version, got {PROBELAB_VERSION:?}"
    );
}

/// A commit is either a real short hash or an explicit `unknown`. Never something plausible and wrong.
#[test]
fn the_commit_is_a_hash_or_an_explicit_unknown() {
    if BUILD_COMMIT == "unknown" {
        return;
    }
    // Either `abc1234` or `abc1234-dirty`. The dirty suffix is the build script telling the truth
    // about a tree that did not match the commit it names.
    let (hash, dirty) = match BUILD_COMMIT.split_once('-') {
        Some((hash, "dirty")) => (hash, true),
        Some(_) => panic!("the only permitted suffix is `-dirty`, got {BUILD_COMMIT:?}"),
        None => (BUILD_COMMIT, false),
    };
    assert!(
        hash.len() >= 7 && hash.chars().all(|c| c.is_ascii_hexdigit()),
        "a commit should be a hex hash, got {hash:?} from {BUILD_COMMIT:?}"
    );
    let _ = dirty;
}

/// `for_run` stamps the build identity onto a run that is actually happening.
#[test]
fn a_run_records_the_build_that_produced_it() {
    let metadata = ReproducibilityMetadata::default().for_run();
    assert_eq!(metadata.probelab_version, PROBELAB_VERSION);
    assert_eq!(metadata.commit_sha, BUILD_COMMIT);
    assert!(!metadata.commit_sha.is_empty());
}

/// `Default` does *not* claim a build identity, and the asymmetry is the point.
///
/// `Default` means "nothing is known about a run that does not exist". Stamping a commit there would
/// attach a build identity to metadata nobody produced — and it would do so on every
/// partially-constructed value, including the ones in tests that are never stored. An empty string
/// reads as "not recorded", which is the truth.
#[test]
fn a_default_claims_no_build_identity() {
    let metadata = ReproducibilityMetadata::default();
    assert_eq!(
        metadata.commit_sha, "",
        "Default must not invent a commit; only `for_run` records one"
    );
    // The version is the exception, because it is always knowable and is not a claim about a run.
    assert_eq!(metadata.probelab_version, PROBELAB_VERSION);
}

/// The commit survives a serialization round trip, because a result bundle is stored as JSON.
///
/// If this field were dropped on write, a stored result would be exactly as unreproducible as before
/// this change — the information would exist only in the live process that produced it.
#[test]
fn the_build_identity_survives_a_round_trip() {
    let original = ReproducibilityMetadata::default().for_run();
    let encoded = serde_json::to_string(&original).expect("reproducibility metadata serializes");
    let decoded: ReproducibilityMetadata =
        serde_json::from_str(&encoded).expect("reproducibility metadata deserializes");
    assert_eq!(decoded, original);
    assert!(encoded.contains("commit_sha"), "the field is on the wire");
    assert!(
        encoded.contains("probelab_version"),
        "the field is on the wire"
    );
}

/// A payload written before these fields existed still loads, and the new fields come back empty.
///
/// `#[serde(default)]` is what makes the field addition non-breaking, and this is the test that says
/// so. Without it every stored bundle from a previous build would fail to parse, which for a
/// laboratory's results is a data-loss bug rather than a migration.
#[test]
fn an_older_payload_still_loads() {
    let legacy = r#"{
        "provider": "deterministic-mock",
        "model": "native-mock-optimal",
        "prompt_template_version": "1",
        "configuration_fingerprint": "abc",
        "environment_version": "grid-world/1"
    }"#;
    let decoded: ReproducibilityMetadata = serde_json::from_str(legacy)
        .expect("a bundle from before the build identity existed loads");
    assert_eq!(decoded.commit_sha, "", "absent means absent, not invented");
    assert_eq!(
        decoded.probelab_version, PROBELAB_VERSION,
        "the default fills in what can be known without the payload"
    );
}
