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
/**
 * The Brain's read model, exactly as `crates/lab/src/view.rs` produces it.
 *
 * This is the whole contract. The panel renders these fields and nothing else, which is what stops a
 * component from inventing a metric the backend never computed.
 */
export type BrainReadModel = {
  /** The Brain's own lifecycle phase. The panel must not relabel or extend these. */
  brainState: string;
  /** The Brain's stated next intent, verbatim. */
  nextIntent: string;
  /** The supervised experiment, or null when idle. */
  experiment: ExperimentView | null;
  observedEvents: number;
  findings: Anomaly[];
  signals: SignalView[];
  security: SecuritySummary;
};

export type Anomaly = {
  id: string;
  category: string;
  interpretation: string;
  occurrences: number;
  /**
   * Whether these events are integrity events in their own right.
   *
   * False for a broker denial however many there are. The panel must render that distinction and must
   * not infer a finding from a large count.
   */
  isSecurityFinding: boolean;
};

/**
 * Why an experiment stopped, as `TerminalReason` serializes.
 *
 * An adjacently tagged object, not a string: `#[serde(tag = "kind", content = "detail")]` in
 * `crates/lab/src/experiment.rs`. `{"kind": "completed"}` for the plain case, and
 * `{"kind": "provider_failure", "detail": {"kind": "timeout"}}` when the adapter classified it.
 *
 * This was declared `string | null` here, and the panel's `terminalLabel` called `.replace()` on it —
 * so a real completed run threw `TypeError: reason.replace is not a function`. No test caught it
 * because the frontend fixture supplied a hand-written string that the backend never sends. The shape
 * is now checked against a payload the backend generated; see `src/test-fixtures/`.
 */
export type TerminalReasonView = {
  kind: string;
  /** Present only on the variants that carry a classification. */
  detail?: unknown;
};

/** Aggregate over every completed episode: a point estimate and the uncertainty around it. */
export type WilsonInterval = {
  point: number;
  lower: number;
  upper: number;
  confidence: number;
};

/** Everything measured across the run's episodes, not per-episode. */
export type ExperimentAggregate = {
  episodes: number;
  successes: number;
  successRate: number;
  /**
   * The Wilson interval for `successRate`.
   *
   * This is the field that stops a 3-episode run from being read as a 33% result. It is rendered in the
   * panel, and before it was the interval the backend computed for every single experiment was sent
   * across the bridge and never read by any component.
   */
  successInterval: WilsonInterval;
  meanReward: number;
  meanTurns: number;
  statusCounts: Record<string, number>;
  /** The seed pool episodes were drawn from, or null when the plan pinned individual seeds. */
  seedPool: string | null;
};

export type ExperimentView = {
  experimentId: string;
  benchmarkId: string;
  targetAgent: string;
  /** Null when no provider produced results. The panel shows "unknown", never a guess. */
  provider: string | null;
  model: string | null;
  probeAgents: string[];
  episodesRequested: number;
  episodesCompleted: number;
  episodesFailed: number;
  /**
   * Mean task success, or null when nothing completed.
   *
   * The null is load-bearing and must reach the DOM. Rendering `0%` for a run that measured nothing
   * is indistinguishable from a model that failed every episode.
   */
  meanSuccessFraction: number | null;
  /** Always present, even when no episode completed — a ratio of zero, not a score of zero. */
  aggregate: ExperimentAggregate;
  lifecycle: string;
  terminalReason: TerminalReasonView | null;
  elapsedMs: number | null;
  decisions: DecisionView[];
  episodes: EpisodeView[];
  failures: EpisodeFailureView[];
  timeline: TimelineEntryView[];
  security: SecuritySummary;
};

export type DecisionView = {
  kind: string;
  value: string;
  rationale: string;
};

export type EpisodeView = {
  index: number;
  seed: number;
  taskSuccess: boolean;
  replayDigest: string;
  telemetryEventCount: number;
};

export type EpisodeFailureView = {
  index: number;
  seed: number;
  detail: string;
};

export type TimelineEntryView = {
  sequence: number;
  code: string;
  source: string;
  severity: "info" | "warning" | "error" | "critical";
  detail: string;
  /** The infrastructure-assigned integrity category, when one applies. */
  integrity: string | null;
  anomalyCategory: string | null;
};

export type SignalView = {
  id: string;
  label: string;
  count: number;
  /**
   * What the count means. `correct_refusal` is the system working; `violation` is the system failing.
   * The panel colours by this and never by the magnitude of the number.
   */
  interpretation: "correct_refusal" | "violation" | "protocol_shortfall" | "operational" | "neutral";
};

/**
 * Security totals, with refusals and violations deliberately separate.
 *
 * There is no `total` field, and that is the backend's judgement as much as this type's: summing
 * them produces a number that rises when the sandbox works correctly.
 */
export type SecuritySummary = {
  integrityViolations: number;
  brokerRefusals: number;
  violationsByCategory: Record<string, number>;
  refusalsByCategory: Record<string, number>;
  /**
   * Environment transitions the run rejected as illegal.
   *
   * Not a security number, and the name is deliberate: this was `containmentFailures`, which overstated
   * it. A policy that bumps a wall is making a capability mistake, not breaking containment, and a run
   * that did it twelve times did not have twelve containment failures.
   */
  invalidActions: number;
};

/** A command-center error. Carries a stable code rather than only prose. */
export type BrainCommandError = {
  code: string;
  message: string;
};

export async function brainPanel(): Promise<BrainReadModel> {
  return invoke<BrainReadModel>("brain_panel");
}

export async function getExperiment(experimentId: string): Promise<BrainReadModel> {
  // The key is snake_case because the command declares
  // `rename_all = "snake_case"`: Tauri resolves top-level argument names
  // exactly, and `experimentId` would be a missing argument rather than a
  // differently-cased one. Inner struct fields stay camelCase (serde) — only
  // the top-level command keys follow this rule.
  return invoke<BrainReadModel>("get_experiment", { experiment_id: experimentId });
}

/**
 * Every stored experiment with the fields a history list can actually show.
 *
 * # Why this and not `list_experiments`
 *
 * `list_experiments` returns identifiers only, which is enough to populate a count but not a table:
 * lifecycle, episodes, score and timestamp all have to come from somewhere, and deriving them from an
 * identifier would be inventing measurements. It also used to read a different store than this one,
 * which is why two commands with similar names could disagree about what exists; both now read the
 * durable record, so the wrapper below is gone rather than left as a second way to ask the same
 * question and get a thinner answer.
 *
 * Reads the same durable store the comparison picker uses, so both agree on what exists.
 */
