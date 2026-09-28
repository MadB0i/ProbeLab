//! Stamps the build identity into the binary.
//!
//! # Why
//!
//! A stored result bundle records what was measured but not *what measured it*. Two runs of the same
//! benchmark at the same seed, on different commits of the lab, are not the same experiment, and
//! nothing in the stored record could distinguish them. `ReproducibilityMetadata` already carries the
//! provider, the model, and the environment version; the software that ties those together was the
//! missing half.
//!
//! # How
//!
//! The version comes from `Cargo.toml` via `env!("CARGO_PKG_VERSION")`, which is always correct and
//! never drifts. The commit is read from git at build time, because there is no way to know it at
//! compile time otherwise.
//!
//! Two rules keep this from being worse than nothing:
//!
//! - A dirty tree is recorded as such. `git rev-parse` alone reports `HEAD` whether or not the working
//!   copy matches it, so a build with uncommitted changes would be stamped with a commit that does not
//!   describe it. That is a claim of reproducibility the build cannot support, so the `-dirty` suffix
//!   is deliberate and a test asserts it.
//! - No git, no commit. Outside a checkout -- a published crate, a vendored source tree, CI with
//!   `fetch-depth: 0` missing -- the build still succeeds and records an explicit `unknown` rather
//!   than failing. A reproducibility field must not be the reason a crate will not compile.
//!
//! `cargo:rerun-if-changed` on `.git/HEAD` is the only reliable trigger: git does not touch mtimes on
//! the source files when a commit changes, so without it a second build would keep the first build's
//! commit forever.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=PROBELAB_COMMIT_SHA");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/HEAD");

    // A caller can supply the commit explicitly, which is what a reproducible-release pipeline should
    // do rather than trusting whatever git happens to say at build time.
    if let Ok(explicit) = std::env::var("PROBELAB_COMMIT_SHA") {
        if !explicit.trim().is_empty() {
            println!("cargo:rustc-env=PROBELAB_COMMIT_SHA={}", explicit.trim());
            return;
        }
    }

    let commit = read_commit().unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=PROBELAB_COMMIT_SHA={commit}");
}

/// The short commit, with `-dirty` appended when the tree does not match it.
fn read_commit() -> Option<String> {
    let head = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !head.status.success() {
        return None;
    }
    let commit = String::from_utf8(head.stdout).ok()?.trim().to_owned();
    if commit.is_empty() {
        return None;
    }

    // `git diff --quiet` exits 1 for a dirty tree, which is a success for our purposes -- the command
    // ran and told us what we needed. Only a failure to run at all is an error here.
    let status = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .ok()?;
    let dirty = status.status.success() && !status.stdout.is_empty();

    Some(if dirty {
        format!("{commit}-dirty")
    } else {
        commit
    })
}
