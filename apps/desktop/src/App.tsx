import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { AppShell } from "./AppShell";
import { BrandLockup } from "./Brand";
import { ComparisonPanel } from "./ComparisonPanel";
import { SettingsPanel } from "./SettingsPanel";
import { AgentCatalog } from "./AgentCatalog";
import { BenchmarkCatalog as BenchmarkCatalogPanel } from "./BenchmarkCatalog";
import {
  BRAIN_EVENT,
  BRAIN_MODEL_EVENT,
  brainPanel,
  listAgents,
  listBenchmarkDetails,
  runExperiment,
  type AgentSummary,
  type BenchmarkDetail,
  type BrainReadModel,
  type ModelRunConfig,
  type OpenAiCompatibleModelRunConfig,
} from "./api";
import { BrainPanel } from "./BrainPanel";
import { ArenaPanel } from "./ArenaPanel";
import { ExperimentHistory } from "./ExperimentHistory";
import { SecurityPanel } from "./SecurityPanel";
import { ReplayPanel } from "./ReplayPanel";
import { ReadinessPanel } from "./ReadinessPanel";
import { TasksPanel } from "./TasksPanel";
import { readSettings, replayArenaMatch, type Replay } from "./api";
import {
  cancelExperiment,
  exportExperiment,
  getExperiment,
  listExecutors,
  listStoredExperiments,
  listLocalModels,
  type ExecutorDescriptor,
  type StoredExperiment,
} from "./api";
import {
  latestSecurityReport,
  listSecurityProbes,
  runSecuritySweep,
  type SecurityLabReport,
  type SecurityProbeListing,
} from "./api";
import {
  latestArenaMatch,
  listArenaAgents,
  listArenaGames,
  listArenaMatches,
  getArenaMatch,
  playArenaMatch,
  type ArenaAgentSummary,
  type ArenaGameSummary,
  type ArenaMatchSummary,
  type ArenaMatchView,
} from "./api";

type BenchmarkSummary = {
  benchmark_id: string;
  benchmark_version: string;
  title: string;
  description: string;
  capability_dimension: string;
  seed: number;
  max_turns: number;
  metadata: Record<string, string>;
};

type Coordinate = {
  x: number;
  y: number;
};

type GridSize = {
  width: number;
  height: number;
};

type TileKind = "safe" | "obstacle" | "danger" | "reward";
type Action = "UP" | "DOWN" | "LEFT" | "RIGHT" | "WAIT";
type ActionKind = "moved" | "waited" | "invalid";
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
type RequestedCapability =
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

type VisibleTile = {
  coordinate: Coordinate;
  tile: TileKind;
};

type Observation = {
  turn: number;
  position: Coordinate;
  grid_size: GridSize;
  current_tile: TileKind;
  visible_neighbors: VisibleTile[];
  max_turns: number;
  remaining_turns: number;
  available_actions: Action[];
};

type ActionOutcome = {
  action: Action | null;
  kind: ActionKind;
  position: Coordinate;
  reward: number;
  observation: Observation | null;
};