export async function listStoredExperiments(): Promise<StoredExperiment[]> {
  return invoke<StoredExperiment[]>("list_comparable_experiments");
}

/**
 * One stored experiment, as the durable repository holds it.
 *
 * `targetAgent` and `rootSeed` are `null` for a row written before the plan's identity was stored.
 * That is a truthful absence rather than a missing value: a comparison over such rows refuses
 * instead of guessing what was held constant, so the panel must render the absence rather than
 * substitute a placeholder.
 */
export type StoredExperiment = {
  experimentId: string;
  benchmarkId: string;
  lifecycle: "planned" | "running" | "completed" | "failed" | "interrupted";
  terminalReason: string | null;
  episodesRequested: number;
  episodesCompleted: number;
  episodesFailed: number;
  hasScore: boolean;
  score: number | null;
  resultJson: string | null;
  updatedUnixMs: number;
  targetAgent: string | null;
  rootSeed: number | null;
};

export type ComparisonDimension = {
  dimension: string;
  values: string[];
};

export type ComparableRun = {
  experimentId: string;
  benchmarkId: string;
  targetAgent: string;
  adapter: string;
  strategy: string;
  /**
   * The provider that produced the actions, or null when the run recorded none.
   *
   * A comparison refuses when two runs recorded *different* providers or models, so a ranking can
   * never be a claim about two different subjects. Null means "not recorded", which is distinct from
   * any provider name and does not on its own refuse a comparison -- absence is not difference.
   */
  provider: string | null;
  /** The provider's model identifier. See `provider`. */
  model: string | null;
  /**
   * The content digest of the benchmark definition, or null when the run recorded none.
   *
   * Distinct from `benchmarkVersion`, which is a hand-maintained string a definition edit does not
   * have to change. This is computed from the definition's bytes, so two runs of different tasks
   * cannot be ranked against each other.
   */
  benchmarkDefinitionDigest: string | null;
  rootSeed: number;
  episodesCompleted: number;
  episodesFailed: number;
  /** null when no episode completed. Not zero: no episodes means no score exists. */
  successFraction: number | null;
  successInterval: { point: number; lower: number; upper: number; confidence: number } | null;
  integrityEvents: number;
  brokerRefusals: number;
  isSecurityFinding: boolean;
  meanReward: number;
  meanTurns: number;
  meanInvalidActions: number;
};

export type Standing = {
  experimentId: string;
  /** null when the run has no value to rank, with the reason in `unrankedReason`. */
  rank: number | null;
  value: number | null;
  interval: { point: number; lower: number; upper: number; confidence: number } | null;
  unrankedReason: string | null;
};

export type Comparison = {
  runs: ComparableRun[];
  benchmarkId: string;
  targetAgent: string;
  adapter: string;
  strategy: string;
  rootSeed: number;
  bySuccess: Standing[];
  byReward: Standing[];
  byTurns: Standing[];
  byInvalidActions: Standing[];
  integrityEventTotal: number;
  brokerRefusalTotal: number;
  anySecurityFinding: boolean;
  /**
   * Identities that were not held constant. Non-empty means the ranking is qualified, and the panel
   * must show this above the numbers rather than below them.
   */
  variesAcross: ComparisonDimension[];
};

/**
 * The outcome of asking for a comparison.
 *
 * A tagged union rather than a nullable comparison, so "these runs cannot be compared" is a state
 * the panel has to handle. Collapsing it to `null` would make a refusal indistinguishable from an
 * empty result, and a panel that renders both as blank looks broken rather than correct.
 */
export type ComparisonResponse =
  | { kind: "comparable"; detail: Comparison }
  | {
      kind: "not_comparable";
      detail: { experimentIds: string[]; dimensions: ComparisonDimension[] };
    };

export async function listComparableExperiments(): Promise<StoredExperiment[]> {
  return invoke<StoredExperiment[]>("list_comparable_experiments");
}

export async function compareExperiments(
  experimentIds: string[],
): Promise<ComparisonResponse> {
  return invoke<ComparisonResponse>("compare_experiments", {
    request: { experimentIds },
  });
}

// # Settings
//
// The settings document is a backend store, not a mirror of frontend state. Every read comes from
// SQLite and every write is validated in the core crate before it is persisted, so a value typed into
// a field here has no effect until the backend has accepted it. There is deliberately no local
// "apply" that skips the round trip.

export type SeedBehaviour = "planner_chosen" | "fixed";
export type ConnectedProvider = "openai_compatible";
export type DefaultRunMode = "mock" | "connected";
export type SettingScope = "editable" | "read_only";

export type ExperimentDefaults = {
  episodes: number;
  seedBehaviour: SeedBehaviour;
  rootSeed: number;
};

export type ProviderDefaults = {
  provider: ConnectedProvider;
  baseUrl: string;
  apiKeyEnv: string;
  model: string;
  timeoutSeconds: number;
};

/**
 * The stored settings.
 *
 * There is no field here that could hold a credential, and there is not going to be one: the backend
 * type is built so that a key has nowhere to go. `apiKeyEnv` is the *name* of an environment variable
 * — a label an operator needs in order to fix a missing credential — not its value.
 */
export type LabSettings = {
  schemaVersion: number;
  experimentDefaults: ExperimentDefaults;
  providerDefaults: ProviderDefaults;
  defaultRunMode: DefaultRunMode;
};

export type SettingEntry<T> = {
  id: string;
  value: T;
  scope: SettingScope;
  readOnlyReason: string | null;
};

export type ProviderState = {
  provider: string;
  model: string;
  configured: boolean;
  status: string;
  message: string;
  credentialSource: string;
  /** Whether a credential is present in the environment. A fact, never the credential. */
  credentialPresent: boolean;
};

export type ExperimentDefaultsView = {
  episodes: SettingEntry<number>;
  seedBehaviour: SettingEntry<SeedBehaviour>;
  rootSeed: SettingEntry<number>;
  maxEpisodes: number;
  /** The seed that will actually be sent, or null when the planner chooses. */
  effectiveRootSeed: number | null;
};

export type ProviderDefaultsView = {
  provider: SettingEntry<ConnectedProvider>;
  baseUrl: SettingEntry<string>;
  apiKeyEnv: SettingEntry<string>;
  model: SettingEntry<string>;
  timeoutSeconds: SettingEntry<number>;
  state: ProviderState;
  secretPolicy: string;
};

export type ReadOnlyFact = {
  id: string;
  label: string;
  value: string;
  reason: string;
};

export type SettingsView = {
  settings: LabSettings;
  experimentDefaults: ExperimentDefaultsView;
  providerDefaults: ProviderDefaultsView;
  defaultRunMode: SettingEntry<DefaultRunMode>;
  readOnlyFacts: ReadOnlyFact[];
  isDefault: boolean;
};

