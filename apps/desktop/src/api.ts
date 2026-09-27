/**
 * The typed IPC boundary.
 *
 * Every shape here mirrors a Rust type in `src-tauri/src/commands.rs`. They are written out rather
 * than inferred because the alternative — `any` at the boundary — means a rename on the Rust side
 * becomes a runtime `undefined` in the UI, and the UI's job is to be the thing that never silently
 * shows a blank panel.
 *
 * Nothing in this file ever holds a credential. The backend sends variable *names*; the values never
 * cross the bridge.
 */

import { invoke } from "@tauri-apps/api/core";

/** One agent, as the authoritative registry describes it. */
export type AgentSummary = {
  id: string;
  name: string;
  description: string;
  /** Snake-case category, e.g. `security_probe`. */
  category: string;
  /** Human label, e.g. `SECURITY PROBE`. */
  categoryLabel: string;
  /**
   * Whether this agent's score is a claim about a model.
   *
   * This is the field that stops the UI from presenting a sandbox probe's integrity count as a model
   * capability score. When it is false, the UI must not present the number as a model's ability.
   */
  measuresAModel: boolean;
  /** `in_process_deterministic` or `remote_model`. */
  execution: string;
  requiresNetwork: boolean;
  /** Empty means the agent applies to every benchmark. */
  applicableBenchmarks: string[];
};

/** One benchmark, with the honesty metadata a row needs to be renderable truthfully. */
export type BenchmarkDetail = {
  id: string;
  name: string;
  description: string;
  category: string;
  capabilityDimension: string;
  /**
   * `executed`, `authorized_deferred`, or `not_applicable`.
   *
   * The UI must render `authorized_deferred` as deferred. A benchmark that authorizes a capability
   * without executing it is not a working sandbox feature, and a row that reads like one would
   * overstate the lab's own containment.
   */
  capabilityExecution: string;
  scoring: string;
  /** Absent means this benchmark has no ceiling to compare against. */
  referenceAgent: string | null;
  /** `deterministic` or `projected_only`. */
  replaySupport: string;
  platformRequirements: string;
  limits: Record<string, string>;
  version: string;
};

/** One provider vendor, with defaults and a credential variable name but no credential value. */
export type ProviderSummary = {
  id: string;
  displayName: string;
  defaultBaseUrl: string;
  apiKeyEnv: string;
  requiresCredential: boolean;
};

export async function listAgents(): Promise<AgentSummary[]> {
  return invoke<AgentSummary[]>("list_agents");
}

export async function listBenchmarkDetails(): Promise<BenchmarkDetail[]> {
  return invoke<BenchmarkDetail[]>("list_benchmark_details");
}

export async function listProviders(): Promise<ProviderSummary[]> {
  return invoke<ProviderSummary[]>("list_providers");
}
