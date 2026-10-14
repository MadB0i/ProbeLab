# Release checklist

What has to happen between "the gates are green" and "there is a signed installer", and which of those
steps are not ours to take.

This file exists because the last three release-shaped steps — generating an installer, signing it, and
publishing it — are all irreversible on the machine they happen on and none of them has been
authorized. Recording them as undone is more useful than omitting them, because omitting them is how a
release gets cut with one of them quietly skipped.

## Current position

| Step | State |
| --- | --- |
| Rust gates (`fmt`, `clippy -D warnings`, `test`, `build`) | Passing |
| Frontend gates (`tsc -b`, `vitest`, `vite build`, `check:styles`, `check:icons`) | Passing |
| Tauri configuration validated against the bundled schema | Passing, gated on CI |
| Version provenance across the three manifests and `docs/STATUS.md` | Passing, gated on CI |
| Accessibility audit across all eleven routes | Passing |
| Grid trace, cancellation, replay integrity, restart durability | Passing |
| Installer generation | **Not done. Requires authorization.** |
| Installer signing | **Not done. Requires a certificate.** |
| Clean-install / uninstall verification | **Not done. Modifies the host. Requires authorization.** |
| Tag, push, publish | **Not done. Requires authorization.** |

## Steps that are ready

### 1. Build the release binaries

```powershell
cargo build --release -p probelab-desktop
cd apps/desktop; pnpm build
```

`pnpm build` is `tsc -b && vite build`, so a bundle cannot be produced from a tree that does not
typecheck. The Tauri `beforeBuildCommand` runs it.

### 2. Record provenance

```powershell
node scripts/release-provenance.mjs write artifacts/provenance.json target\release\probelab-desktop.exe
```

Writes the SHA-256, the size, the commit, and whether the tree was clean. If the tree is dirty the
manifest says so and says why that matters: a binary built from a modified tree cannot be reproduced
from the commit it names.

Re-check it at any time with:

```powershell
node scripts/release-provenance.mjs verify artifacts/provenance.json
```

This script does not sign, upload, or publish. It writes one local JSON file.

## Steps that need an owner and an authorization

### 3. Generate the installer

```powershell
cd apps/desktop; pnpm tauri build
```

Produces `target/release/bundle/nsis/ProbeLab_<version>_x64-setup.exe`. It has succeeded on this host
before; see `docs/STATUS.md` item 28. It is listed here rather than performed because it writes into
`target/release/bundle/` and because it is the step that consumes the signing certificate below.

**Needs:** explicit authorization. Nothing about the build depends on it, so there is no reason to run
it before someone has agreed to the artifact it produces.

### 4. Sign

Signing needs a code-signing certificate that does not live on a developer machine. None is present
here, and no script in this repository can honestly prepare one: the interesting half of signing is
the custody of the key, which is a process decision, not a code change.

**Needs:** a certificate, its storage arrangement, and a decision about whether the repository should
ever hold it. Until then the honest answer is that the installer would ship unsigned, and every user
would see a publisher warning naming an unverified signer.

### 5. Verify the installer on a clean machine

Running the setup executable installs software and modifies the host. The bundle has been produced and
launched from, but install and uninstall behavior has not been exercised.

**Needs:** authorization, and ideally a disposable VM rather than a developer's machine.

### 6. Tag, push, publish

Last commit, annotated tag, push, and any release publication.

**Needs:** authorization for each of these individually. Nothing in this repository performs them, and
no automation in `.github/workflows/` is configured to publish.

## What is deliberately not automated

- No workflow builds or uploads a release artifact. The CI workflow gates; it does not distribute.
- No workflow signs anything.
- No version is bumped automatically. `scripts/check-version-provenance.py` asserts the three
  manifests and `docs/STATUS.md` agree, and deliberately does not decide what the next version is —
  that is a judgement about what changed.
- No release is published from CI.

Each of these is a decision with a name attached to it. Automating them would remove the name.