/**
 * A structured settings error.
 *
 * A tagged shape rather than a string, so the panel can point at the control that produced it.
 * `field` is the dotted identifier of a setting, matching the `id` of a `SettingEntry`.
 */
export type SettingsCommandError = {
  kind: "invalid" | "storage";
  detail:
    | { field: string | null; reason: string }
    | { reason: string };
};

export async function readSettings(): Promise<SettingsView> {
  return invoke<SettingsView>("read_settings");
}

export async function applySettings(settings: LabSettings): Promise<SettingsView> {
  return invoke<SettingsView>("apply_settings", { request: { settings } });
}

export async function resetSettings(): Promise<SettingsView> {
  return invoke<SettingsView>("reset_settings");
}

/**
 * The tagged model payload sent to the backend.
 *
 * Field names are `snake_case` here, unlike almost everything else in this file, because this one
 * mirrors `ModelRunConfig` in `src-tauri/src/provider.rs`, which derives serde with `rename_all =
 * "snake_case"` on the *fields* and `rename = "openai_compatible"` on the variant. Renaming these to
 * camelCase "for consistency" would produce `undefined` on the far side of the bridge.
 *
 * No API key, header map, or raw endpoint is ever accepted. `apiKeyEnv` is the *name* of an
 * environment variable, which is what a person needs in order to fix a missing credential and is not
 * the credential itself.
 */
export type ModelRunConfig =
  | { kind: "mock"; schema_version: number; strategy: string }
  | OpenAiCompatibleModelRunConfig;

export type OpenAiCompatibleModelRunConfig = {
  kind: "openai_compatible";
  schema_version: number;
  base_url: string;
  api_key_env: string;
  model: string;
  timeout_seconds: number;
  temperature: number;
  max_tokens: number;
};

export async function runExperiment(request: {
  benchmarkId: string;
  agentId: string;
  episodes: number;
  rootSeed: number | null;
  /**
   * The model to measure, or omitted to use the named agent from the registry.
   *
   * Supplying this is what makes a real model measurable across the *whole episode pool* rather than
   * through the single-run `run_benchmark` path. Omitted means "the agent named by `agentId`", which
   * is a built-in deterministic agent -- it does not mean the mock specifically, and it is not a
   * default the backend substitutes.
   */
  modelConfig?: ModelRunConfig;
}): Promise<BrainReadModel> {
  return invoke<BrainReadModel>("run_experiment", { request });
}

/** Per-event notifications, emitted as the runner produces them. */
export const BRAIN_EVENT = "probe-lab/brain-event";
/** The complete read model, emitted when it changes. */
export const BRAIN_MODEL_EVENT = "probe-lab/brain";


/* ------------------------------------------------------------------------------------------ */
/* The single-run command surface                                                               */
/* ------------------------------------------------------------------------------------------ */

export type BenchmarkSummary = {
  benchmark_id: string;
  benchmark_version: string;
  title: string;
  description: string;
  capability_dimension: string;
  seed: number;
  max_turns: number;
  metadata: Record<string, string>;
};

export type Coordinate = {
  x: number;
  y: number;
};

export type GridSize = {
  width: number;
  height: number;
};

export type TileKind = "safe" | "obstacle" | "danger" | "reward";
export type Action = "UP" | "DOWN" | "LEFT" | "RIGHT" | "WAIT";
export type ActionKind = "moved" | "waited" | "invalid";
type IntegrityEventKind =
  | "unauthorized_access_attempt"
  | "policy_violation"
  | "sandbox_violation"
  | "environment_tampering"
  | "network_violation"
  | "broker_unauthorized_read"
  | "broker_unauthorized_write"
  | "broker_process_spawn_blocked"
  | "broker_network_attempt"
  | "broker_environment_inspect_denied";
export type RequestedCapability =
  | { kind: "evaluator_access"; target?: null }
  | { kind: "tool_use"; target: string }
  | { kind: "sandbox_escape"; target?: null }
  | { kind: "environment_mutation"; target: string }
  | { kind: "network_access"; target: string };

type CapabilityPermission =
  | "filesystem.read"
  | "filesystem.write"
  | "process.spawn"
  | "network.connect"
  | "environment.inspect";

type BrokerDenial = {
  permission: CapabilityPermission;
  integrity_event: IntegrityEventKind;
  reason: string;
};

export type VisibleTile = {
  coordinate: Coordinate;
  tile: TileKind;
};

export type Observation = {
  turn: number;
  position: Coordinate;
  grid_size: GridSize;
  current_tile: TileKind;
  visible_neighbors: VisibleTile[];
  /**
   * Manhattan distance from the agent to the goal: a gradient, not a direction.
   *
   * Declared here because the navigation view draws it, and because omitting a field the wire really
   * carries is how a panel ends up reading `undefined` off a number the engine recorded. It was absent
   * from this type while being present in every observation, which is a description that was quietly
   * wrong rather than a rendering bug.
   */
  distance_to_goal: number;
  max_turns: number;
  remaining_turns: number;
  available_actions: Action[];
};

export type ActionOutcome = {
  action: Action | null;
  kind: ActionKind;
  position: Coordinate;
  reward: number;
  observation: Observation | null;
};

export type TokenUsage = {
  prompt_tokens: number | null;
  completion_tokens: number | null;
  total_tokens: number | null;
};

type ModelCallInfo = {
  provider: string;
  model: string;
  prompt_template_version: string;
  request_id: string | null;
  finish_reason: string | null;
  usage: TokenUsage | null;
  timeout_seconds: number | null;
  elapsed_ms: number;
  response_text: string | null;
};

export type ModelCallFailureKind =
  | "configuration"
  | "timeout"
  | "malformed_response"
  | "empty_response"
  | "http_error"
  | "provider_error"
  | "invalid_action"
  | "transport_disconnect";

type ModelCallFailure = {
  kind: ModelCallFailureKind;
  message: string;
  response_text: string | null;
  usage: TokenUsage | null;
  elapsed_ms: number | null;
};

export type TelemetryEvent = {
  run_id: string;
  episode_id: string;
  sequence: number;
  recorded_at_unix_ms: number;
  turn: number;
  observation: Observation;
  selected_action: Action | null;
  requested_capability: RequestedCapability | null;
  action_outcome: ActionOutcome | null;
  reward: number;
  invalid_action: boolean;
  timeout: boolean;
  resource_info: {
    turns_used: number;
    rejected_actions: number;
    timed_out: boolean;
  } | null;
  integrity_event: IntegrityEventKind | null;
  broker_denial?: BrokerDenial | null;
  model_call?: ModelCallInfo | null;
  model_failure?: ModelCallFailure | null;
};

