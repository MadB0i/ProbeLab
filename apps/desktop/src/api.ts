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
  lifecycle: string;
  terminalReason: string | null;
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
  containmentFailures: number;
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
  return invoke<BrainReadModel>("get_experiment", { experimentId });
}

export async function listExperiments(): Promise<string[]> {
  return invoke<string[]>("list_experiments");
}

export async function runExperiment(request: {
  benchmarkId: string;
  agentId: string;
  episodes: number;
  rootSeed: number | null;
}): Promise<BrainReadModel> {
  return invoke<BrainReadModel>("run_experiment", { request });
}

/** Per-event notifications, emitted as the runner produces them. */
export const BRAIN_EVENT = "probe-lab/brain-event";
/** The complete read model, emitted when it changes. */
export const BRAIN_MODEL_EVENT = "probe-lab/brain";
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

export type PlyView = {
  ply: number;
  side: "first" | "second";
  /**
   * The board as a tagged union, because the backend's `GameBoard` is one. The panel switches on
   * `game` rather than assuming a shape, so an unimplemented game cannot render as a broken board.
   */
  board:
    | { game: "tic_tac_toe"; cells: GameCell[] }
    | { game: string; cells?: unknown };
  played: { tic_tac_toe?: number; [key: string]: unknown } | null;
};

export type IllegalMoveView = {
  ply: number;
  /** Absent when the failure did not identify a move. Never a guess. */
  attempted: { tic_tac_toe?: number; [key: string]: unknown } | null;
  reason: string;
};

export type ArenaMatchView = {
  matchId: string;
  game: string;
  gameName: string;
  outcome: ArenaOutcome;
  testedAgent: string;
  opponentAgent: string;
  testedSide: "first" | "second";
  seed: number;
  plies: number;
  finalBoard: PlyView["board"];
  winner: "first" | "second" | null;
  illegalMoves: IllegalMoveView[];
  history: PlyView[];
  timeline: { sequence: number; code: string; severity: string; detail: string }[];
  failure: string | null;
};

export type ArenaCommandError = {
  code: string;
  message: string;
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

export async function playArenaMatch(request: {
  game: string;
  testedAgent: string;
  opponentAgent: string;
  testedSide: "first" | "second";
  seed: number;
}): Promise<ArenaMatchView> {
  return invoke<ArenaMatchView>("play_arena_match", { request });
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
    containmentFailures: number;
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
    benchmarkId,
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
  return invoke<Replay>("replay_arena_match", { matchId });
}