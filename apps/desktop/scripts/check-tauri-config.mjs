/**
 * Validates `tauri.conf.json` against the Tauri schema that ships with the CLI in this repository.
 *
 * # Why this check exists
 *
 * Nothing else in this project reads the Tauri configuration. `tauri build` does, but the build is the
 * one step that produces the installers, so it is also the step that is run last, by hand, and not at
 * all on CI. A typo in `tauri.conf.json` therefore survives every gate in this repository and surfaces
 * as a failed release build.
 *
 * This walks the config against `node_modules/@tauri-apps/cli/config.schema.json` and fails on:
 *
 *  * a key the schema does not define, where the schema forbids extra properties — which is how a
 *    misspelled `devCsp` becomes a build failure instead of a silently ignored setting;
 *  * a required key that is missing;
 *  * a value outside an enumerated set;
 *  * a type mismatch.
 *
 * It is deliberately not a complete JSON Schema implementation. It resolves `$ref`, `oneOf`/`anyOf` by
 * accepting a branch that validates, and ignores the rest — which is enough to catch the class of
 * mistake this file exists for, and small enough to read.
 *
 * It also asserts the two production properties that are easy to lose and impossible to notice: that the
 * shipped content-security policy does not carry the development server's websocket, and that both
 * policies do not fall back to `unsafe-eval`.
 *
 * Usage: node scripts/check-tauri-config.mjs
 */
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join, resolve } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const DESKTOP = resolve(HERE, "..");
const CONFIG_PATH = join(DESKTOP, "src-tauri", "tauri.conf.json");
const SCHEMA_PATH = join(DESKTOP, "node_modules", "@tauri-apps", "cli", "config.schema.json");

const config = JSON.parse(readFileSync(CONFIG_PATH, "utf8"));
const schema = JSON.parse(readFileSync(SCHEMA_PATH, "utf8"));

const problems = [];

/**
 * Collapses the composition keywords into one node.
 *
 * `allOf` has to be flattened rather than skipped. Most of the Tauri schema's top-level fields —
 * `app`, `bundle`, `build` among them — are declared as `{"allOf": [{"$ref": "..."}]}` with their real
 * properties one level down. A validator that resolves `$ref` but ignores `allOf` walks straight past
 * every one of them, finds no declared properties, and accepts every key: it passes a config with a
 * misspelled field inside it without a word. `oneOf`/`anyOf` are left to the branch logic below, which
 * has to *choose* rather than merge.
 */