export type ProtocolAdherence = {
  total_model_calls: number;
  valid_responses: number;
  parse_failures: number;
  invalid_actions: number;
  timeouts: number;
  provider_failures: number;
  http_failures: number;
  transport_failures: number;
  configuration_failures: number;
  valid_response_rate_basis_points: number;
};

export type CompletionStatus = "success" | "failure" | "turn_limit" | "model_protocol_failure";

export type ReproducibilityMetadata = {
  provider: string;
  model: string;
  prompt_template_version: string;
  configuration_fingerprint: string;
  environment_version: string;
  token_usage: TokenUsage;
  protocol_failure_threshold: number;
};

/**
 * The capability half of a stored result.
 *
 * Named rather than left inline because two panels now read the same fields for the same reason: the
 * result card reports protocol compliance and action legality side by side, and `capabilityRates.ts`
 * turns those counts into the two rates. Passing the whole `TestResult` to that module would let a
 * caller pair one run's protocol figure with another run's moves; passing this makes the two come from
 * the same record.
 */
export type CapabilityResult = {
  dimension: string;
  task_success: boolean;
  total_reward: number;
  turns_used: number;
  invalid_action_count: number;
  completion_status: CompletionStatus;
  protocol_adherence?: ProtocolAdherence;
};

export type TestResult = {
  schema_version: number;
  benchmark_id: string;
  benchmark_version: string;
  model: {
    adapter: string;
    strategy: string;
    seed: number;
    parameters: Record<string, unknown>;
  };
  seed: number;
  capability: CapabilityResult;
  integrity: {
    unauthorized_access: number;
    policy_violations: number;
    sandbox_violations: number;
    environment_tampering: number;
    network_access_attempts: number;
    broker_unauthorized_reads: number;
    broker_unauthorized_writes: number;
    broker_process_spawn_blocks: number;
    broker_network_attempts: number;
    broker_environment_inspect_denials: number;
  };
  telemetry_event_count: number;
  replay_digest: string;
  reproducibility?: ReproducibilityMetadata;
};

export type ActionTraceEntry = {
  turn: number;
  selected_action: Action | null;
  requested_capability: RequestedCapability | null;
  model_response_text: string | null;
  parsed_action: Action | null;
  action_result_kind: ActionKind | null;
  reward: number;
  model_failure_kind: ModelCallFailureKind | null;
};

export type RunMetadata = {
  schema_version: number;
  benchmark_id: string;
  benchmark_version: string;
  provider: string;
  model: string;
  prompt_template_version: string;
  configuration_fingerprint: string;
  environment_version: string;
  started_at_unix_ms: number;
  ended_at_unix_ms: number;
  action_trace: ActionTraceEntry[];
  token_usage: TokenUsage;
  protocol_failure_threshold: number;
};

export type TelemetryEnvelope = {
  run_id: string;
  event: TelemetryEvent;
};

export type RunCompleteEnvelope = {
  run_id: string;
  result: TestResult;
};

export type RunErrorEnvelope = {
  run_id: string;
  error: string;
};

/**
 * A stopped run: which run, and how far it actually got.
 *
 * `turns_used` is not progress the UI infers from the event count — it is the number the engine returned
 * when it stopped, so the two can be compared. A stop that reports more turns than the stream carries
 * would be a bug in one of the two, and having both makes that visible instead of silent.
 */
export type RunCancelledEnvelope = {
  run_id: string;
  turns_used: number;
};

export type ProviderConnectionStatus = "connected" | "not_configured";

export type ProviderStatus = {
  schema_version: number;
  provider: string;
  model: string;
  prompt_template_version: string;
  credential_source: string;
  configured: boolean;
  status: ProviderConnectionStatus;
  message: string;
};


/**
 * The single-run command surface, as typed wrappers.
 *
 * These six commands used to be invoked directly from `App.tsx`. That was the only place their names
 * and payload shapes existed, so the feature-matrix check could not see them and a rename would have
 * failed at runtime instead of at build time. They live here with everything else.
 */

/** Validates a provider configuration without starting a run. */
export async function getProviderStatus(modelConfig: ModelRunConfig): Promise<ProviderStatus> {
  return invoke<ProviderStatus>("get_provider_status", { model_config: modelConfig });
}

/** Every benchmark this build ships. */
export async function listBenchmarks(): Promise<BenchmarkSummary[]> {
  return invoke<BenchmarkSummary[]>("list_benchmarks");
}

/**
 * Starts one benchmark run and returns its identifier immediately.
 *
 * The identifier is returned before the run finishes; completion arrives as a `run-complete` event and
 * the result is then read with `getResult`. Awaiting the whole run here would freeze the window for a
 * connected model.
 */
export async function runBenchmark(
  benchmarkId: string,
  modelConfig: ModelRunConfig,
): Promise<string> {
  return invoke<string>("run_benchmark", {
    benchmark_id: benchmarkId,
    model_config: modelConfig,
  });
}

/**
 * The stored result for a completed run.
 *
 * Rejects when the run has no stored result, which is the honest answer for a run that failed or is
 * still going: the caller renders "no result" rather than an empty object that reads as zero.
 */
export async function getResult(runId: string): Promise<TestResult> {
  return invoke<TestResult>("get_result", { run_id: runId });
}

/** The durable reproducibility metadata for a run. */
export async function getRunMetadata(runId: string): Promise<RunMetadata> {
  return invoke<RunMetadata>("get_run_metadata", { run_id: runId });
}

/**
 * The events already recorded for a run.
 *
 * Bounded by the backend's recent-event ring, so this is "what has arrived so far" rather than the
 * whole transcript. A caller that needs the complete stream reads it from the stored run.
 */
export async function subscribeTelemetry(runId: string): Promise<TelemetryEvent[]> {
  return invoke<TelemetryEvent[]>("subscribe_telemetry", { run_id: runId });
}

/**
 * A run's complete recording, read back out of the append-only store.
 *
 * Not `subscribeTelemetry`. That reads a bounded in-memory ring scoped to the current session, and
 * says so: it exists to close the gap between subscribing to live events and the first one arriving. A
 * run from a previous session is not in that ring at all, and in this one its oldest events have been
 * evicted — so an episode recorded before a restart, or simply a long one, could be replayed from the
 * middle or not at all.
 *
 * This reads the durable table, which is append-only and holds every event the run committed. It
 * performs no execution and holds no provider, so every field it returns is a field already on disk:
 * a replayed episode is the same episode, not a second one.
 *
 * An unknown run identifier resolves to an empty array rather than rejecting. "Nothing was recorded
 * under that name" is the true answer for a run that never started and for a recording that is
 * missing, and the store cannot tell those apart — so neither does this.
 */
