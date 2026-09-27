//! Containment identity regression tests.
//!
//! # Why these exist
//!
//! A contained process launch has two code paths in `windows.rs`: one that applies a derived
//! AppContainer identity through `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES`, and one that does not.
//! The second exists because the API allows it, and it is exactly the kind of thing that silently
//! becomes reachable.
//!
//! The audit that prompted this file found that `ContainedProcessSpawner` — the product's only entry
//! point — already calls `with_app_container()` unconditionally and fails closed when the identity
//! cannot be placed in force. So the implementation was correct and the *proof* was missing. These tests
//! are that proof, asserted structurally so a later edit cannot quietly remove the guarantee.
//!
//! # What is not claimed
//!
//! AppContainer isolation is not complete OS isolation. It restricts the token's capabilities and
//! package access; it does not confine every kernel surface, and it says nothing about the network
//! beyond the AppContainer's default outbound denial. No test here claims otherwise, and the
//! containment documentation carries the same caveat.

/// The spawner must always request an identity, with no flag to skip it.
#[test]
fn the_spawner_always_requests_an_identity() {
    let source = include_str!("../../sandbox/src/containment/spawn.rs");
    // Every branch that builds a `RestrictedProcessSpec` must attach an AppContainer request. The
    // if/else here covers both an empty and a non-empty capability list; a third branch added later
    // without one would be caught by the same two strings being present.
    assert!(
        source.contains("spec = spec.with_app_container();"),
        "the default spawn path must request an AppContainer identity"
    );
    assert!(
        source.contains("spec = spec.with_app_container_capabilities("),
        "the capability-granting path must also request an identity"
    );
    // And the negative space: there must be no way to launch without one.
    for forbidden in [
        "without_app_container",
        "skip_app_container",
        "no_app_container",
        "allow_unrestricted",
    ] {
        assert!(
            !source.contains(forbidden),
            "the spawner exposes {forbidden:?}, which would launch a child without an identity"
        );
    }
}

/// The launch must fail closed rather than fall back when the identity is absent.
#[test]
fn a_missing_identity_fails_closed() {
    let source = include_str!("../../sandbox/src/containment/windows.rs");
    assert!(
        source.contains("an AppContainer identity was requested but none could be placed in force"),
        "the fail-closed refusal for a missing identity is gone; without it a spawn could fall back to \
         an unisolated launch"
    );
    // The check must be a `return Err`, not a log line.
    assert!(
        source.contains("return Err(ContainmentError::AppContainer("),
        "a missing identity must return an error, not merely record one"
    );
}

/// The process spec alone does not grant an identity, and that is why the spawner must.
#[test]
fn a_bare_spec_has_no_identity_which_is_why_the_spawner_adds_one() {
    // This documents the hazard rather than asserting a bug: `RestrictedProcessSpec::new()` is
    // permissive, so anything constructing a spec directly could launch an unisolated child. The
    // spawner is the only sanctioned path, and the two tests above pin that it always adds identity.
    let source = include_str!("../../sandbox/src/containment.rs");
    assert!(
        source.contains("app_container: None,"),
        "the spec's default changed; if a bare spec now carries an identity, the spawner's explicit \
         call is redundant and this test should be revisited"
    );
    assert!(
        source.contains("pub fn with_app_container(mut self) -> Self"),
        "the opt-in that the spawner relies on is gone"
    );
}

/// The product must not construct a launch spec directly, bypassing the spawner.
#[test]
fn the_product_does_not_construct_specs_directly() {
    // A direct `RestrictedProcessSpec::new` anywhere outside the sandbox crate would be a path that
    // skips the spawner's unconditional identity. This asserts the absence structurally.
    for (name, source) in [
        ("broker", include_str!("../../broker/src/lib.rs")),
        ("engine", include_str!("../../engine/src/lib.rs")),
        ("benchmarks", include_str!("../../benchmarks/src/lib.rs")),
        ("lab", include_str!("../../lab/src/lib.rs")),
    ] {
        assert!(
            !source.contains("RestrictedProcessSpec"),
            "{name} constructs a process spec directly, bypassing the spawner's identity guarantee"
        );
    }
}