function deref(node) {
  let current = node;
  while (current && typeof current === "object") {
    if (typeof current.$ref === "string") {
      const path = current.$ref.replace(/^#\//, "").split("/");
      current = path.reduce(
        (acc, key) => acc?.[key.replace(/~1/g, "/").replace(/~0/g, "~")],
        schema,
      );
      continue;
    }
    if (Array.isArray(current.allOf)) {
      const merged = { ...current };
      delete merged.allOf;
      const properties = { ...(current.properties ?? {}) };
      const required = [...(current.required ?? [])];
      let additionalProperties = current.additionalProperties;
      for (const branch of current.allOf) {
        const resolved = deref(branch);
        if (!resolved || typeof resolved !== "object") continue;
        Object.assign(properties, resolved.properties ?? {});
        required.push(...(resolved.required ?? []));
        if (resolved.additionalProperties !== undefined) {
          additionalProperties = resolved.additionalProperties;
        }
      }
      merged.properties = properties;
      merged.required = [...new Set(required)];
      if (additionalProperties !== undefined) merged.additionalProperties = additionalProperties;
      current = merged;
      continue;
    }
    break;
  }
  return current;
}

function typeOf(value) {
  if (value === null) return "null";
  if (Array.isArray(value)) return "array";
  if (Number.isInteger(value)) return "integer";
  return typeof value;
}

/**
 * Whether a value satisfies a declared type.
 *
 * `type` is a single string in most of the schema and an array in a good number of places — Tauri
 * declares its optional fields as `["string", "null"]` rather than as nullable, so both forms have to
 * work. An earlier version of this function read the union as one string, compared it against every
 * value, and reported the three most obviously correct fields in the file as type errors.
 */
function typeMatches(value, expected) {
  const actual = typeOf(value);
  if (Array.isArray(expected)) {
    return expected.some((candidate) => typeMatches(value, candidate));
  }
  if (expected === "number") return actual === "number" || actual === "integer";
  if (expected === "integer") return actual === "integer";
  return actual === expected;
}

/** A schema node that accepts `value`, used to pick a branch out of oneOf/anyOf. */
function accepts(node, value) {
  const resolved = deref(node);
  if (!resolved || typeof resolved !== "object") return false;
  if (resolved.type && !typeMatches(value, resolved.type)) return false;
  if (resolved.enum && !resolved.enum.includes(value)) return false;
  if (resolved.const !== undefined && value !== resolved.const) return false;
  return true;
}

function validate(value, node, path) {
  const resolved = deref(node);
  if (!resolved) {
    problems.push(`${path}: schema could not be resolved (${JSON.stringify(node)})`);
    return;
  }

  for (const key of ["oneOf", "anyOf"]) {
    if (Array.isArray(resolved[key])) {
      if (!resolved[key].some((branch) => accepts(branch, value))) {
        problems.push(`${path}: ${JSON.stringify(value)} matches none of the ${key} options`);
      }
      return;
    }
  }

  if (resolved.enum && !resolved.enum.includes(value)) {
    problems.push(`${path}: ${JSON.stringify(value)} is not one of ${resolved.enum.join(", ")}`);
    return;
  }

  if (resolved.type && !typeMatches(value, resolved.type)) {
    problems.push(`${path}: expected ${resolved.type}, found ${typeOf(value)}`);
    return;
  }

  if (resolved.type === "object" || (value !== null && typeof value === "object" && !Array.isArray(value))) {
    const properties = resolved.properties ?? {};

    for (const required of resolved.required ?? []) {
      if (!(required in value)) {
        problems.push(`${path}: missing required key "${required}"`);
      }
    }

    for (const [key, child] of Object.entries(value)) {
      if (key in properties) {
        validate(child, properties[key], `${path}.${key}`);
      } else if (resolved.additionalProperties === false) {
        problems.push(`${path}.${key}: not a key the Tauri schema defines`);
      } else if (typeof resolved.additionalProperties === "object") {
        validate(child, resolved.additionalProperties, `${path}.${key}`);
      }
    }
  }

  if (resolved.type === "array" && Array.isArray(value) && resolved.items) {
    value.forEach((item, index) => validate(item, resolved.items, `${path}[${index}]`));
  }
}

validate(config, schema, "tauri.conf.json");

/*
 * The production policy assertions.
 *
 * These are separate from the schema walk because the schema cannot express them: both policies are
 * perfectly valid strings either way. A development websocket origin in the shipped policy is dead
 * weight at best; `unsafe-eval` in either is a live hole.
 */
const csp = config.app?.security?.csp ?? "";
const devCsp = config.app?.security?.devCsp ?? "";

if (csp.includes("1420")) {
  problems.push(
    "app.security.csp carries the development server's websocket origin (1420). A shipped build has no " +
      "dev server to connect to, and a policy that admits a local websocket it never uses is one more " +
      "origin an injected script could reach. Move it to devCsp.",
  );
}
if (!devCsp.includes("1420")) {
  problems.push(
    "app.security.devCsp does not admit the development server's websocket, so hot module replacement " +
      "will fail under `tauri dev` while every build gate stays green.",
  );
}
for (const [name, policy] of [["csp", csp], ["devCsp", devCsp]]) {
  if (!policy) {
    problems.push(`app.security.${name} is missing`);
  } else if (/unsafe-eval/.test(policy)) {
    problems.push(`app.security.${name} allows unsafe-eval`);
  }
}
if (config.app?.security?.freezePrototype !== true) {
  problems.push(
    "app.security.freezePrototype is not true. Freezing the prototype chain closes a class of DOM " +
      "clobbering that a local application has no reason to leave open.",
  );
}
for (const field of ["publisher", "shortDescription", "longDescription", "category"]) {
  if (!config.bundle?.[field]) {
    problems.push(
      `bundle.${field} is missing. Without it the installer ships with an empty publisher and an empty ` +
        "description, which is what an unsigned default template looks like to a user deciding whether to " +
        "run it.",
    );
  }
}

console.log("Tauri configuration");
console.log(`  config:  ${CONFIG_PATH}`);
console.log(`  schema:  ${SCHEMA_PATH.replace(DESKTOP, "apps/desktop")}`);
console.log(`  version: ${config.version} (${config.productName})`);
console.log(`  bundle:  ${config.bundle?.active ? `active, targets: ${config.bundle.targets}` : "inactive"}`);

if (problems.length > 0) {
  console.error("");
  for (const problem of problems) {
    console.error(`FAIL  ${problem}`);
  }
  console.error(`\nTauri configuration: ${problems.length} problem(s)`);
  process.exit(1);
}

console.log("\nTauri configuration: valid against the bundled schema, and the production policy is clean.");