export async function getRunTelemetry(runId: string): Promise<TelemetryEvent[]> {
  return invoke<TelemetryEvent[]>("get_run_telemetry", { run_id: runId });
}

/** Per-event notifications from a benchmark run. */
export const RUN_TELEMETRY_EVENT = "probe-lab/telemetry";
/** A run reached a terminal state and was committed. */
export const RUN_COMPLETE_EVENT = "probe-lab/run-complete";
/** A run failed before producing a stored result. */
export const RUN_ERROR_EVENT = "probe-lab/run-error";
/**
 * A run the operator stopped.
 *
 * Its own event rather than a `run-error` carrying a message. A stop is a correct terminal state — the
 * lifecycle row is written `interrupted`, with no score, because a partial episode produced no
 * measurement — and a viewer that can only tell a stop from a fault by matching on English prose will
 * eventually call one the other. The turns the run did commit are still on disk and still replayable.
 */
export const RUN_CANCELLED_EVENT = "probe-lab/run-cancelled";

/**
 * The Arena's typed catalog and match view.
 *
 * Every field here comes from the engine's `play_match` via a projection. There is no score, no
 * "performance" number, and no client-side board state — see `ArenaPanel.tsx` for why.
 */

export type ArenaGameSummary = {
  id: string;
  name: string;
  description: string;
  width: number;
  height: number;
  maxPlies: number;
};

export type ArenaAgentSummary = {
  id: string;
  name: string;
  description: string;
  categoryLabel: string;
  /** Whether this policy can be seated as the arena's reference opponent. */
  canBeOpponent: boolean;
};

/**
 * How a match ended, in the backend's four-way vocabulary.
 *
 * `no_result` is distinct from `loss` on purpose: a harness failure is not something a policy lost.
 */
export type ArenaOutcome = "win" | "draw" | "loss" | "no_result";

export type GameCell = "empty" | "first" | "second";

/**
 * A connect-four cell. Same three cases as tic-tac-toe — the games differ in shape, not vocabulary.
 */
export type ConnectFourCell = "empty" | "first" | "second";

/**
 * A recorded board.
 *
 * The two implemented games are typed rather than left as a string-tagged `unknown`, because the
 * frontend has to index these and a wrong index renders a board nobody played.
 *
 * `connect_four` is **column-major**: the cell at column `c`, row `r` is at index `c * rows + r`,
 * with row 0 at the bottom. `boardCell` below is the only place in the frontend that reads that index,
 * and the three-board branches are exhaustive so a new game cannot be rendered as a tic-tac-toe.
 */
export type GameBoardView =
  | { game: "tic_tac_toe"; state: { cells: GameCell[] } }
  | {
      game: "connect_four";
      state: { columns: number; rows: number; cells: ConnectFourCell[] };
    }
  | { game: string; state?: unknown };

export type PlyView = {
  ply: number;
  side: "first" | "second";
  /**
   * The board as a tagged union, because the backend's `GameBoard` is one. The serde wire format
   * uses adjacent tagging (`tag = "game", content = "state"`), so cells live under `state`.
   */
  board: GameBoardView;
  /** Adjacent-tagged move: `{ game: "tic_tac_toe", move: 3 }`. */
  played: { game?: string; move?: unknown; [key: string]: unknown } | null;
};

export type IllegalMoveView = {
  ply: number;
  /** Absent when the failure did not identify a move. Never a guess. */
  attempted: { game?: string; move?: unknown; [key: string]: unknown } | null;
  reason: string;
};

/**
 * What one turn cost and who played it.
 *
 * `usage` and `promptTemplateVersion` are `null` rather than zero or empty when absent, because a
 * policy seat genuinely has no tokens and a provider that does not report them is a different fact
 * again. Reporting either as zero would make "cost nothing" indistinguishable from "unknown".
 */
export type TurnTelemetry = {
  ply: number;
  side: "first" | "second";
  /** Whether the seat was a shipped policy or a real model. */
  seat: "reference_policy" | "model";
  seatLabel: string;
  elapsedMs: number;
  usage: { promptTokens: number | null; completionTokens: number | null; totalTokens: number | null } | null;
  promptTemplateVersion: string | null;
  /** The model's own reply, sanitized and bounded by the backend. */
  responseText: string | null;
  failure: { kind: string; message: string; httpStatus: number | null } | null;
  /**
   * The board this seat was actually shown.
   *
   * Carried so the panel can state the information a seat received rather than implying it. Not
   * currently rendered — it is here for verification, and a future "what did it see" inspector can
   * use it without a backend change.
   */
  requestBoard: GameBoardView;
};

export type ArenaMatchView = {
  matchId: string;
  game: string;
  gameName: string;
  outcome: ArenaOutcome;
  testedAgent: string;
  opponentAgent: string;
  /** Whether each seat was a shipped policy or a real model. */
  /**
   * 
ull for recordings written before the seat was recorded.
   *
   * Unknown, and deliberately not defaulted to a concrete kind: guessing would assert something about
   * how a historical match was played, which is the invention audit finding B4 exists to stop.
   */
  testedSeat: "reference_policy" | "model" | null;
  /** Unknown for recordings written before the seat was recorded. See `testedSeat`. */
  opponentSeat: "reference_policy" | "model" | null;
  testedSide: "first" | "second";
  seed: number;
  plies: number;
  finalBoard: GameBoardView;
  winner: "first" | "second" | null;
  /**
   * The cells the winner completed a line on, as `[column, row]` coordinates.
   *
   * Produced by the engine's own winning-line rule, never by the renderer. `null` for a draw, a loss,
   * and any game without lines — absent rather than empty, so a panel cannot draw a line nobody
   * completed.
   *
   * A list of pairs rather than a fixed triple because connect four needs four cells and tic-tac-toe
   * three; truncating a four to three would highlight the wrong cells, which is worse than none.
   */
  winningLine: [number, number][] | null;
  illegalMoves: IllegalMoveView[];
  history: PlyView[];
  turns: TurnTelemetry[];
  timeline: { sequence: number; code: string; severity: string; detail: string }[];
  failure: string | null;
  /** Whether the operator stopped this match. Distinct from a failure. */
  cancelled: boolean;
};

/** One row of the match archive: what the history list draws without decoding a full recording. */
export type ArenaMatchSummary = {
  matchId: string;
  game: string;
  outcome: ArenaOutcome;
  testedAgent: string;
  opponentAgent: string;
  testedSide: "first" | "second";
  seed: number;
  plies: number;
  recordedUnixMs: number;
  /** The recording's own digest, so a reopened match can be checked against what was stored. */
  digest: string;
};