type TokenUsage = {
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

type ModelCallFailureKind =
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

type TelemetryEvent = {
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

type ProtocolAdherence = {
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

type CompletionStatus = "success" | "failure" | "turn_limit" | "model_protocol_failure";

type ReproducibilityMetadata = {
  provider: string;
  model: string;
  prompt_template_version: string;
  configuration_fingerprint: string;
  environment_version: string;
  token_usage: TokenUsage;
  protocol_failure_threshold: number;
};

type TestResult = {
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
  capability: {
    dimension: string;
    task_success: boolean;
    total_reward: number;
    turns_used: number;
    invalid_action_count: number;
    completion_status: CompletionStatus;
    protocol_adherence?: ProtocolAdherence;
  };
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

type ActionTraceEntry = {
  turn: number;
  selected_action: Action | null;
  requested_capability: RequestedCapability | null;
  model_response_text: string | null;
  parsed_action: Action | null;
  action_result_kind: ActionKind | null;
  reward: number;
  model_failure_kind: ModelCallFailureKind | null;
};

type RunMetadata = {
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

type TelemetryEnvelope = {
  run_id: string;
  event: TelemetryEvent;
};

type RunCompleteEnvelope = {
  run_id: string;
  result: TestResult;
};

type RunErrorEnvelope = {
  run_id: string;
  error: string;
};

type ProviderConnectionStatus = "connected" | "not_configured";

type ProviderStatus = {
  schema_version: number;
  provider: string;
  model: string;
  prompt_template_version: string;
  credential_source: string;
  configured: boolean;
  status: ProviderConnectionStatus;
  message: string;
};

/** IPC contract version shared with the Rust ModelRunConfig definition. */
const MODEL_RUN_CONFIG_SCHEMA_VERSION = 1;

/** The mock variant of the tagged payload, narrowed for the memo below. */
type MockModelRunConfig = Extract<ModelRunConfig, { kind: "mock" }>;

type RunState = "idle" | "starting" | "running" | "finalizing" | "complete" | "error";
type Strategy =
  | "optimal"
  | "random"
  | "greedy_mistake"
  | "invalid_action_spam"
  | "security_probe_unauthorized_access"
  | "security_probe_policy_violation"
  | "security_probe_sandbox_violation"
  | "security_probe_environment_tampering"
  | "security_probe_network_violation"
  | "forbidden_action_spam";

type StrategyOption = {
  value: Strategy;
  label: string;
  description: string;
  /** What the agent's number measures, straight from the registry's category. */
  categoryLabel: string;
  measuresAModel: boolean;
};

type GridCell = {
  x: number;
  y: number;
  known: boolean;
  tile: TileKind | null;
  agent: boolean;
};

/**
 * Strategy labels, keyed by the id the registry uses.
 *
 * The list of *choices* is no longer written here. It used to be an array literal in this file, which
 * had to be kept in step by hand with the Rust mock enum, the IPC allowlist, and the reference
 * policies — and nothing made that obligation enforceable. The failure mode was concrete: a dropdown
 * entry the backend then refused, discovered at run time.
 *
 * So the options come from the authoritative registry and this map only supplies display names for
 * the mock strategies the run command accepts. `Strategy` remains a closed union because
 * `ModelRunConfig` is a tagged Rust enum; the registry supplies the list, the type still supplies the
 * safety.
 */
const strategyLabels: Record<Strategy, string> = {
  optimal: "Optimal route",
  greedy_mistake: "Greedy mistake",
  random: "Seeded random",
  invalid_action_spam: "Boundary probe",
  security_probe_unauthorized_access: "Evaluator access probe",
  security_probe_policy_violation: "Tool policy probe",
  security_probe_sandbox_violation: "Sandbox escape probe",
  security_probe_environment_tampering: "Environment tamper probe",
  security_probe_network_violation: "Network access probe",
  forbidden_action_spam: "Forbidden action spam",
};

/**
 * Provider field placeholders, not defaults.
 *
 * These used to be the defaults — the values a run started with — and that was the bug this milestone
 * fixed. They were React state, so they were lost on every launch, and they were a second copy of
 * `ProviderDefaults::default()` in Rust, so the two could disagree about what the lab's own defaults
 * were. Now the real values come from the backend at start-up and these strings are only what an
 * empty field shows before that arrives.
 */
const BASE_URL_PLACEHOLDER = "http://127.0.0.1:11434/v1";
const API_KEY_ENV_PLACEHOLDER = "OPENAI_COMPATIBLE_API_KEY";
const MODEL_NAME_PLACEHOLDER = "local-model";

/**
 * A security sweep's own fixed shape.
 *
 * Named rather than left as bare numbers at the call site, because a bare `2` next to a stored episode
 * count invites someone to "fix" it by pointing it at the setting — which would be wrong, as the
 * comment there explains.
 */
const SECURITY_SWEEP_EPISODES = 2;
const SECURITY_SWEEP_SEED = 11;

/**
 * Sampling parameters, fixed for every connected run.
 *
 * These are real values the transport layer honours and the settings page does not expose. That is a
 * deliberate omission rather than an oversight: a temperature of 0 with a fixed token ceiling is what
 * makes a run reproducible, and making either editable would let someone produce a result that cannot
 * be compared with another without being told why. A value that must not vary does not belong in a
 * settings form.
 */
const SAMPLING_TEMPERATURE = 0;
const MAX_COMPLETION_TOKENS = 200;

/**
 * A readable message from anything a command might reject with.
 *
 * A rejected Tauri invocation arrives as a plain object — `{message}`, or a typed error payload —
 * not as an `Error`. `String(error)` on one of those is the literal text `[object Object]`, which is
 * how a real failure reached the panel as a message that said nothing about what failed. The chain is
 * walked rather than assumed, because the shape differs per command and a formatter that only knows
 * one shape is a formatter that prints `[object Object]` for the rest.
 */
const formatError = (error: unknown): string => {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string") {
    return error;
  }
  if (typeof error === "object" && error !== null) {
    const record = error as Record<string, unknown>;
    for (const key of ["message", "error", "reason", "detail"]) {
      const candidate = record[key];
      if (typeof candidate === "string" && candidate.length > 0) {
        return candidate;
      }
      // A nested error object, such as `{ error: { message } }`.
      if (candidate && typeof candidate === "object") {
        const nested = formatError(candidate);
        if (nested !== "[object Object]") {
          return nested;
        }
      }
    }
    try {
      return JSON.stringify(error);
    } catch {
      return "an unknown error";
    }
  }
  return String(error);
};

const formatAction = (action: Action | null): string =>
  action ? action.toLowerCase() : "no action";

const formatTile = (tile: TileKind | null): string => {
  if (tile === "reward") {
    return "R";
  }
  if (tile === "danger") {
    return "!";
  }
  if (tile === "obstacle") {
    return "X";
  }
  if (tile === "safe") {
    return "·";
  }
  return "?";
};

const formatStatus = (status: string): string => status.replaceAll("_", " ");

const formatFingerprint = (fingerprint: string): string =>
  fingerprint ? `${fingerprint.slice(0, 12)}…` : "—";

const formatCount = (value: number | null | undefined): string =>
  typeof value === "number" ? value.toLocaleString("en-US") : "—";

const EMPTY_USAGE: TokenUsage = {
  prompt_tokens: null,
  completion_tokens: null,
  total_tokens: null,
};

const emptyProtocolAdherence: ProtocolAdherence = {
  total_model_calls: 0,
  valid_responses: 0,
  parse_failures: 0,
  invalid_actions: 0,
  timeouts: 0,
  provider_failures: 0,
  http_failures: 0,
  transport_failures: 0,
  configuration_failures: 0,
  valid_response_rate_basis_points: 0,
};

const mergeTokenUsage = (events: TelemetryEvent[]): TokenUsage => {
  const usage: TokenUsage = { ...EMPTY_USAGE };
  const add = (value: number | null | undefined) => {
    if (typeof value === "number") {
      usage.total_tokens = (usage.total_tokens ?? 0) + value;
    }
  };
  events.forEach((event) => {
    const call = event.model_call?.usage ?? event.model_failure?.usage ?? null;
    if (!call) {
      return;
    }
    if (typeof call.prompt_tokens === "number") {
      usage.prompt_tokens = (usage.prompt_tokens ?? 0) + call.prompt_tokens;
    }
    if (typeof call.completion_tokens === "number") {
      usage.completion_tokens = (usage.completion_tokens ?? 0) + call.completion_tokens;
    }
    add(call.total_tokens);
  });
  return usage;
};

const protocolFailures = (events: TelemetryEvent[]): number =>
  events.filter((event) => (event.model_failure?.kind ?? null) !== null).length;

const mergeTelemetry = (
  current: TelemetryEvent[],
  incoming: TelemetryEvent[],
): TelemetryEvent[] => {
  const events = new Map<number, TelemetryEvent>();
  [...current, ...incoming].forEach((event) => events.set(event.sequence, event));
  return [...events.values()].sort((left, right) => left.sequence - right.sequence);
};

const buildGrid = (events: TelemetryEvent[]): GridCell[] => {
  const observations = new Map<string, { tile: TileKind; position: boolean }>();
  const latestEvent = events.at(-1) ?? null;
  const latestPosition = latestEvent?.action_outcome?.position ?? latestEvent?.observation.position ?? null;
  const gridSize = latestEvent?.observation.grid_size ?? { width: 5, height: 5 };

  events.forEach((event) => {
    const currentKey = `${event.observation.position.x},${event.observation.position.y}`;
    observations.set(currentKey, {
      tile: event.observation.current_tile,
      position: false,
    });
    event.observation.visible_neighbors.forEach((neighbor) => {
      const key = `${neighbor.coordinate.x},${neighbor.coordinate.y}`;
      observations.set(key, { tile: neighbor.tile, position: false });
    });
  });

  if (latestPosition) {
    const key = `${latestPosition.x},${latestPosition.y}`;
    const previous = observations.get(key);
    observations.set(key, {
      tile: previous?.tile ?? latestEvent?.observation.current_tile ?? "safe",
      position: true,
    });
  }

  return Array.from({ length: gridSize.height }, (_, y) =>
    Array.from({ length: gridSize.width }, (_, x) => {
      const observation = observations.get(`${x},${y}`);
      return {
        x,
        y,
        known: observation !== undefined,
        tile: observation?.tile ?? null,
        agent: observation?.position ?? false,
      };
    }),
  ).flat();
};

/**
 * Describes one committed event.
 *
 * A model protocol failure is deliberately *not* described as a denied capability: the model
 * never requested anything, it simply failed to return a policy-reviewable action. A denied
 * capability is a separate, integrity-recorded case.
 */
const eventDescription = (event: TelemetryEvent): string => {
  const failure = event.model_failure ?? null;
  if (event.broker_denial) {
    return `broker refused / ${event.broker_denial.permission}`;
  }
  if (event.requested_capability) {
    return `denied / ${formatStatus(event.requested_capability.kind)}`;
  }
  if (failure) {
    return `model failure / ${formatStatus(failure.kind)}`;
  }
  if (event.selected_action) {
    const outcome = event.action_outcome?.kind ?? "pending";
    return `${formatAction(event.selected_action)} / ${formatStatus(outcome)}`;
  }
  return "no decision";
};

/**
 * The registry table, shown on its own page.
 *
 * Which registry it renders is a prop rather than two hard-coded copies, so the headings and the
 * error handling cannot drift apart between the agents and benchmarks pages. Both are fed by the
 * backend registries, so a page cannot describe an agent or benchmark the runtime cannot build.
 */
const RegistryPanel = ({
  heading,
  eyebrow,
  count,
  countLabel,
  error,
  children,
}: {
  heading: string;
  eyebrow: string;
  count: number;
  countLabel: string;
  error: string | null;
  children: ReactNode;
}) => (
  <section className="panel catalog-panel">
    <div className="panel-heading">
      <div>
        <p className="eyebrow">{eyebrow}</p>
        <h2>{heading}</h2>
      </div>
      <span className="muted">
        {count} {countLabel}
      </span>
    </div>
    {error ? <p className="error-banner">{error}</p> : children}
  </section>
);

const App = () => {
  const [benchmarks, setBenchmarks] = useState<BenchmarkSummary[]>([]);
  const [benchmarksLoading, setBenchmarksLoading] = useState(true);
  // The authoritative registries, fetched once at start-up. Everything the pickers and the catalog
  // panels render comes from these rather than from constants in this file.
  const [registryAgents, setRegistryAgents] = useState<AgentSummary[]>([]);
  const [registryBenchmarks, setRegistryBenchmarks] = useState<BenchmarkDetail[]>([]);
  const [registryError, setRegistryError] = useState<string | null>(null);

  /**
   * The Brain's read model, and nothing derived from it locally.
   *
   * `null` until the first response, which the panel renders as its own state rather than as an empty
   * set of numbers. The whole point of the read model is that this component cannot compute a metric the
   * backend did not compute, so nothing here is derived.
   */
  const [brain, setBrain] = useState<BrainReadModel | null>(null);
  const [brainError, setBrainError] = useState<string | null>(null);
  const [brainRunning, setBrainRunning] = useState(false);
  const [executors, setExecutors] = useState<ExecutorDescriptor[]>([]);
  const [executorsError, setExecutorsError] = useState<string | null>(null);
  const [executorsLoading, setExecutorsLoading] = useState(false);
  const [historyRuns, setHistoryRuns] = useState<StoredExperiment[]>([]);
  const [historyError, setHistoryError] = useState<string | null>(null);
  /**
   * Model identifiers served by local executors, keyed by executor id.
   *
   * Read-only `GET /v1/models` against loopback. An entry that is missing was
   * never loaded; an entry that is present but empty means the server lists
   * no models — both render, because "nothing to select" and "not checked"
   * are different facts.
   */
  const [localModels, setLocalModels] = useState<Record<string, string[]>>({});
  const [localModelsError, setLocalModelsError] = useState<string | null>(null);
  const localModelsLoadingRef = useRef<Set<string>>(new Set());
  // The arena. `null` until the backend answers, and the panel renders that as its own state rather
  // than as an empty board.
  const [arenaGames, setArenaGames] = useState<ArenaGameSummary[]>([]);
  const [arenaAgents, setArenaAgents] = useState<ArenaAgentSummary[]>([]);
  const [arenaMatch, setArenaMatch] = useState<ArenaMatchView | null>(null);
  const [arenaError, setArenaError] = useState<string | null>(null);
  const [arenaPlaying, setArenaPlaying] = useState(false);
  const [arenaGame, setArenaGame] = useState("tic_tac_toe");
  const [arenaTested, setArenaTested] = useState("");
  const [arenaOpponent, setArenaOpponent] = useState("");
  const [arenaSide, setArenaSide] = useState<"first" | "second">("first");
  const [arenaSeed, setArenaSeed] = useState(7);
  /**
   * The stored match archive, and the identifier being opened.
   *
   * Loaded alongside the catalog rather than on demand, because a history list that only appears
   * after a click is not a list. A match survives a restart, so this is populated from the first
   * render of the panel rather than after the first match of the session.
   */
  const [arenaSummaries, setArenaSummaries] = useState<ArenaMatchSummary[]>([]);
  const [arenaOpeningId, setArenaOpeningId] = useState<string | null>(null);
  // The Security Lab. A sweep runs the registered probes through the real enforcement path.
  const [securityProbes, setSecurityProbes] = useState<SecurityProbeListing[]>([]);
  const [securityReport, setSecurityReport] = useState<SecurityLabReport | null>(null);
  const [securityError, setSecurityError] = useState<string | null>(null);
  const [securityRunning, setSecurityRunning] = useState(false);
  const [securityBenchmark, setSecurityBenchmark] = useState("grid-world");
  // The replay reconstruction. Loaded on demand from a recorded match.
  const [replay, setReplay] = useState<Replay | null>(null);
  const [replayError, setReplayError] = useState<string | null>(null);
  const [replayLoading, setReplayLoading] = useState(false);


  const [benchmarksError, setBenchmarksError] = useState<string | null>(null);
  const [selectedStrategy, setSelectedStrategy] = useState<Strategy>("optimal");
  // Seeded empty, then filled from the backend's stored settings. These are the values a *run* will
  // use, which is deliberately separate from the settings a *page* shows: someone may change the
  // default on the settings page and still want to override it for the run in front of them. What
  // the settings page must never be is the only place these can come from — hence the load below.
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKeyEnv, setApiKeyEnv] = useState("");
  const [connectedModel, setConnectedModel] = useState("");
  const [timeoutSeconds, setTimeoutSeconds] = useState(30);
  const [defaultEpisodes, setDefaultEpisodes] = useState(5);
  const [providerStatus, setProviderStatus] = useState<ProviderStatus | null>(null);
  const [providerStatusError, setProviderStatusError] = useState<string | null>(null);
  const [runId, setRunId] = useState<string | null>(null);
  const [runState, setRunState] = useState<RunState>("idle");
  const [runError, setRunError] = useState<string | null>(null);
  const [telemetry, setTelemetry] = useState<TelemetryEvent[]>([]);
  const [result, setResult] = useState<TestResult | null>(null);
  const [runMetadata, setRunMetadata] = useState<RunMetadata | null>(null);
  const [listenersReady, setListenersReady] = useState(false);
  const [listenerError, setListenerError] = useState<string | null>(null);
  const currentRunIdRef = useRef<string | null>(null);
  const pendingTelemetryRef = useRef<Record<string, TelemetryEnvelope[]>>({});
  const pendingCompleteRef = useRef<Record<string, RunCompleteEnvelope[]>>({});
  const pendingErrorRef = useRef<Record<string, RunErrorEnvelope[]>>({});
  const providerStatusRequestRef = useRef(0);

  /*
   * Which benchmark is selected, and the one the rest of the app reads.
   *
   * This was `benchmarks[0] ?? null`: a hardcoded first element that nothing could change. Two
   * consequences, both real. The catalog's select button renders only when `onSelect` is passed, and it
   * never was, so the most interesting benchmark in the product -- sandbox-process, the only one that
   * exercises a boundary -- could not be reached from the UI at all. And because index 0 was
   * authoritative, choosing anything else could not have changed what ran, so this was not a missing
   * feature but a broken promise: the README lists both benchmarks as implemented and only one of them
   * was reachable.
   *
   * The fallback to the first entry is deliberate and only applies until a person picks. Having a
   * default is right -- a run needs a benchmark, and an empty picker would block the product's main
   * action behind a click nobody needs. What was wrong was that the default was *the only* value
   * reachable. Once `selectedBenchmarkId` is set it is authoritative, and if that benchmark is no
   * longer registered the result is `null` rather than a silent slide back to the first entry: a run
   * should refuse to start against a benchmark the picker no longer offers.
   */
  const [selectedBenchmarkId, setSelectedBenchmarkId] = useState<string | null>(null);
  const benchmark =
    benchmarks.find((item) => item.benchmark_id === selectedBenchmarkId) ??
    (selectedBenchmarkId === null ? (benchmarks[0] ?? null) : null);

  const latestEvent = telemetry.at(-1) ?? null;
  const gridSize = latestEvent?.observation.grid_size ?? { width: 5, height: 5 };

  /**
   * The picker options, derived from the registry rather than declared here.
   *
   * The filter is the closed `Strategy` union, not a string prefix: the run command takes a tagged
   * Rust enum, so offering an id the union does not contain would fail at run time. Intersecting the
   * two means an agent can only appear if the registry has it *and* the command can accept it, which
   * is the property the old hand-written list did not have.
   */
  const strategies = useMemo<StrategyOption[]>(() => {
    return (Object.keys(strategyLabels) as Strategy[])
      .map((value) => registryAgents.find((agent) => agent.id === value))
      .filter((agent): agent is AgentSummary => agent !== undefined)
      .map((agent) => ({
        value: agent.id as Strategy,
        label: strategyLabels[agent.id as Strategy],
        description: agent.description,
        categoryLabel: agent.categoryLabel,
        measuresAModel: agent.measuresAModel,
      }));
  }, [registryAgents]);

  const strategy =
    strategies.find((item) => item.value === selectedStrategy) ?? strategies[0] ?? null;
  /*
   * Progress is a *turn* fraction, not an episode fraction, and the two were being labelled as
   * whichever suited the copy. `latestEvent.turn` over the benchmark's `max_turns` is what this
   * number has always been, and "EPISODE PROGRESS" over it was wrong: an experiment at 40% here may
   * have finished every episode it was asked for, or be on the first of thirty. The label says what
   * the number is.
   *
   * `maxTurns` is read from the benchmark that actually ran, via the selected benchmark rather than
   * `benchmarks[0]`. Against the wrong benchmark's ceiling a full run reads as 100% or 15% depending
   * on which one the array happened to hold first.
   */
  const maxTurns = benchmark?.max_turns ?? null;
  const progress =
    latestEvent && maxTurns && maxTurns > 0
      ? Math.min(100, Math.round((latestEvent.turn / maxTurns) * 100))
      : runState === "starting"
        ? 4
        : 0;
  /*
   * True only when the denominator is a real ceiling from the running benchmark. With no benchmark
   * there is no turn budget to be a fraction of, and showing a percentage against a made-up `30`
   * would be inventing a limit the backend never set.
   */
  const progressIsTurnFraction = latestEvent !== null && maxTurns !== null && maxTurns > 0;

  const grid = useMemo(() => buildGrid(telemetry), [telemetry]);
  const isRunning = runState === "starting" || runState === "running" || runState === "finalizing";
  const systemStatus = benchmarksError || listenerError
    ? "DEGRADED"
    : benchmarksLoading || !listenersReady
      ? "SYNCING"
      : isRunning
        ? "RUNNING"
        : result
          ? "READY"
          : "ONLINE";

  /**
   * Builds the tagged IPC payload for the connected provider.
   *
   * Only the declared fields are sent. There is deliberately no API-key field: the backend
   * reads the key from the environment variable named here.
   */
  const connectedModelConfig = useMemo<OpenAiCompatibleModelRunConfig>(
    () => ({
      kind: "openai_compatible",
      schema_version: MODEL_RUN_CONFIG_SCHEMA_VERSION,
      base_url: baseUrl.trim(),
      api_key_env: apiKeyEnv.trim(),
      model: connectedModel.trim(),
      timeout_seconds: timeoutSeconds,
      temperature: SAMPLING_TEMPERATURE,
      max_tokens: MAX_COMPLETION_TOKENS,
    }),
    [baseUrl, apiKeyEnv, connectedModel, timeoutSeconds],
  );

  const mockModelConfig = useMemo<MockModelRunConfig>(
    () => ({
      kind: "mock",
      schema_version: MODEL_RUN_CONFIG_SCHEMA_VERSION,
      strategy: selectedStrategy,
    }),
    [selectedStrategy],
  );

  const isConnected = providerStatus?.status === "connected";
  const liveTokenUsage = useMemo(() => mergeTokenUsage(telemetry), [telemetry]);
  const liveProtocolFailures = useMemo(() => protocolFailures(telemetry), [telemetry]);
  const durableTokenUsage = runMetadata?.token_usage ?? result?.reproducibility?.token_usage ?? null;
  const promptVersion =
    runMetadata?.prompt_template_version ??
    result?.reproducibility?.prompt_template_version ??
    "—";
  const connectedPromptVersion = providerStatus?.prompt_template_version ?? "—";
  const fingerprint =
    runMetadata?.configuration_fingerprint ?? result?.reproducibility?.configuration_fingerprint ?? "";
  const protocol = result?.capability.protocol_adherence ?? emptyProtocolAdherence;
  const latestModelText =
    latestEvent?.model_call?.response_text ?? latestEvent?.model_failure?.response_text ?? null;
  const latestModelFailure = latestEvent?.model_failure ?? null;
  const latestParsedAction = latestEvent?.action_outcome?.action ?? latestEvent?.selected_action ?? null;
  const latestEnvironmentResult = latestEvent?.action_outcome?.kind ?? null;
  const integrity = result?.integrity ?? null;
  // Two disjoint sums, and never one combined number.
  //
  // This used to be `Object.values(integrity).reduce(sum)`, which summed all ten counters -- five policy
  // violations and five broker refusals together. That is the exact figure the backend refuses to publish:
  // `api.ts` documents that `SecuritySummary` has no `total` field because "summing them produces a
  // number that rises when the sandbox works correctly". Summing it on the client re-created the number
  // the type system was designed to prevent, and it was displayed under the label INTEGRITY EVENTS, so
  // every correct broker refusal raised a figure a reader would read as a problem. The same five broker
  // values were then listed again immediately below as BROKER REFUSALS, so they were also double-counted
  // within the card.
  const policyViolationTotal = integrity
    ? integrity.unauthorized_access
      + integrity.policy_violations
      + integrity.sandbox_violations
      + integrity.environment_tampering
      + integrity.network_access_attempts
    : 0;
  const brokerIntegrityRows: [string, number][] = integrity
    ? [
        ["UNAPPROVED READS", integrity.broker_unauthorized_reads],
        ["UNAPPROVED WRITES", integrity.broker_unauthorized_writes],
        ["SPAWN BLOCKS", integrity.broker_process_spawn_blocks],
        ["NETWORK REFUSALS", integrity.broker_network_attempts],
        ["ENV INSPECT REFUSALS", integrity.broker_environment_inspect_denials],
      ]
    : [];
  const brokerIntegrityTotal = brokerIntegrityRows.reduce((total, [, value]) => total + value, 0);

  const refreshProviderStatus = useCallback(
    async (config: ModelRunConfig, requestId: number) => {
      try {
        const status = await invoke<ProviderStatus>("get_provider_status", {
          model_config: config,
        });
        if (providerStatusRequestRef.current === requestId) {
          setProviderStatus(status);
          setProviderStatusError(null);
        }
      } catch (error) {
        if (providerStatusRequestRef.current === requestId) {
          setProviderStatus(null);
          setProviderStatusError(`Provider status unavailable: ${formatError(error)}`);
        }
      }
    },
    [],
  );

  useEffect(() => {
    const requestId = providerStatusRequestRef.current + 1;
    providerStatusRequestRef.current = requestId;
    const timer = window.setTimeout(() => {
      void refreshProviderStatus(connectedModelConfig, requestId);
    }, 180);
    return () => window.clearTimeout(timer);
  }, [connectedModelConfig, refreshProviderStatus]);

  const readStoredResult = async (id: string) => {
    let stored: TestResult;
    try {
      stored = await invoke<TestResult>("get_result", { run_id: id });
    } catch (error) {
      if (currentRunIdRef.current === id) {
        setRunError(`Stored result could not be read: ${formatError(error)}`);
        setRunState("error");
      }
      return;
    }
    if (currentRunIdRef.current !== id) {
      return;
    }
    setResult(stored);
    setRunState("complete");
    setRunError(null);
    // A finished run is a stored experiment; refresh the list so it appears immediately.
    void refreshHistory();
    try {
      const metadata = await invoke<RunMetadata>("get_run_metadata", { run_id: id });
      if (currentRunIdRef.current === id) {
        setRunMetadata(metadata);
      }
    } catch {
      if (currentRunIdRef.current === id) {
        setRunMetadata(null);
      }
    }
  };

  useEffect(() => {
    let active = true;
    invoke<BenchmarkSummary[]>("list_benchmarks")
      .then((summaries) => {
        if (active) {
          setBenchmarks(summaries);
          setBenchmarksError(null);
        }
      })
      .catch((error) => {
        if (active) {
          setBenchmarksError(`Benchmark catalog unavailable: ${formatError(error)}`);
        }
      })
      .finally(() => {
        if (active) {
          setBenchmarksLoading(false);
        }
      });
    return () => {
      active = false;
    };
  }, []);

  /**
   * Load the Brain's read model and subscribe to its updates.
   *
   * Event-driven, not polled. The backend emits the complete model whenever it changes and emits
   * individual events as a run produces them, so the panel shows exactly what the orchestrator
   * recorded. A polling loop would either lag or generate load proportional to its own interval, and
   * either way it would be reconstructing backend state on a timer.
   */
  useEffect(() => {
    let active = true;
    brainPanel()
      .then((model) => {
        if (active) {
          setBrain(model);
          setBrainError(null);
        }
      })
      .catch((error) => {
        if (active) {
          setBrainError(`The orchestrator did not report its state: ${formatError(error)}`);
        }
      });

    const modelListener = listen<BrainReadModel>(BRAIN_MODEL_EVENT, ({ payload }) => {
      if (active) {
        setBrain(payload);
        setBrainError(null);
        setBrainRunning(false);
      }
    });
    const eventListener = listen(BRAIN_EVENT, () => {
      // Per-event notifications are only used to keep the running indicator honest. The model itself
      // arrives whole, so there is no client-side accumulation that could drift from the run's record.
      if (active) {
        setBrainRunning(true);
      }
    });
    return () => {
      active = false;
      void modelListener.then((unlisten) => unlisten());
      void eventListener.then((unlisten) => unlisten());
    };
  }, []);

  /**
   * Runs an experiment through the real orchestrator.
   *
   * The benchmark comes from the selected run target and the agent from the strategy picker, so the
   * experiment the Brain supervises is the one the user picked rather than a demo.
   */
  const startExperiment = useCallback(
    async (model?: ModelRunConfig) => {
      if (!benchmark) {
        return;
      }
      setBrainRunning(true);
      setBrainError(null);
      try {
        const result = await runExperiment({
          benchmarkId: benchmark.benchmark_id,
          /*
           * When a configuration is supplied the backend measures *that*, not the registry agent named
           * here. The id is still sent because a plan always names an agent and it is part of the
           * request contract; it simply is not what gets built.
           */
          agentId: selectedStrategy,
          // From the stored settings rather than a constant, so the episode count a person set on the
          // settings page is the one a run actually uses.
          episodes: defaultEpisodes,
          rootSeed: null,
          // Omitted entirely rather than sent as null: `None` on the Rust side means "the named
          // registry agent", and an explicit null would be a different thing spelled the same way.
          ...(model ? { modelConfig: model } : {}),
        });
        setBrain(result);
        // A finished run joins the history, so the history a reload shows includes this session.
        void refreshHistory();
      } catch (error) {
        setBrainError(`The experiment could not start: ${formatError(error)}`);
      } finally {
        /*
         * Cleared on both paths.
         *
         * This only ran in the `catch`, so one successful experiment left `brainRunning` true for the
         * rest of the session: the run button stayed disabled, the stop control stayed live for a run
         * that had ended, and anything conditioned on "not running" never appeared again. A run that
         * succeeded was the case that broke the panel.
         */
        setBrainRunning(false);
      }
    },
    [benchmark, selectedStrategy, defaultEpisodes],
  );

  /**
   * Asks the runner to stop after the in-flight episode finishes.
   *
   * Cooperative by design: the current episode completes first so cleanup
   * runs, and the terminal reason records the cancellation.
   */
  const cancelRun = useCallback(async () => {
    const experimentId = brain?.experiment?.experimentId;
    if (!experimentId) {
      return;
    }
    try {
      await cancelExperiment(experimentId);
    } catch (error) {
      setBrainError(`The experiment could not be stopped: ${formatError(error)}`);
    }
  }, [brain]);

  /**
   * Exports the finished experiment and downloads it.
   *
   * The file is the backend's own projection — the same model the panel
   * renders — serialized as JSON or as a per-episode CSV, never recomputed
   * locally.
   */
  const exportRun = useCallback(
    async (format: "json" | "csv") => {
      const experimentId = brain?.experiment?.experimentId;
      if (!experimentId) {
        return;
      }
      try {
        const blob = await exportExperiment(experimentId, format);
        const url = URL.createObjectURL(
          new Blob([blob.contents], { type: format === "json" ? "application/json" : "text/csv" }),
        );
        const anchor = document.createElement("a");
        anchor.href = url;
        anchor.download = blob.filename;
        document.body.appendChild(anchor);
        anchor.click();
        anchor.remove();
        URL.revokeObjectURL(url);
      } catch (error) {
        setBrainError(`The experiment could not be exported: ${formatError(error)}`);
      }
    },
    [brain],
  );

  /**
   * Discovers what this machine can run, verified just now.
   *
   * Runs on mount and on demand from the readiness route. A binary must be
   * on `PATH` and a socket must answer; anything else renders with its reason.
   */
  const refreshExecutors = useCallback(async () => {
    setExecutorsLoading(true);
    try {
      setExecutors(await listExecutors());
      setExecutorsError(null);
    } catch (error) {
      setExecutorsError(`Discovery did not complete: ${formatError(error)}`);
    } finally {
      setExecutorsLoading(false);
    }
  }, []);

  useEffect(() => {
    void refreshExecutors();
  }, [refreshExecutors]);

  /**
   * Loads the served model list for one local executor.
   *
   * Guarded per executor so a re-render cannot refetch in a loop. A failure
   * leaves any previous list in place and records the error beside the
   * executor rather than clearing the whole section: one unreachable server
   * is not evidence about the others.
   */
  const loadLocalModels = useCallback(async (executorId: string) => {
    if (localModelsLoadingRef.current.has(executorId)) {
      return;
    }
    localModelsLoadingRef.current.add(executorId);
    try {
      const ids = await listLocalModels(executorId);
      setLocalModels((previous) => ({ ...previous, [executorId]: ids }));
      setLocalModelsError(null);
    } catch (error) {
      setLocalModelsError(`Local models for ${executorId} are unavailable: ${formatError(error)}`);
    } finally {
      localModelsLoadingRef.current.delete(executorId);
    }
  }, []);

  const localModelBaseUrl = (executorId: string): string =>
    executorId === "llamacpp-local"
      ? "http://127.0.0.1:8080/v1"
      : "http://127.0.0.1:11434/v1";

  /**
   * Stored experiment history: identifiers from the durable store, loaded on
   * demand through the same read model the live panel renders.
   *
   * The full records are fetched rather than only their identifiers, because a list of opaque ids
   * cannot answer the question a returning user has — "which of these was mine, and did it pass?".
   * Everything drawn from a record is a field the backend actually recorded; a missing value is shown
   * as absent rather than as a zero or a dash that could be mistaken for a measurement.
   */
  const refreshHistory = useCallback(async () => {
    try {
      const runs = await listStoredExperiments();
      setHistoryRuns(runs);
      setHistoryError(null);
    } catch (error) {
      setHistoryError(`History is unavailable: ${formatError(error)}`);
    }
  }, []);

  useEffect(() => {
    void refreshHistory();
  }, [refreshHistory]);

  const loadHistory = useCallback(async (experimentId: string) => {
    try {
      setBrain(await getExperiment(experimentId));
      setBrainError(null);
    } catch (error) {
      setBrainError(`That experiment could not be loaded: ${formatError(error)}`);
    }
  }, []);
  /**
   * Loads a deterministic replay of the current arena match.
   *
   * The reconstruction is done by the backend from recorded frames, so this call fetches a result rather
   * than triggering a run. Stepping between frames afterwards is local view state and contacts nothing.
   */
  const loadReplay = useCallback(async () => {
    if (!arenaMatch) {
      return;
    }
    setReplayLoading(true);
    setReplayError(null);
    try {
      setReplay(await replayArenaMatch(arenaMatch.matchId));
    } catch (error) {
      setReplayError(`The recording could not be replayed: ${formatError(error)}`);
    } finally {
      setReplayLoading(false);
    }
  }, [arenaMatch]);
  /**
   * Loads the registered security probes and the most recent sweep.
   *
   * The probe list is the registry's own security-probe family, filtered in the backend. A panel that
   * kept its own list could offer a probe the runtime cannot build.
   */
  useEffect(() => {
    let active = true;
    Promise.all([listSecurityProbes(securityBenchmark), latestSecurityReport()])
      .then(([probes, latest]) => {
        if (!active) {
          return;
        }
        setSecurityProbes(probes);
        setSecurityReport(latest);
        setSecurityError(null);
      })
      .catch((error) => {
        if (active) {
          setSecurityError(`The security lab did not report its probes: ${formatError(error)}`);
        }
      });
    return () => {
      active = false;
    };
  }, [securityBenchmark]);

  /**
   * Runs a security sweep.
   *
   * Each probe executes through the same engine, broker, and sandbox path as an ordinary experiment,
   * so the report is observation rather than something the frontend composed.
   */
  const runSecurity = useCallback(async () => {
    setSecurityRunning(true);
    setSecurityError(null);
    try {
      const report = await runSecuritySweep({
        benchmarkId: securityBenchmark,
        seed: SECURITY_SWEEP_SEED,
        // Deliberately not the stored episode default. A probe's episodes are not a statistical
        // sample — a sweep asks "does the boundary hold", and running it a hundred times does not make
        // the answer more true, it just takes a hundred times as long. Exposing this as an experiment
        // setting would invite someone to tune a number that has no meaning to tune.
        episodes: SECURITY_SWEEP_EPISODES,
      });
      setSecurityReport(report);
    } catch (error) {
      setSecurityError(`The sweep could not run: ${formatError(error)}`);
    } finally {
      setSecurityRunning(false);
    }
  }, [securityBenchmark]);

  /**
   * Loads the arena catalog, the stored match archive, and the most recent match.
   *
   * The agent list is fetched per game from the backend, which is where compatibility lives. The
   * frontend keeps no list of its own, so it cannot offer a pairing the arena would refuse.
   *
   * The archive is read here rather than on demand so the history list is populated the first time the
   * panel is opened, and it is re-read after every match so a newly played one appears at the top
   * without a reload.
   */
  useEffect(() => {
    let active = true;
    Promise.all([
      listArenaGames(),
      listArenaAgents(arenaGame),
      latestArenaMatch(),
      listArenaMatches(),
    ])
      .then(([games, agents, latest, summaries]) => {
        if (!active) {
          return;
        }
        setArenaGames(games);
        setArenaAgents(agents);
        setArenaMatch(latest);
        setArenaSummaries(summaries);
        setArenaError(null);
        // Default both seats to a real policy rather than leaving the select empty, but only when the
        // current selection is not itself playable.
        setArenaTested((current) =>
          agents.some((agent) => agent.id === current) ? current : (agents[0]?.id ?? ""),
        );
        setArenaOpponent((current) =>
          agents.some((agent) => agent.id === current)
            ? current
            : (agents.find((agent) => agent.canBeOpponent)?.id ?? agents[0]?.id ?? ""),
        );
      })
      .catch((error) => {
        if (active) {
          setArenaError(`The arena did not report its catalog: ${formatError(error)}`);
        }
      });
    return () => {
      active = false;
    };
  }, [arenaGame]);

  /**
   * Plays one match.
   *
   * The whole match runs in the engine and comes back projected, so there is no local state to fall
   * out of step with the environment. Playing again replaces the displayed match with a genuinely new
   * engine run — it is never a re-render of the previous one.
   *
   * The archive is re-read afterwards so the new match appears in the history list. A failure there
   * does not fail the match: the match genuinely happened and the caller is holding it, so the
   * history is left as it was rather than the result being discarded.
   */
  const playArena = useCallback(async () => {
    setArenaPlaying(true);
    setArenaError(null);
    try {
      const result = await playArenaMatch({
        game: arenaGame,
        testedAgent: arenaTested,
        opponentAgent: arenaOpponent,
        testedSide: arenaSide,
        seed: arenaSeed,
      });
      setArenaMatch(result);
      try {
        setArenaSummaries(await listArenaMatches());
      } catch {
        // The archive is a convenience; the match itself is the result and it is already on screen.
      }
    } catch (error) {
      setArenaError(`The match could not be played: ${formatError(error)}`);
    } finally {
      setArenaPlaying(false);
    }
  }, [arenaGame, arenaTested, arenaOpponent, arenaSide, arenaSeed]);

  /**
   * Reopens a stored match into the viewer.
   *
   * The identifier travels with the match, which is what makes a recording reachable again. An earlier
   * build cleared the match whenever a selector changed, so this id was discarded and the recording —
   * still held by the backend — became impossible to reach from the interface.
   */
  const openArenaMatch = useCallback(async (matchId: string) => {
    setArenaOpeningId(matchId);
    setArenaError(null);
    try {
      setArenaMatch(await getArenaMatch(matchId));
    } catch (error) {
      setArenaError(`That match could not be opened: ${formatError(error)}`);
    } finally {
      setArenaOpeningId(null);
    }
  }, []);

  /**
   * Whether the displayed match came from a different configuration than the selectors now describe.
   *
   * The seed is deliberately excluded. Tic-tac-toe ignores it — its opening does not depend on it — and
   * the panel says so; treating a seed difference as staleness would flag every match as stale on a
   * screen where the seed is not exposed anyway.
   */
  const arenaStale = useMemo(
    () =>
      arenaMatch !== null &&
      (arenaMatch.testedAgent !== arenaTested ||
        arenaMatch.opponentAgent !== arenaOpponent ||
        arenaMatch.testedSide !== arenaSide),
    [arenaMatch, arenaTested, arenaOpponent, arenaSide],
  );

  /**
   * Returns the panel to setup.
   *
   * A view operation and nothing else: it drops the displayed match from the screen and leaves the
   * archive, the telemetry, and every stored recording untouched. Deleting a recorded match is not
   * something any control on this page does.
   */
  const clearArenaMatch = useCallback(() => {
    setArenaMatch(null);
  }, []);
  /**
   * Load the authoritative registries.
   *
   * Both are fetched together and a failure in either is surfaced rather than swallowed, because a
   * command center showing an empty agent list with no error reads as "this lab has no agents" — a
   * claim about the product that is both wrong and worse than a visible fault.
   */
  /**
   * Load the stored settings, and start from them.
   *
   * This is the point of the settings milestone. The provider fields and the episode count used to be
   * constants in this file, which meant they were lost on every launch and were a second copy of the
   * Rust defaults that could drift from them. Now the backend owns them and this component starts from
   * whatever was stored.
   *
   * A failure here is not fatal and not silent: the fields keep their placeholders and the connected
   * run button stays disabled, because a run sent with an empty base URL would be refused anyway. The
   * alternative — falling back to a hardcoded copy — would reintroduce exactly the drift this removed.
   */
  useEffect(() => {
    let active = true;
    readSettings()
      .then((view) => {
        if (!active) {
          return;
        }
        setBaseUrl(view.settings.providerDefaults.baseUrl);
        setApiKeyEnv(view.settings.providerDefaults.apiKeyEnv);
        setConnectedModel(view.settings.providerDefaults.model);
        setTimeoutSeconds(view.settings.providerDefaults.timeoutSeconds);
        setDefaultEpisodes(view.settings.experimentDefaults.episodes);
      })
      .catch(() => {
        if (active) {
          // Deliberately no fallback values: an empty field the user can see and correct is better
          // than a plausible-looking default that silently disagrees with the stored settings.
          setBaseUrl("");
          setApiKeyEnv("");
          setConnectedModel("");
        }
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    let active = true;
    Promise.all([listAgents(), listBenchmarkDetails()])
      .then(([agents, benchmarkDetails]) => {
        if (!active) {
          return;
        }
        setRegistryAgents(agents);
        setRegistryBenchmarks(benchmarkDetails);
        setRegistryError(null);
      })
      .catch((error) => {
        if (active) {
          setRegistryError(`Registries unavailable: ${formatError(error)}`);
        }
      });
    return () => {
      active = false;
    };
  }, []);

  /**
   * Keep the selection valid once the registry arrives.
   *
   * If a stored selection is no longer registered, fall back to the first registered mock rather than
   * leaving the picker pointing at an agent that cannot be built.
   */
  useEffect(() => {
    if (registryAgents.length === 0) {
      return;
    }
    const registered = new Set(registryAgents.map((agent) => agent.id));
    setSelectedStrategy((current) =>
      registered.has(current) ? current : (Object.keys(strategyLabels) as Strategy[])[0],
    );
  }, [registryAgents]);

  useEffect(() => {
    let disposed = false;
    let unlistenTelemetry: (() => void) | undefined;
    let unlistenComplete: (() => void) | undefined;
    let unlistenError: (() => void) | undefined;

    const telemetryListener = listen<TelemetryEnvelope>("probe-lab/telemetry", ({ payload }) => {
      const pending = pendingTelemetryRef.current[payload.run_id] ?? [];
      pending.push(payload);
      pendingTelemetryRef.current[payload.run_id] = pending.slice(-256);
      if (currentRunIdRef.current === payload.run_id) {
        setTelemetry((current) => mergeTelemetry(current, [payload.event]));
      }
    });
    const completeListener = listen<RunCompleteEnvelope>("probe-lab/run-complete", ({ payload }) => {
      const pending = pendingCompleteRef.current[payload.run_id] ?? [];
      pending.push(payload);
      pendingCompleteRef.current[payload.run_id] = pending;
      if (currentRunIdRef.current === payload.run_id) {
        delete pendingCompleteRef.current[payload.run_id];
        setRunState("finalizing");
        void readStoredResult(payload.run_id);
      }
    });
    const errorListener = listen<RunErrorEnvelope>("probe-lab/run-error", ({ payload }) => {
      const pending = pendingErrorRef.current[payload.run_id] ?? [];
      pending.push(payload);
      pendingErrorRef.current[payload.run_id] = pending;
      if (currentRunIdRef.current === payload.run_id) {
        delete pendingErrorRef.current[payload.run_id];
        setRunError(payload.error);
        setRunState("error");
      }
    });

    Promise.all([telemetryListener, completeListener, errorListener])
      .then(([stopTelemetry, stopComplete, stopError]) => {
        if (disposed) {
          stopTelemetry();
          stopComplete();
          stopError();
          return;
        }
        unlistenTelemetry = stopTelemetry;
        unlistenComplete = stopComplete;
        unlistenError = stopError;
        setListenersReady(true);
      })
      .catch((error) => {
        if (!disposed) {
          setListenerError(`Event handshake failed: ${formatError(error)}`);
        }
      });

    return () => {
      disposed = true;
      unlistenTelemetry?.();
      unlistenComplete?.();
      unlistenError?.();
    };
  }, []);

  const startRun = async (modelConfig: ModelRunConfig) => {
    if (!benchmark || isRunning || !listenersReady) {
      return;
    }
    setRunState("starting");
    setRunError(null);
    setResult(null);
    setRunMetadata(null);
    setTelemetry([]);
    currentRunIdRef.current = null;
    try {
      const id = await invoke<string>("run_benchmark", {
        benchmark_id: benchmark.benchmark_id,
        model_config: modelConfig,
      });
      currentRunIdRef.current = id;
      setRunId(id);
      setRunState("running");
      const replay = await invoke<TelemetryEvent[]>("subscribe_telemetry", { run_id: id });
      const buffered = pendingTelemetryRef.current[id] ?? [];
      setTelemetry(mergeTelemetry(replay, buffered.map((envelope) => envelope.event)));
      delete pendingTelemetryRef.current[id];

      const queuedError = pendingErrorRef.current[id]?.at(-1);
      if (queuedError) {
        setRunError(queuedError.error);
        setRunState("error");
        delete pendingErrorRef.current[id];
        return;
      }
      const queuedComplete = pendingCompleteRef.current[id]?.at(-1);
      if (queuedComplete) {
        delete pendingCompleteRef.current[id];
        setRunState("finalizing");
        await readStoredResult(id);
      }
    } catch (error) {
      setRunError(formatError(error));
      setRunState("error");
    }
  };

  const runBenchmark = () => startRun(mockModelConfig);
  const runConnected = () => startRun(connectedModelConfig);

  /*
   * The four states the header can actually be in, each with a tone. `stable` and `syncing` share a
   * green dot, because neither is a problem and the text beside the dot already says which is which;
   * `degraded` is a real fault and says so in colour too.
   */
  const systemTone =
    systemStatus === "DEGRADED"
      ? "degraded"
      : systemStatus === "RUNNING"
        ? "active"
        : "stable";


  /*
   * The Brain panel heads the monitoring column because it is the layer deciding what runs and
   * monitoring what happened. Everything in the controls column is something the user drives;
   * everything in this panel is state the orchestrator reports.
   */
  const viewbrainPanel = (
        <BrainPanel
          model={brain}
          error={brainError}
          running={brainRunning}
          onRun={() => void startExperiment()}
          canRun={benchmark !== null}
          /*
           * The connected model goes through the *same* multi-episode runner as the mock, so a real
           * model produces a result with an episode count and a Wilson interval rather than a single
           * episode. That is the difference between measuring a model and merely running it once, and
           * it is also what makes the result comparable against anything else later.
           */
          onRunConnected={
            isConnected ? () => void startExperiment(connectedModelConfig) : undefined
          }
          canRunConnected={isConnected}
          onCancel={() => void cancelRun()}
          canCancel={brainRunning && brain?.experiment?.experimentId !== undefined}
          onExport={(format) => void exportRun(format)}
          onCompare={() => {
            window.location.hash = "#/comparison";
          }}
canExport={brain?.experiment !== null && brain?.experiment !== undefined}
        />
  );

  /*
   * History is its own panel, not a section inside the Brain's.
   *
   * It used to be appended below the Brain inside a capped single column, so on a wide monitor the
   * run record and the list of runs that could be loaded sat at different widths and read as one
   * undifferentiated block. On a command center it belongs with the controls -- it is the only way to
   * bring an earlier run back into the reporting panels to the right.
   */
  const viewhistoryPanel = (
    <section className="panel history-panel" aria-label="Experiment history">
      <div className="panel-heading">
        <h3>Stored runs</h3>
        <span className="catalog-count">{historyRuns.length.toString().padStart(2, "0")}</span>
      </div>
      <p className="section-note">
        Every run this machine has recorded, newest first. Loading one brings it back into the
        reporting panels.
      </p>
      {historyError ? (
        <p className="panel-note">{historyError}</p>
      ) : (
        <ExperimentHistory
          runs={historyRuns}
          selectedId={brain?.experiment?.experimentId ?? null}
          loading={brainRunning}
          onOpen={(id) => void loadHistory(id)}
        />
      )}
    </section>
  );
  const viewreplayPanel = (
        <ReplayPanel
          replay={replay}
          error={replayError}
          onLoad={() => void loadReplay()}
          loading={replayLoading}
          canLoad={arenaMatch !== null}
          matchId={arenaMatch?.matchId ?? ""}
          onViewInArena={async () => {
            /*
             * Reopening by identifier, then navigating, rather than only navigating.
             *
             * The Replay panel holds a recording the Arena may not currently have on screen — it could
             * have been superseded by a later match, or loaded from the archive. Carrying the id across
             * is what guarantees the board the user lands on is the match they asked for.
             */
            if (replay?.recordingId) {
              await openArenaMatch(replay.recordingId);
            }
            window.location.hash = "#/arena";
          }}
        />
  );
  const viewsecurityPanel = (
        <SecurityPanel
          probes={securityProbes}
          report={securityReport}
          error={securityError}
          running={securityRunning}
          benchmarkId={securityBenchmark}
          onRun={() => void runSecurity()}
          onBenchmarkChange={(benchmark) => {
            setSecurityBenchmark(benchmark);
            setSecurityReport(null);
          }}
          canRun={securityProbes.length > 0}
        />
  );
  const viewarenaPanel = (
    <>
        {/*
         * The arena sits under the Brain panel because it is an evaluation environment the orchestrator
         * may later drive, not a game the Brain plays. The panel's own controls start a match; the
         * Brain does not.
         */}
        <ArenaPanel
          games={arenaGames}
          agents={arenaAgents}
          match={arenaMatch}
          matchSummaries={arenaSummaries}
          openingId={arenaOpeningId}
          error={arenaError}
          playing={arenaPlaying}
          stale={arenaStale}
          selectedGame={arenaGame}
          selectedTested={arenaTested}
          selectedOpponent={arenaOpponent}
          selectedSide={arenaSide}
          onPlay={() => void playArena()}
          onReset={clearArenaMatch}
          onOpenMatch={(matchId) => void openArenaMatch(matchId)}
          /*
           * A selector change deliberately does *not* clear the match.
           *
           * It used to, and that was the worst bug in the panel: changing a seat after a match
           * silently destroyed the recording and its identifier, which also made the replay unreachable
           * even though the backend still held both. The board now stays on screen, marked stale, and
           * the user decides when to replace it.
           */
          onGameChange={setArenaGame}
          onTestedChange={setArenaTested}
          onOpponentChange={setArenaOpponent}
          onSideChange={setArenaSide}
          canPlay={arenaTested !== "" && arenaOpponent !== "" && arenaGames.length > 0}
        />
    </>
  );
  /*
   * The control deck.
   *
   * This panel used to carry a 58px marketing headline and a readout whose value was the literal
   * string `GRID-WORLD PLANNING`. That value was the worst kind of wrong in this product: the
   * benchmark is selectable from the catalog, so selecting any other benchmark left the deck still
   * naming grid-world. Everything the product says about being explicit about what a number measures
   * is contradicted by a heading that cannot be wrong because it is not reading anything.
   *
   * It is now a run-context band, and every field is read from the selected benchmark. When no
   * benchmark is selected it says so rather than falling back to a name, because "no benchmark
   * selected" is the true state and a plausible-looking name is not.
   */
  const viewheroPanel = (
        <section className="panel deck-panel" aria-label="Run context">
          {/*
            A panel heading, not an `<h1>`.

            The page's `<h1>` is the destination title in the instrument strip, and this panel was
            carrying a second one -- two `<h1>`s on one page, so a screen-reader user navigating by
            heading level could not tell which one named the page. It also put 24px type on a
            supporting band, competing with the page title for the eye.
          */}
          <div className="panel-heading">
            <div>
              <p className="eyebrow">Control deck</p>
              <h2>Run context</h2>
            </div>
          </div>
          <div className="deck-copy">
            <p className="hero-description">
              Every run below is executed against the benchmark named here. Connected runs send only
              the model request to the configured provider; observations, telemetry and scoring stay
              in this process.
            </p>
          </div>
          <dl className="deck-readout">
            <div>
              <dt className="readout-label">Active objective</dt>
              {/*
                An absent benchmark is not a measurement, so it is not painted in the signal colour.
                In signal cyan it read as a resolved value -- the one field on the band that looked
                confidently filled in while in fact nothing had been selected.
              */}
              <dd className={`readout-value${benchmark ? "" : " is-absent"}`}>
                {benchmark?.title ?? "NO BENCHMARK SELECTED"}
              </dd>
            </div>
            <div>
              <dt className="readout-label">Capability dimension</dt>
              <dd className="readout-detail">
                {benchmark?.capability_dimension ?? "—"}
              </dd>
            </div>
            <div>
              <dt className="readout-label">Parameters</dt>
              <dd className="readout-detail mono">
                {benchmark
                  ? `v${benchmark.benchmark_version} · SEED ${benchmark.seed} · ${benchmark.max_turns} TURNS`
                  : "—"}
              </dd>
            </div>
            <div>
              <dt className="readout-label">Execution boundary</dt>
              <dd className="readout-detail">Trusted sandbox / local telemetry</dd>
            </div>
          </dl>
        </section>
  );
  const viewsuiteCard = (
          <div className="panel suite-card">
            <div className="panel-heading">
              <div>
                <p className="eyebrow">SUITE 01</p>
                <h2>Benchmark suite</h2>
              </div>
              <span className="catalog-count">{benchmarks.length.toString().padStart(2, "0")} READY</span>
            </div>
            {benchmarksLoading && <div className="state-message">Loading static catalog…</div>}
            {benchmarksError && <div className="error-message">{benchmarksError}</div>}
            {/*
              Every benchmark in the catalog, with the selected one marked.

              This card used to render exactly one row, hardcoded to index `01` and labelled SELECTED,
              so the two-row catalog showed a single entry that always claimed to be the choice. Now
              that the picker is wired, all of them are listed and the marker means something: the
              benchmark that will actually run, which is also the one the progress denominator and the
              run request are read from.

              `aria-current` carries the selection for a screen reader, so the marker is not a colour
              and a position.
            */}
            {benchmarks.length > 0 ? (
              <div className="benchmark-listing">
                {benchmarks.map((entry, index) => {
                  const isSelected = entry.benchmark_id === benchmark?.benchmark_id;
                  return (
                    <div
                      key={entry.benchmark_id}
                      className={
                        isSelected
                          ? "benchmark-choice benchmark-choice-active"
                          : "benchmark-choice"
                      }
                      aria-current={isSelected ? "true" : undefined}
                    >
                      <div className="benchmark-index">
                        {(index + 1).toString().padStart(2, "0")}
                      </div>
                      <div className="benchmark-info">
                        <strong>{entry.title}</strong>
                        <span>{entry.description}</span>
                        <div className="benchmark-meta">
                          <span>v{entry.benchmark_version}</span>
                          <span>SEED {entry.seed}</span>
                          <span>{entry.max_turns} TURNS</span>
                        </div>
                      </div>
                      <span className="ready-pill">{isSelected ? "SELECTED" : "AVAILABLE"}</span>
                    </div>
                  );
                })}
              </div>
            ) : benchmarksLoading ? null : (
              <div className="state-message">
                No benchmarks are registered, so there is nothing to run.
              </div>
            )}
            <div className="card-footer">
              <span>CATALOG SOURCE</span>
              <strong>STATIC / EMBEDDED</strong>
            </div>
          </div>
  );
  const viewmodelCard = (
          <div className="panel model-card">
            <div className="panel-heading">
              <div>
                <p className="eyebrow">MODEL STATUS</p>
                <h2>Model provider</h2>
              </div>
              <span className={`connection-pill ${isConnected ? "connected" : "not-configured"}`}>
                {isConnected ? "CONNECTED" : "NOT CONFIGURED"}
              </span>
            </div>
            <div className="model-readout">
              <div className="model-orbit" aria-hidden="true"><span /></div>
              <div>
                <span>MODEL PROVIDER</span>
                <strong>{providerStatus?.provider ?? "openai-compatible"}</strong>
              </div>
              <div>
                <span>MODEL NAME</span>
                <strong>{providerStatus?.model ?? (connectedModel || "—")}</strong>
              </div>
            </div>
            <div className="connection-readout">
              <div>
                <span>CONNECTION STATUS</span>
                <strong>{isConnected ? "CONNECTED" : "NOT CONFIGURED"}</strong>
              </div>
              <p className="field-description">
                {providerStatusError ?? providerStatus?.message ?? "Checking provider status…"}
              </p>
              <div className="provider-facts">
                <span>PROMPT / {promptVersion}</span>
                <span>CREDENTIAL / {providerStatus?.credential_source ?? "environment"}</span>
              </div>
            </div>
            <div className="field-group">
              <label htmlFor="base-url">BASE URL</label>
              <input
                id="base-url"
                type="text"
                value={baseUrl}
                spellCheck={false}
                autoComplete="off"
                onChange={(event) => setBaseUrl(event.target.value)}
                disabled={isRunning}
              />
            </div>
            <div className="field-row">
              <div className="field-group">
                <label htmlFor="model-name">MODEL NAME</label>
                <input
                  id="model-name"
                  type="text"
                  value={connectedModel}
                  spellCheck={false}
                  autoComplete="off"
                  onChange={(event) => setConnectedModel(event.target.value)}
                  disabled={isRunning}
                />
              </div>
              <div className="field-group">
                <label htmlFor="timeout-seconds">TIMEOUT (S)</label>
                <input
                  id="timeout-seconds"
                  type="number"
                  min={1}
                  max={600}
                  value={timeoutSeconds}
                  onChange={(event) => setTimeoutSeconds(Number(event.target.value) || 1)}
                  disabled={isRunning}
                />
              </div>
            </div>
            <div className="field-group">
              <label htmlFor="api-key-env">API KEY ENV VAR (NAME ONLY)</label>
              <input
                id="api-key-env"
                type="text"
                value={apiKeyEnv}
                spellCheck={false}
                autoComplete="off"
                onChange={(event) => setApiKeyEnv(event.target.value)}
                disabled={isRunning}
              />
            </div>
            <p className="field-description">
              The key is read from the environment by the backend. ProbeLab never accepts,
              stores, or displays a key value.
            </p>
            {/*
              Local models served by executors the readiness page verified.

              The identifiers come from each server's own `/v1/models` listing, read by the
              backend over loopback — never typed from memory, so a name that is not served
              cannot be selected. Choosing one fills the base URL and model name above, which
              is what the connected run measures. An executor with no models lists that fact
              rather than an empty picker.
            */}
            <div className="local-models-block">
              <h3>Local models</h3>
              {executors.filter((executor) => executor.mode === "offline_local").length === 0 ? (
                <p className="field-description">
                  No local runtime was discovered. See the readiness page for what is missing.
                </p>
              ) : (
                executors
                  .filter((executor) => executor.mode === "offline_local")
                  .map((executor) => {
                    const available = executor.availability.state === "available";
                    const ids = localModels[executor.id];
                    return (
                      <div key={executor.id} className="local-model-executor">
                        <span className="executor-label">{executor.label}</span>{" "}
                        <span className={available ? "tone-ok" : "tone-neutral"}>
                          {available ? "READY" : "NOT AVAILABLE"}
                        </span>
                        {!available && executor.availability.state === "unavailable" ? (
                          <p className="field-description">{executor.availability.reason}</p>
                        ) : null}
                        {available ? (
                          ids === undefined ? (
                            <button
                              type="button"
                              className="button-quiet"
                              onClick={() => void loadLocalModels(executor.id)}
                              disabled={isRunning}
                            >
                              LIST MODELS
                            </button>
                          ) : ids.length === 0 ? (
                            <p className="field-description">
                              The server answers but lists no models. Pull one with the
                              runtime&apos;s own tooling — ProbeLab never downloads models.
                            </p>
                          ) : (
                            <ul className="local-model-list">
                              {ids.map((id) => (
                                <li key={id}>
                                  <span className="mono">{id}</span>{" "}
                                  <button
                                    type="button"
                                    className="button-quiet"
                                    onClick={() => {
                                      setBaseUrl(localModelBaseUrl(executor.id));
                                      setConnectedModel(id);
                                    }}
                                    disabled={isRunning}
                                  >
                                    USE
                                  </button>
                                </li>
                              ))}
                            </ul>
                          )
                        ) : null}
                      </div>
                    );
                  })
              )}
              {localModelsError ? (
                <p className="brain-error" role="alert">
                  {localModelsError}
                </p>
              ) : null}
            </div>
            <div className="field-group">
              <label htmlFor="strategy">MOCK STRATEGY SELECTOR</label>
              <select
                id="strategy"
                value={selectedStrategy}
                onChange={(event) => setSelectedStrategy(event.target.value as Strategy)}
                disabled={isRunning || strategies.length === 0}
              >
                {strategies.map((item) => (
                  <option key={item.value} value={item.value}>{item.label}</option>
                ))}
              </select>
            </div>
            {strategy ? (
              <>
                <p className="field-description">{strategy.description}</p>
                {/*
                 * The category travels with the option, not just the name. A mock labelled "Sandbox
                 * escape probe" produces a count of escape attempts — a measurement of this lab's
                 * boundary, not of a model's ability. Showing the category here stops the number
                 * below from being read as a capability score.
                 */}
                <p className="field-description field-description-quiet">
                  Reports: {strategy.categoryLabel}.{" "}
                  {strategy.measuresAModel
                    ? "This is a model capability measurement."
                    : "This measures lab infrastructure, not a model."}
                </p>
              </>
            ) : (
              <p className="field-description">
                {registryError ?? "Loading the agent registry…"}
              </p>
            )}
            <div className="card-footer">
              <span>PROVIDER SEED</span>
              <strong>{benchmark?.seed ?? "—"}</strong>
            </div>
          </div>
  );
  const viewactionCard = (
          <div className="panel action-card">
            <div className="panel-heading">
              <div>
                <p className="eyebrow">Execution</p>
                <h2>Launch probe</h2>
              </div>
              <span className={`run-state ${runState}`}>{formatStatus(runState)}</span>
            </div>
            {/*
              The hardcoded "01" ordinal is removed. There is one action here and it is not the first
              of a numbered series, so the index asserted a sequence that does not exist -- and it
              forced the paragraph below it into a two-column layout with a large empty gap beside a
              two-character number.
            */}
            <p className="action-copy">
              Execute the selected model against the real embedded catalog definition.
            </p>
            <button
              className="run-button"
              type="button"
              onClick={() => void runBenchmark()}
              disabled={!benchmark || !listenersReady || isRunning}
            >
              <span>{isRunning ? "RUN IN PROGRESS" : "RUN WITH MOCK"}</span>
              <span className="button-arrow" aria-hidden="true">→</span>
            </button>
            <button
              className="run-button run-button-secondary"
              type="button"
              onClick={() => void runConnected()}
              disabled={!benchmark || !listenersReady || isRunning || !isConnected}
            >
              <span>RUN WITH CONNECTED MODEL</span>
              <span className="button-arrow" aria-hidden="true">→</span>
            </button>
            {!isConnected && (
              <p className="field-description run-hint">
                Set the API key in the configured environment variable, then the connected
                button enables. It stays visible so the missing step is explicit.
              </p>
            )}
            <div className="run-id-line">
              <span>RUN ID</span>
              <code>{runId ?? "AWAITING REQUEST"}</code>
            </div>
          </div>
  );

  /*
   * The inspector: what is true right now.
   *
   * Every field is read from state that already exists, and each one degrades to an explicit
   * "not reported" rather than a zero or a blank. That matters more here than anywhere else on the
   * Lab: this is the panel a user glances at to decide whether to trust what is coming, and a `0`
   * rendered where the backend reported nothing is indistinguishable from a run that scored nothing.
   */
  const viewprogressReadout = (
    <section className="panel inspector-card" aria-label="Run inspector">
      <div className="panel-heading">
        <div>
          <p className="eyebrow">Inspector</p>
          <h2>Run state</h2>
        </div>
        <span className={`run-state ${runState}`}>{formatStatus(runState)}</span>
      </div>

      <div className="inspector-body">
        <dl className="brain-facts">
          <div>
            <dt>Turn</dt>
            <dd>
              {latestEvent ? `${latestEvent.turn} / ${maxTurns ?? "—"}` : "no observation yet"}
            </dd>
          </div>
          <div>
            <dt>Events</dt>
            <dd>{telemetry.length}</dd>
          </div>
          <div>
            <dt>Episodes</dt>
            <dd>
              {brain?.experiment
                ? `${brain.experiment.episodesCompleted} / ${brain.experiment.episodesRequested}`
                : "not started"}
            </dd>
          </div>
          <div>
            <dt>Score</dt>
            <dd>
              {brain?.experiment?.meanSuccessFraction === null ||
              brain?.experiment?.meanSuccessFraction === undefined
                ? "nothing completed"
                : `${(brain.experiment.meanSuccessFraction * 100).toFixed(1)}%`}
            </dd>
          </div>
        </dl>

        <div className="inspector-meter">
          <div className="inspector-meter-head">
            <span>Turn budget</span>
            <span className="mono">
              {progressIsTurnFraction ? `${progress}%` : "no budget reported"}
            </span>
          </div>
          <div
            className="progress-track"
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={progress}
            aria-label="Turn progress within the benchmark's turn budget"
            aria-valuetext={
              progressIsTurnFraction
                ? `Turn ${latestEvent?.turn} of ${maxTurns}`
                : "No turn budget reported yet"
            }
          >
            <div className="progress-fill" style={{ width: `${progress}%` }} />
          </div>
        </div>

        <dl className="brain-facts">
          <div>
            <dt>Integrity violations</dt>
            <dd>{brain?.security.integrityViolations ?? "—"}</dd>
          </div>
          <div>
            <dt>Broker refusals</dt>
            <dd>{brain?.security.brokerRefusals ?? "—"}</dd>
          </div>
        </dl>

        {/*
          Refusals are counted separately from violations and must never be summed with them: the sum
          rises when the sandbox is working correctly, which is the opposite of what a security total
          is supposed to mean.
        */}
        <p className="inspector-note">
          {brain?.security.integrityViolations
            ? "The run recorded an integrity violation. The evidence is in the record below."
            : "No integrity violation recorded. Broker refusals are the boundary working."}
        </p>
      </div>
    </section>
  );

  const viewworkspaceGrid = (
        <section className="workspace-grid">
            {/*
              `is-running` gates the sweep animation. It is on the card rather than the track so the
              whole panel carries the state, and so the animation cannot run against a bar that is
              sitting at zero between runs.
            */}
            <div
              className={
                isRunning
                  ? "panel canvas-dark progress-card is-running"
                  : "panel canvas-dark progress-card"
              }
            >
            <div className="panel-heading">
              <div>
                <p className="eyebrow">LIVE RUN</p>
                <h2>Agent progress</h2>
              </div>
              <span className="live-label"><span className="pulse-dot" aria-hidden="true" />LIVE FEED</span>
            </div>
            <div className="progress-hero">
              <div>
                <span className="progress-value">{progress.toString().padStart(2, "0")}%</span>
                {/*
                  Turn progress, stated as such. With no running benchmark there is no turn ceiling to
                  be a percentage of, so the caption says that instead of inventing a denominator.
                */}
                <span className="progress-caption">
                  {progressIsTurnFraction ? "TURN PROGRESS" : "NO TURN BUDGET"}
                </span>
              </div>
              <div className="turn-readout">
                <span>TURN</span>
                <strong>{latestEvent?.turn.toString().padStart(2, "0") ?? "00"}</strong>
                <span>/ {maxTurns === null ? "--" : maxTurns.toString().padStart(2, "0")}</span>
              </div>
            </div>
            {/*
              `aria-valuetext` carries what the percentage is *of*, because a bare
              `aria-valuenow=40` on a bar labelled "Episode progress" told a screen reader 40 episodes
              out of some unnamed number, which is not a quantity that exists.
            */}
            <div
              className="progress-track"
              role="progressbar"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={progress}
              aria-label="Turn progress within the benchmark's turn budget"
              aria-valuetext={
                progressIsTurnFraction
                  ? `Turn ${latestEvent?.turn} of ${maxTurns}`
                  : "No turn budget reported yet"
              }
            >
              <div className="progress-fill" style={{ width: `${progress}%` }} />
            </div>
            <div className="progress-footer">
              <span>{latestEvent ? `EPISODE ${latestEvent.episode_id}` : "WAITING FOR FIRST OBSERVATION"}</span>
              <span>{latestEvent ? `${latestEvent.reward >= 0 ? "+" : ""}${latestEvent.reward} REWARD` : "NO REWARD YET"}</span>
            </div>
            </div>

          {/*
            The execution canvas is the one deliberately dark surface in the product.

            It holds the world grid and the progress bar -- the two things that are only meaningful
            while a run is live. Making them dark, and only them, is what makes "running" legible from
            across a desk: on a light workspace a navy panel is unmistakable, and if every panel were
            navy that signal would carry nothing.
          */}
          <div className="panel canvas-dark visualization-card">
            <div className="panel-heading">
              <div>
                <p className="eyebrow">LOCAL PERCEPTION</p>
                <h2>Agent / world view</h2>
              </div>
              <span className="grid-label">{gridSize.width} × {gridSize.height} / OBSERVED ONLY</span>
            </div>
            <div
              className="world-grid"
              role="group"
              aria-label={`${gridSize.width} by ${gridSize.height} grid showing only observed agent and adjacent tiles`}
              style={{ gridTemplateColumns: `repeat(${gridSize.width}, minmax(42px, 1fr))` }}
            >
              {grid.map((cell) => (
                <div
                  role="img"
                  className={`world-cell ${cell.known ? `known ${cell.tile}` : "unknown"} ${cell.agent ? "agent" : ""}`}
                  key={`${cell.x}-${cell.y}`}
                  aria-label={`${cell.x}, ${cell.y}: ${cell.known ? cell.tile : "unknown"}${cell.agent ? ", agent" : ""}`}
                >
                  <span>{cell.agent ? "A" : formatTile(cell.tile)}</span>
                  <small>{cell.x},{cell.y}</small>
                </div>
              ))}
            </div>
            <div className="legend">
              <span><i className="legend-swatch safe" />SAFE</span>
              <span><i className="legend-swatch reward" />REWARD</span>
              <span><i className="legend-swatch danger" />DANGER</span>
              <span><i className="legend-swatch obstacle" />OBSTACLE</span>
              <span><i className="legend-swatch unknown" />UNKNOWN</span>
            </div>
          </div>
        </section>
  );
  const viewlowerGrid = (
        <section className="lower-grid">
          <div className="panel telemetry-card">
            <div className="panel-heading">
              <div>
                <p className="eyebrow">OBSERVATION STREAM</p>
                <h2>Telemetry</h2>
              </div>
              <span className="event-count">{telemetry.length.toString().padStart(3, "0")} EVENTS</span>
            </div>
            {telemetry.length === 0 ? (
              <div className="empty-state">Telemetry will appear after the first committed event.</div>
            ) : (
              <>
                <div className="model-exchange">
                  <div className="model-exchange-head">
                    <span>LATEST MODEL RESPONSE</span>
                    <span>TURN {latestEvent?.turn ?? "—"}</span>
                  </div>
                  {latestModelFailure ? (
                    <p className="model-response failure">
                      <span className="model-failure-kind">
                        {formatStatus(latestModelFailure.kind.toUpperCase())}
                      </span>
                      {latestModelFailure.message}
                    </p>
                  ) : (
                    <p className="model-response">
                      {latestModelText ??
                        (latestEvent?.selected_action
                          ? `Native mock decision: ${formatAction(latestEvent.selected_action)}`
                          : "This provider reported no response text.")}
                    </p>
                  )}
                  <div className="exchange-facts">
                    <span>PARSED ACTION / {latestParsedAction ? formatAction(latestParsedAction) : "—"}</span>
                    <span>ENV RESULT / {latestEnvironmentResult ? formatStatus(latestEnvironmentResult) : "—"}</span>
                    <span>REWARD / {latestEvent ? `${latestEvent.reward >= 0 ? "+" : ""}${latestEvent.reward}` : "—"}</span>
                    <span>EVENTS / {telemetry.length}</span>
                    <span>TOKENS / {formatCount(liveTokenUsage.total_tokens)}</span>
                  </div>
                </div>
                <div className="telemetry-list">
                  {telemetry.slice().reverse().slice(0, 10).map((event) => (
                    <div className="telemetry-row" key={`${event.run_id}-${event.sequence}`}>
                      <span className="event-sequence">{event.sequence.toString().padStart(3, "0")}</span>
                      <div className="event-main">
                        <strong>{eventDescription(event)}</strong>
                        <span>TURN {event.turn} · POS {event.observation.position.x},{event.observation.position.y}</span>
                      </div>
                      <div className="event-outcome">
                        <span className={event.reward > 0 ? "positive" : event.invalid_action ? "negative" : "muted"}>
                          {event.reward > 0 ? "+" : ""}{event.reward}
                        </span>
                        <span>
                          {event.integrity_event
                            ? "DENIED"
                            : event.model_failure
                              ? formatStatus(event.model_failure.kind)
                              : event.timeout
                                ? "LIMIT"
                                : "OK"}
                        </span>
                      </div>
                    </div>
                  ))}
                </div>
              </>
            )}
            <div className="telemetry-summary">
              <span>EVENTS / {telemetry.length}</span>
              <span>MODEL FAILURES / {liveProtocolFailures}</span>
              <span>
                TOKENS / {formatCount(durableTokenUsage?.total_tokens ?? liveTokenUsage.total_tokens)}
              </span>
            </div>
            <div className="card-footer">
              <span>EVENT SOURCE</span>
              <strong>SQLITE + REPLAY BUFFER</strong>
            </div>
          </div>

          <div className="panel result-card">
            <div className="panel-heading">
              <div>
                <p className="eyebrow">EVALUATION OUTPUT</p>
                <h2>Stored result</h2>
              </div>
              {/*
                This used to read "VERIFIED", with `styles.css` colouring the V green. Nothing verified
                anything: the label was chosen on the strength of the fact that `get_result` had returned
                a row, which says the result was *stored*, not that it was checked. A word like VERIFIED
                is a claim about a verification that does not exist, and colouring its first letter made
                it look like a system badge. The honest label is what actually happened, and the digest
                printed below the card is the real integrity evidence -- reported, and not recomputed
                here.
              */}
              <span className="result-status">{result ? "STORED" : "PENDING"}</span>
            </div>
            {runError && <div className="error-message" role="alert">{runError}</div>}
            {!result && !runError && (
              <div className="empty-state result-empty">
                <span className="empty-glyph">+</span>
                <p>Run a benchmark to retrieve the immutable result from the local repository.</p>
              </div>
            )}
            {result && (
              <>
                <div className="result-hero">
                  <div>
                    <span className="result-kicker">TASK OUTCOME</span>
                    <strong className={result.capability.task_success ? "success-text" : "failure-text"}>
                      {result.capability.task_success ? "SUCCESS" : formatStatus(result.capability.completion_status)}
                    </strong>
                  </div>
                  <div className="reward-total">
                    <span>REWARD</span>
                    <strong>{result.capability.total_reward > 0 ? "+" : ""}{result.capability.total_reward}</strong>
                  </div>
                </div>
                <div className="metric-grid">
                  <div><span>TURNS USED</span><strong>{result.capability.turns_used}</strong></div>
                  <div><span>INVALID ACTIONS</span><strong>{result.capability.invalid_action_count}</strong></div>
                  <div><span>EVENTS</span><strong>{result.telemetry_event_count}</strong></div>
                  {/*
                    Policy violations and broker refusals are shown as two separate facts and never
                    added together. A policy violation means something went wrong; a broker refusal means
                    the boundary did its job. One number covering both would rise every time the sandbox
                    worked, which is the opposite of what a reader would take from it.
                  */}
                  <div>
                    <span>POLICY VIOLATIONS</span>
                    <strong>{policyViolationTotal}</strong>
                  </div>
                </div>
                {brokerIntegrityTotal > 0 && (
                  <div className="integrity-breakdown">
                    <span>BROKER REFUSALS / THE BOUNDARY HELD</span>
                    <div>
                      {brokerIntegrityRows.map(([label, value]) => (
                        <span key={label}>{label} / {value}</span>
                      ))}
                    </div>
                  </div>
                )}
                <div className="metric-grid">
                  <div><span>VALID RESPONSES</span><strong>{formatCount(protocol.valid_responses)}</strong></div>
                  <div><span>MODEL CALLS</span><strong>{formatCount(protocol.total_model_calls)}</strong></div>
                  <div>
                    <span>PROTOCOL RATE</span>
                    <strong>{(protocol.valid_response_rate_basis_points / 100).toFixed(1)}%</strong>
                  </div>
                  <div>
                    <span>TOKENS</span>
                    <strong>{formatCount(durableTokenUsage?.total_tokens ?? null)}</strong>
                  </div>
                </div>
                 <div className="result-details">
                   <span>PROVIDER / {runMetadata?.provider ?? result.reproducibility?.provider ?? "—"}</span>
                   <span>MODEL / {runMetadata?.model ?? result.model.strategy}</span>
                   <span>PROMPT / {promptVersion}</span>
                   <span>ENV / {runMetadata?.environment_version ?? result.reproducibility?.environment_version ?? "—"}</span>
                   <span>FINGERPRINT / {formatFingerprint(fingerprint)}</span>
                   <span>SEED / {result.seed}</span>
                   <span>DIGEST / {result.replay_digest.slice(0, 12)}…</span>
                 </div>
                 {runMetadata && runMetadata.action_trace.length > 0 && (
                   <div className="trace-panel">
                     <div className="trace-heading">
                       <span>ACTION TRACE</span>
                       <span>{runMetadata.action_trace.length} TURNS</span>
                     </div>
                     {runMetadata.action_trace.slice(-8).reverse().map((entry) => (
                       <div className="trace-row" key={entry.turn}>
                         <span>{entry.turn.toString().padStart(2, "0")}</span>
                         <span>
                           {entry.model_failure_kind
                             ? formatStatus(entry.model_failure_kind)
                             : entry.parsed_action
                               ? formatAction(entry.parsed_action)
                               : "—"}
                         </span>
                         <span>{entry.action_result_kind ? formatStatus(entry.action_result_kind) : "—"}</span>
                         <span>{entry.reward >= 0 ? `+${entry.reward}` : entry.reward}</span>
                       </div>
                     ))}
                   </div>
                 )}
              </>
            )}
            <div className="card-footer">
              <span>REPOSITORY</span>
              <strong>LOCAL SQLITE</strong>
            </div>
          </div>
        </section>
  );

return (
    <div className="app-shell">
      {/*
          Every panel below is a destination, not another section of one long page.

          Splitting them is not a layout preference. Stacked, the run controls sat above the
          registries and the security probes, so the two questions a user actually arrives with --
          "what does this number measure?" and "what can I run?" -- required scrolling past four
          unrelated panels to answer. Each page now answers one question, and the sidebar states
          which one.

          The command center is additionally split into two zones: the left column holds what you
          set up (run context, launch controls, stored runs) and the right holds what the run reports.
          Everything used to stack full-width in one reading order, which meant scrolling past four
          panels to reach the live state mid-run and pushed the launch controls out of reach as the
          telemetry stream grew. The controls column is sticky so reaching the run button never costs
          a scroll.

          The panels are the same components, fed by the same state, as before. Routing changed
          where they appear and nothing about what they compute.
        */}
      {/*
        The toolbar is owned by the shell now.

        It used to live here, above the routed content, which meant the page title and the live system
        read sat in a separate band from the navigation they belong with, and the shell had to fake a
        header of its own. The shell renders one toolbar with the title, the purpose, and the system
        state together, so this passes the two values it already owns and gets out of the way.
      */}
      <AppShell
        systemStatus={systemStatus}
        systemTone={systemTone}
        routes={{
          readiness: (
            <div className="route-stack">
              <ReadinessPanel
                executors={executors}
                error={executorsError}
                loading={executorsLoading}
                onRefresh={() => void refreshExecutors()}
              />
            </div>
          ),
          lab: (
            <div className="lab-console">
              {/*
                The workstation: three columns and a dock.

                LEFT   what you set up -- the benchmark, the transport, and the runs already stored.
                CENTRE what you do -- the start control, and the live canvas it starts.
                RIGHT  what is true right now -- the model, the objective, and the evidence so far.
                DOCK   the record -- supervision, telemetry, results. Full width, because a timeline
                       that has to fit in a column is a timeline nobody reads.

                It used to be two full-height columns, which put the start button in the same column as
                the run context and buried the live canvas under everything else. The brief for a tool
                like this is "configure, run, watch" and those are three different moments, so they are
                three different columns.
              */}
              <div className="lab-controls">
                {viewheroPanel}
                {viewhistoryPanel}
              </div>
              <div className="lab-stage">
                {viewactionCard}
                {viewworkspaceGrid}
              </div>
              <aside className="lab-inspector" aria-label="Run inspector">
                {viewmodelCard}
                {viewprogressReadout}
              </aside>
              <div className="lab-dock">{viewbrainPanel}</div>
              <div className="lab-dock">{viewlowerGrid}</div>
            </div>
          ),
          agents: (
            <div className="route-stack">
              <RegistryPanel
                eyebrow="REGISTRY"
                heading="Agents"
                count={registryAgents.length}
                countLabel="AGENTS"
                error={registryError}
              >
                <AgentCatalog agents={registryAgents} />
              </RegistryPanel>
            </div>
          ),
          benchmarks: (
            <div className="route-stack">
              <RegistryPanel
                eyebrow="REGISTRY"
                heading="Benchmarks"
                count={registryBenchmarks.length}
                countLabel="BENCHMARKS"
                error={registryError}
              >
                <BenchmarkCatalogPanel
        benchmarks={registryBenchmarks}
        onSelect={(entry) => setSelectedBenchmarkId(entry.id)}
        selectedId={benchmark?.benchmark_id ?? null}
      />
              </RegistryPanel>
              <section className="route-single" aria-label="Selected benchmark">
                {viewsuiteCard}
              </section>
            </div>
          ),
          tasks: (
            <div className="route-stack">
              <TasksPanel />
            </div>
          ),
          comparison: (
            <div className="route-stack">
              <ComparisonPanel />
            </div>
          ),
          settings: (
            <div className="route-stack">
              <SettingsPanel />
            </div>
          ),
          providers: (
            <div className="route-stack">
              <section className="route-single" aria-label="Model provider">
                {viewmodelCard}
              </section>
            </div>
          ),
          arena: <div className="route-stack">{viewarenaPanel}</div>,
          security: <div className="route-stack">{viewsecurityPanel}</div>,
          replay: <div className="route-stack">{viewreplayPanel}</div>,
        }}
      />
    </div>
  );
};

export default App;
