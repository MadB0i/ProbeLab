/**
 * Computes and verifies a provenance manifest for built artifacts.
 *
 * # What this is for
 *
 * "The binary on the download page is the one this commit produced" is a claim that has no evidence
 * behind it unless somebody records it. This writes that evidence: for each artifact, the SHA-256 of
 * its bytes, its size, the commit the tree was at, and whether the tree was clean. `--verify` reads a
 * manifest back and re-hashes the files it names.
 *
 * # What this deliberately does not do
 *
 * It does not sign, upload, publish, tag, or produce an installer. It writes a local JSON file and
 * nothing else. A signature has to come from a key that does not sit on a developer machine, so the
 * signing step is not something this script can honestly prepare on its own — see
 * `docs/RELEASE-CHECKLIST.md`, which records that as a step needing an owner and a certificate.
 *
 * The commit and dirty state are recorded rather than assumed, because a binary built from a modified
 * tree has no meaningful provenance, and a manifest that quietly omits that fact is worse than no
 * manifest.
 *
 * Usage:
 *   node scripts/release-provenance.mjs write <output.json> <artifact...>
 *   node scripts/release-provenance.mjs verify <manifest.json>
 */
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync, statSync, writeFileSync } from "node:fs";
import { basename, relative, resolve } from "node:path";

const REPO = resolve(new URL("..", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1"));

/** Runs a git command and returns its trimmed output, or null if it could not be run. */
function git(...args) {
  try {
    return execFileSync("git", args, { cwd: REPO, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  } catch {
    return null;
  }
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function describe(path) {
  const absolute = resolve(path);
  return {
    path: relative(REPO, absolute).replace(/\\/g, "/"),
    bytes: statSync(absolute).size,
    sha256: sha256(absolute),
  };
}

/** The state of the tree the artifacts were built from. */
function provenance() {
  const commit = git("rev-parse", "HEAD");
  const branch = git("rev-parse", "--abbrev-ref", "HEAD");
  const status = git("status", "--porcelain");
  return {
    commit,
    branch,
    // Recorded rather than assumed. An empty string means clean; anything else lists the paths.
    dirty: Boolean(status),
    dirtyPaths: status ? status.split("\n").map((line) => line.slice(3)) : [],
    builtAt: new Date().toISOString(),
  };
}

const [command, ...rest] = process.argv.slice(2);

if (command === "write") {
  const [output, ...artifacts] = rest;
  if (!output || artifacts.length === 0) {
    console.error("usage: node scripts/release-provenance.mjs write <output.json> <artifact...>");
    process.exit(2);
  }

  const entries = artifacts.map((artifact) => {
    try {
      return describe(artifact);
    } catch (error) {
      console.error(`FAIL  cannot hash ${artifact}: ${error.message}`);
      process.exit(1);
    }
  });

  const manifest = { schema: 1, ...provenance(), artifacts: entries };
  writeFileSync(resolve(output), `${JSON.stringify(manifest, null, 2)}\n`);

  console.log("Provenance manifest written");
  console.log(`  ${relative(REPO, resolve(output))}`);
  console.log(`  commit ${manifest.commit?.slice(0, 12) ?? "unknown"} on ${manifest.branch ?? "unknown"}`);
  if (manifest.dirty) {
    console.log(`  WARNING: the tree was not clean — ${manifest.dirtyPaths.length} path(s) modified`);
  }
  for (const entry of entries) {
    console.log(`  ${entry.sha256}  ${String(entry.bytes).padStart(10)}  ${entry.path}`);
  }
  if (manifest.dirty) {
    console.log("");
    console.log("This manifest records a build from a modified tree. That is not a failure, but it is");
    console.log("not provenance either: a release built from it cannot be reproduced from the commit.");
  }
  process.exit(0);
}

if (command === "verify") {
  const [manifestPath] = rest;
  if (!manifestPath) {
    console.error("usage: node scripts/release-provenance.mjs verify <manifest.json>");
    process.exit(2);
  }

  const manifest = JSON.parse(readFileSync(resolve(manifestPath), "utf8"));
  const failures = [];

  for (const entry of manifest.artifacts) {
    const absolute = resolve(REPO, entry.path);
    let actual;
    try {
      actual = sha256(absolute);
    } catch (error) {
      failures.push(`${entry.path}: ${error.message}`);
      continue;
    }
    if (actual !== entry.sha256) {
      failures.push(
        `${entry.path}: recorded ${entry.sha256.slice(0, 16)}…, file hashes ${actual.slice(0, 16)}…`,
      );
    } else if (statSync(absolute).size !== entry.bytes) {
      failures.push(`${entry.path}: size differs from the manifest`);
    }
  }

  console.log(`Provenance of ${basename(resolve(manifestPath))}`);
  console.log(`  commit ${manifest.commit ?? "unknown"}${manifest.dirty ? " (tree was dirty)" : ""}`);
  for (const entry of manifest.artifacts) {
    console.log(`  ${entry.sha256.slice(0, 16)}…  ${entry.path}`);
  }
  if (failures.length > 0) {
    console.error("");
    for (const failure of failures) {
      console.error(`FAIL  ${failure}`);
    }
    process.exit(1);
  }
  console.log("\nEvery artifact matches the manifest.");
  process.exit(0);
}

console.error("usage: node scripts/release-provenance.mjs <write|verify> ...");
process.exit(2);