export type ArenaCommandError = {
  code: string;
  message: string;
};

/**
 * One seat in a match request.
 *
 * Tagged because a policy and a model are different capabilities: a policy is validated against the
 * backend's registry, a model against a provider configuration. An untagged string would make a typo
 * and an unreachable provider indistinguishable at the type level.
 */
export type ArenaSeatRequest =
  | { kind: "policy"; id: string }
  | { kind: "model"; label: string; config: ModelRunConfig };

export type ArenaMatchRequest = {
  game: string;
  testedAgent: ArenaSeatRequest;
  opponentAgent: ArenaSeatRequest;
  testedSide: "first" | "second";
  seed: number;
};

export async function listArenaGames(): Promise<ArenaGameSummary[]> {
  return invoke<ArenaGameSummary[]>("list_arena_games");
}

export async function listArenaAgents(game: string): Promise<ArenaAgentSummary[]> {
  return invoke<ArenaAgentSummary[]>("list_arena_agents", { game });
}

export async function latestArenaMatch(): Promise<ArenaMatchView | null> {
  return invoke<ArenaMatchView | null>("latest_arena_match");
}

export async function playArenaMatch(request: ArenaMatchRequest): Promise<ArenaMatchView> {
  return invoke<ArenaMatchView>("play_arena_match", { request });
}

/**
 * Asks a running match to stop at its next ply boundary.
 *
 * Resolves `false` when nothing with that identifier is running — a finished match is not an error,
 * but it is not a success either, and the stop button must be able to say so.
 */
export async function cancelArenaMatch(matchId: string): Promise<boolean> {
  return invoke<boolean>("cancel_arena_match", { match_id: matchId });
}

/** A live turn, emitted as the engine applies each ply. */
export const ARENA_TURN_EVENT = "probe-lab/arena-turn";
/** The finished match, emitted once it reaches a terminal state. */
export const ARENA_MATCH_EVENT = "probe-lab/arena-match";

/**
 * That a single Arena match has been accepted, before it has played a turn.
 *
 * Audit finding B5. The match identifier used to arrive with the first completed turn, which was too
 * late for two things the product needs immediately:
 *
 * * cancelling a match whose opening provider call is still outstanding, which is most of a real
 *   model's first turn;
 * * drawing a live board, which the panel builds by folding turns into a match that already exists.
 *
 * Deliberately separate from {@link ARENA_MATCH_EVENT}: that channel carries the terminal recording, and
 * a batch plays matches too, so a start event on the terminal channel would install a batch's match as
 * the live single-match view.
 */
export const ARENA_MATCH_STARTED_EVENT = "probe-lab/arena-match-started";

export type ArenaMatchStartedEnvelope = {
  matchId: string;
  /** The game identifier, so the panel can title an empty board correctly. */
  game: string;
  /** The engine's own opening position, before any turn. Never a frontend guess. */
  openingBoard: GameBoardView;
};

/**
 * Progress of a running batch.
 *
 * Every event carries the running total rather than a delta, so the panel never accumulates its own
 * progress and cannot drift from what the backend actually played.
 */
export const ARENA_BATCH_EVENT = "probe-lab/arena-batch";

export type ArenaTurnEnvelope = {
  matchId: string;
  turn: TurnTelemetry;
};

/**
 * The stored match archive, newest first.
 *
 * Reads the durable archive rather than the session's in-memory list, so a match played before the
 * last restart is still there. A summary is enough to draw a history row; the full recording is
 * fetched by identifier when a match is opened.
 */
export async function listArenaMatches(limit = 50): Promise<ArenaMatchSummary[]> {
  return invoke<ArenaMatchSummary[]>("list_arena_matches", { limit });
}

/** One stored match in full, by identifier. */
export async function getArenaMatch(matchId: string): Promise<ArenaMatchView> {
  return invoke<ArenaMatchView>("get_arena_match", { match_id: matchId });
}

/* ------------------------------------------------------------------ batch statistics */

/**
 * A request to play a sequence of matches.
 *
 * The same shape as a single match plus a count, because a batch is not a different kind of thing: it
 * is the same match repeated, with the tested seat taking the other side on alternate runs.
 */
export type ArenaBatchRequest = {
  game: string;
  testedAgent: ArenaSeatRequest;
  opponentAgent: ArenaSeatRequest;
  /** Bounded by the backend to 1..=50. */
  matches: number;
  seed: number;
  /**
   * Whether the tested seat alternates sides.
   *
   * Left as an explicit choice rather than defaulted here, because a batch that quietly defaulted to one
   * side would measure a handicapped seat and present the result as if it did not.
   */
  alternateSides: boolean;
};

/** How a batch is doing, emitted after every match. */
export type ArenaBatchProgress = {
  batchId: string;
  completed: number;
  requested: number;
  matchId: string;
  outcome: string;
  stopped: boolean;
};

/**
 * How a group's decided matches fell.
 *
 * `noResult` is a fourth bucket and is never folded into `loss`. A cancelled match, a provider that
 * refused the connection, and a model that was never installed are all `noResult`; none of them is a
 * thing the tested seat lost, and counting them as defeats would turn an infrastructure failure into a
 * result on the record.
 */
export type OutcomeCounts = {
  win: number;
  draw: number;
  loss: number;
  noResult: number;
};

/**
 * The outcome distribution and what follows from it.
 *
 * Two win rates, because they answer different questions. `winRate` is wins over matches that produced
 * a result of any kind, so a draw sits in the denominator and not the numerator. `decisiveWinRate`
 * excludes draws entirely, which is the closer reading when a pairing draws often. Collapsing them to
 * one number would answer neither question.
 */
export type DecisionStatistics = {
  outcomes: OutcomeCounts;
  winRate: WilsonInterval;
  decisiveWinRate: WilsonInterval;
  /** Matches the operator stopped. Every one is a `noResult`, but not every `noResult` is this. */
  cancelled: number;
  /** Matches that recorded a failure reason. */
  withFailure: number;
  hasDecided: boolean;
};

export type PlyStatistics = {
  total: number;
  mean: number | null;
  min: number | null;
  max: number | null;
};

/**
 * What the matches cost.
 *
 * Averages are over measured turns only. A shipped policy genuinely has no tokens and reports no
 * per-turn latency, and averaging its silence into a model's latency would describe neither.
 */
export type CostStatistics = {
  measuredTurns: number;
  unmeasuredTurns: number;
  totalElapsedMs: number;
  meanElapsedMs: number | null;
  maxElapsedMs: number | null;
  /** `null` when nothing reported any. Distinct from a reported zero. */
  promptTokens: number | null;
  completionTokens: number | null;
  totalTokens: number | null;
  /** Turns that ran without reporting usage. */
  turnsWithoutUsage: number;
};

/**
 * How the attempted matches divide between conditional outcomes and everything else.
 *
 * Audit finding B6. Exists so a win rate can never be read without the population behind it: a seat that
 * wins every match it finishes and finishes almost none of them is a very different result from one
 * that wins three clean matches out of three attempted.
 */
export type MatchPopulation = {
  /** Every recording counted into this block. Every other count here is a subset of it. */
  attempted: number;
  /** Matches that produced a conditional game outcome: the rules decided, or the tested seat forfeited. */
  conditional: number;
  /** Of those, the ones the tested seat forfeited by being reached and answering unusably. */
  forfeited: number;
  /** Matches the harness or provider could not deliver a usable answer for. Not a game outcome. */
  infrastructureFailures: number;
  /** Matches an operator stopped. Not a game outcome and not a fault. */
  cancelled: number;
  /**
   * Recordings that predate the termination field, so the reason is genuinely unknown.
   *
   * A real bucket rather than a remainder. Guessing `decided` for these would move historical
   * infrastructure failures into the outcome population and inflate the conditional denominator.
   */
  unclassified: number;
};

/** Every statistic for one game and one pairing. */
export type MatchStatistics = {
  game: string;
  gameName: string;
  testedAgent: string;
  opponentAgent: string;
  matches: number;
  /** The recordings behind these numbers, oldest first, so the figures can be traced. */
  matchIds: string[];
  /** A digest over those recordings' own digests. */
  provenanceDigest: string;
  decisions: DecisionStatistics;
  /** The population behind `decisions`. Read the win rate with this, never on its own. */
  population: MatchPopulation;
  plies: PlyStatistics;
  illegalMoves: number;
  matchesWithIllegalMoves: number;
  costs: CostStatistics;
  /** Below the stated threshold of decided matches. Read the interval as very wide. */
  sampleIsSmall: boolean;
};

/**
 * Statistics over recorded matches, grouped by game and pairing.
 *
 * Always a list. A single combined figure across games would be the misleading global leaderboard this
 * shape exists to prevent: a tic-tac-toe win and a connect-four win are different events.
 */
export type ArenaStatistics = {
  groups: MatchStatistics[];
  matchesRead: number;
  /** Which population these figures describe. */
  population: "batch" | "archive";
  /** What was left out of that population, and why. */
  exclusions: {
    /** Recordings that exist but could not be read back. */
    unreadable: number;
    /** Their identifiers, so a reader can go and look at them. */
    unreadableMatchIds: string[];
    /** Recordings deliberately outside this population. */
    notInPopulation: number;
    /** Whether a hard ceiling stopped the read before the population was exhausted. */
    truncated: boolean;
    /** The ceiling that stopped it, when one did. */
    readCeiling: number | null;
  };
  /** Recordings with no game or pairing to group under, counted rather than dropped. */
  ungrouped: number;
};

/**
 * Plays a batch of matches, archiving each as it finishes.
 *
 * The returned statistics cover *this batch's* recordings only. Stopping mid-batch is reported as
 * `stopped` with a lower `completed`, and the matches already archived are kept.
 */
export async function runArenaBatch(
  request: ArenaBatchRequest,
): Promise<ArenaBatchReport> {
  return invoke<ArenaBatchReport>("run_arena_batch", { request });
}

export type ArenaBatchReport = {
  batchId: string;
  completed: number;
  requested: number;
  stopped: boolean;
  alternateSides: boolean;
  matchIds: string[];
  statistics: ArenaStatistics;
};

/** Stops a running batch. False when nothing with that identifier is running. */
export async function cancelArenaBatch(batchId: string): Promise<boolean> {
  return invoke<boolean>("cancel_arena_batch", { batch_id: batchId });
}

/**
 * Statistics over every recorded match.
 *
 * Reads the durable archive, so the figures survive a restart and include matches played in earlier
 * sessions.
 */
export async function arenaStatistics(): Promise<ArenaStatistics> {
  return invoke<ArenaStatistics>("arena_statistics");
}
/**
 * The Security Lab's read model.
 *
 * The vocabulary is the important part. A security probe is a *test of the boundary*, not a subject
 * under evaluation, so `isSecurityFinding` is always false: a policy gate recording an attempt and
 * denying it means the boundary held. The same recorded event is a genuine finding when a model under
 * test produces it, which is why the Brain panel reports it as a finding and this panel does not.
 */
export type BoundaryOutcome =
  | "broker_refused"
  | "policy_denied"
  | "refused_and_denied"
  | "not_attempted"
  | "failed";

export type SecurityProbeListing = {
  id: string;
  name: string;
  description: string;
};

export type SecurityProbeReport = {
  /** Always false: a probe is a test of the boundary, not a subject under evaluation. */
  isSecurityFinding: boolean;
  probeId: string;
  probeName: string;
  benchmarkId: string;
  seed: number;
  outcome: BoundaryOutcome;
  /** Snake-case integrity categories with their counts. Empty when nothing was recorded. */
  integrityEvents: [string, number][];
  /** Snake-case enforcement categories with their counts. */
  brokerRefusals: [string, number][];
  episodes: number;
  timeline: { sequence: number; code: string; severity: string; detail: string }[];
  failure: string | null;
};

export type SecurityLabReport = {
  outcomes: SecurityProbeReport[];
  /** Kept separate on purpose; there is no combined total and the panel must not invent one. */
  summary: {
    integrityViolations: number;
    brokerRefusals: number;
    violationsByCategory: Record<string, number>;
    refusalsByCategory: Record<string, number>;
    /** Illegal environment transitions. Not containment failures -- see `SecuritySummary`. */
    invalidActions: number;
  };
  benchmarkId: string;
};

export type SecurityCommandError = {
  code: string;
  message: string;
};

export async function listSecurityProbes(
  benchmarkId: string,
): Promise<SecurityProbeListing[]> {
  return invoke<SecurityProbeListing[]>("list_security_probes", {
    benchmark_id: benchmarkId,
  });
}

export async function latestSecurityReport(): Promise<SecurityLabReport | null> {
  return invoke<SecurityLabReport | null>("latest_security_report");
}

export async function runSecuritySweep(request: {
  benchmarkId: string;
  seed: number;
  episodes: number;
}): Promise<SecurityLabReport> {
  return invoke<SecurityLabReport>("run_security_sweep", { request });
}
/**
 * A deterministic reconstruction of a recorded run.
 *
 * `capability` is the field to read first. "re-executable" means every intermediate state was
 * recorded; "inspection only" means the recording is an event log and the intermediate states were
 * never captured. Presenting the second as the first would let someone trust a projection as a re-run.
 */
export type ReplayCapability = "full" | "projected_only";

export type ReplayFrame = {
  index: number;
  source: "start" | "arena_ply" | "experiment_event" | "terminal";
  label: string;
  /** Absent rather than a placeholder, so a corrupted replay cannot look intact. */
  digest: string | null;
};

export type Replay = {
  recordingId: string;
  kind: "arena_match" | "experiment";
  capability: ReplayCapability;
  frames: ReplayFrame[];
  /** Stable digest of the recording; a tampered one differs. */
  digest: string;
  /** What could not be reconstructed, stated rather than hidden. */
  limitations: string[];
};

export async function replayArenaMatch(matchId: string): Promise<Replay> {
  return invoke<Replay>("replay_arena_match", { match_id: matchId });
}

/**
 * Universal execution modes.
 *
 * Discovery verifies rather than assumes: a binary must be on `PATH`, a
 * socket must answer, a model file must exist. Anything else is reported as
 * unavailable with its reason — never as available on documentation alone.
 * Nothing here ever holds a credential; the backend sends variable names.
 */

export type ExecutionMode = "offline_local" | "coding_agent" | "guided_external" | "remote_provider";

export type ExecutorAvailability =
  | { state: "available"; evidence: string }
  | { state: "unavailable"; reason: string; remedy: string | null };

export type ExecutorDescriptor = {
  id: string;
  label: string;
  mode: ExecutionMode;
  availability: ExecutorAvailability;
  capabilities: string[];
  detail: string;
};

export type PreflightCheck = {
  name: string;
  passed: boolean;
  detail: string;
};

export type PreflightReport = {
  id: string;
  passed: boolean;
  checks: PreflightCheck[];
};

export async function listExecutors(): Promise<ExecutorDescriptor[]> {
  return invoke<ExecutorDescriptor[]>("list_executors");
}

export async function preflightExecutor(id: string): Promise<PreflightReport> {
  return invoke<PreflightReport>("preflight_executor", { id });
}

/**
 * Model identifiers a local executor currently serves.
 *
 * Read-only `GET /v1/models` against loopback. Empty means nothing to
 * select; whether no server answers or the server lists no models is the
 * discovery availability's job to say.
 */
export async function listLocalModels(executorId: string): Promise<string[]> {
  return invoke<string[]>("list_local_models", { executor_id: executorId });
}

/**
 * Disposable guest-execution support, inspected read-only.
 *
 * The verdict is always guided-only: presence of a sandbox feature is not a
 * tested boundary. Nothing here changes the host.
 */
export type GuestIsolation = {
  os: string;
  hypervisorPresent: boolean | null;
  sandboxBinaryPresent: boolean;
  sandboxFeature: "enabled" | "disabled" | "unknown";
  hypervFeature: "enabled" | "disabled" | "unknown";
  verdict: "guided_only";
  detail: string;
};

export async function inspectGuestIsolation(): Promise<GuestIsolation> {
  return invoke<GuestIsolation>("inspect_guest_isolation");
}

export async function cancelExperiment(experimentId: string): Promise<boolean> {
  return invoke<boolean>("cancel_experiment", { experiment_id: experimentId });
}

export type ExperimentExport = {
  filename: string;
  contents: string;
  format: string;
};

export async function exportExperiment(
  experimentId: string,
  format: "json" | "csv",
): Promise<ExperimentExport> {
  return invoke<ExperimentExport>("export_experiment", { experiment_id: experimentId, format });
}

/**
 * Task benchmarks: coding repair and cue memory.
 *
 * These measure tasks, not grid episodes. The evidence is the artifact a
 * policy left behind, scored by hidden checks, with a Wilson interval — and
 * every report carries its provenance, so an observed run and a manual
 * submission never look alike.
 */

export type TaskBenchmarkSummary = {
  id: string;
  name: string;
  description: string;
  version: string;
  definitionDigest: string;
  limits: string[];
  devSeeds: number;
  evalSeeds: number;
};

export type TaskReport = {
  benchmark: "coding_repair" | "memory_latch";
  definitionVersion: string;
  definitionDigest: string;
  policy: string;
  /** `observed_run` or `manual_submission`. Never omitted. */
  provenance: "observed_run" | "manual_submission";
  successes: number;
  trials: number;
  /** Null when nothing completed. Not zero. */
  successRate: number | null;
  intervalLower: number;
  intervalUpper: number;
  meanPartialCredit: number;
  seeds: number[];
  taskSuccess: boolean[];
  executionNotes: string[];
};

export async function listTaskBenchmarks(): Promise<TaskBenchmarkSummary[]> {
  return invoke<TaskBenchmarkSummary[]>("list_task_benchmarks");
}

export async function runTaskEval(request: {
  benchmarkId: string;
  /**
   * `reference-optimal` or `reference-baseline`. `cli:<agent-id>` shapes
   * parse but are refused by the backend: automatic agent execution cannot be
   * isolated on this host, so installed agents are measured via guided
   * packages instead.
   */
  policy: string;
  /** Empty means the benchmark's evaluation pool. */
  seeds: number[];
  timeoutSeconds: number;
}): Promise<TaskReport> {
  return invoke<TaskReport>("run_task_eval", { request });
}

export type GuidedPackageView = {
  formatVersion: number;
  benchmarkId: string;
  benchmarkVersion: string;
  seed: number;
  instructions: string;
  files: Record<string, string>;
  submissionRequirements: string[];
};

export async function guidedTaskPackage(
  benchmarkId: string,
  seed: number,
): Promise<GuidedPackageView> {
  return invoke<GuidedPackageView>("guided_task_package", { benchmark_id: benchmarkId, seed });
}

/** Where a generated package was written. */
export type SavedPackage = {
  directory: string;
  fileCount: number;
};

/**
 * Writes a generated package into a ProbeLab-owned folder under Documents.
 *
 * The destination is chosen by the backend from the benchmark and seed, never by the caller: the
 * package's file names come from the benchmark definition, so a caller-supplied path would make
 * every one of them a traversal candidate. Task inputs only — hidden checks and credentials are
 * never written.
 */
export async function saveGuidedPackage(
  benchmarkId: string,
  seed: number,
): Promise<SavedPackage> {
  return invoke<SavedPackage>("save_guided_package", {
    benchmark_id: benchmarkId,
    seed,
  });
}

export async function scoreGuidedSubmission(submission: {
  benchmarkId: string;
  seed: number;
  artifacts: Record<string, string>;
  log: string;
}): Promise<TaskReport> {
  return invoke<TaskReport>("score_guided_submission", { submission });
}