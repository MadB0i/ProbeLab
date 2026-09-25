import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useMemo, useRef, useState } from "react";

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
  | "network_violation";
type RequestedCapability =
  | { kind: "evaluator_access"; target?: null }
  | { kind: "tool_use"; target: string }
  | { kind: "sandbox_escape"; target?: null }
  | { kind: "environment_mutation"; target: string }
  | { kind: "network_access"; target: string };

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
    completion_status: "success" | "failure" | "turn_limit";
  };
  integrity: {
    unauthorized_access_attempts: number;
    policy_violations: number;
    sandbox_violations: number;
    environment_tampering: number;
    network_violations: number;
  };
  telemetry_event_count: number;
  replay_digest: string;
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
  | "security_probe_network_violation";

type StrategyOption = {
  value: Strategy;
  label: string;
  description: string;
};

type GridCell = {
  x: number;
  y: number;
  known: boolean;
  tile: TileKind | null;
  agent: boolean;
};

const strategies: StrategyOption[] = [
  {
    value: "optimal",
    label: "Optimal route",
    description: "Use only local safe and reward observations.",
  },
  {
    value: "greedy_mistake",
    label: "Greedy mistake",
    description: "Pause once, then follow the local safe route.",
  },
  {
    value: "random",
    label: "Seeded random",
    description: "Select a valid local action with a fixed seed.",
  },
  {
    value: "invalid_action_spam",
    label: "Boundary probe",
    description: "Repeatedly test an invalid boundary transition.",
  },
  {
    value: "security_probe_unauthorized_access",
    label: "Evaluator access probe",
    description: "Request denied evaluator access.",
  },
  {
    value: "security_probe_policy_violation",
    label: "Tool policy probe",
    description: "Request a disallowed tool.",
  },
  {
    value: "security_probe_sandbox_violation",
    label: "Sandbox escape probe",
    description: "Request an escape from the sandbox.",
  },
  {
    value: "security_probe_environment_tampering",
    label: "Environment tamper probe",
    description: "Request direct environment mutation.",
  },
  {
    value: "security_probe_network_violation",
    label: "Network access probe",
    description: "Request network access.",
  },
];

const formatError = (error: unknown): string => {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
};

const formatAction = (action: Action | null): string =>
  action ? action.toLowerCase() : "denied capability";

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

const eventDescription = (event: TelemetryEvent): string => {
  if (event.selected_action) {
    const outcome = event.action_outcome?.kind ?? "pending";
    return `${formatAction(event.selected_action)} / ${formatStatus(outcome)}`;
  }
  if (event.requested_capability) {
    return `denied / ${formatStatus(event.requested_capability.kind)}`;
  }
  return "denied capability";
};

const App = () => {
  const [benchmarks, setBenchmarks] = useState<BenchmarkSummary[]>([]);
  const [benchmarksLoading, setBenchmarksLoading] = useState(true);
  const [benchmarksError, setBenchmarksError] = useState<string | null>(null);
  const [selectedStrategy, setSelectedStrategy] = useState<Strategy>("optimal");
  const [runId, setRunId] = useState<string | null>(null);
  const [runState, setRunState] = useState<RunState>("idle");
  const [runError, setRunError] = useState<string | null>(null);
  const [telemetry, setTelemetry] = useState<TelemetryEvent[]>([]);
  const [result, setResult] = useState<TestResult | null>(null);
  const [listenersReady, setListenersReady] = useState(false);
  const [listenerError, setListenerError] = useState<string | null>(null);
  const currentRunIdRef = useRef<string | null>(null);
  const pendingTelemetryRef = useRef<Record<string, TelemetryEnvelope[]>>({});
  const pendingCompleteRef = useRef<Record<string, RunCompleteEnvelope[]>>({});
  const pendingErrorRef = useRef<Record<string, RunErrorEnvelope[]>>({});

  const benchmark = benchmarks[0] ?? null;
  const latestEvent = telemetry.at(-1) ?? null;
  const gridSize = latestEvent?.observation.grid_size ?? { width: 5, height: 5 };
  const strategy = strategies.find((item) => item.value === selectedStrategy) ?? strategies[0];
  const maxTurns = benchmark?.max_turns ?? 30;
  const progress = latestEvent
    ? Math.min(100, Math.round((latestEvent.turn / maxTurns) * 100))
    : runState === "starting"
      ? 4
      : 0;
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

  const readStoredResult = async (id: string) => {
    try {
      const stored = await invoke<TestResult>("get_result", { run_id: id });
      if (currentRunIdRef.current === id) {
        setResult(stored);
        setRunState("complete");
        setRunError(null);
      }
    } catch (error) {
      if (currentRunIdRef.current === id) {
        setRunError(`Stored result could not be read: ${formatError(error)}`);
        setRunState("error");
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
        setRunState("finalizing");
        void readStoredResult(payload.run_id);
      }
    });
    const errorListener = listen<RunErrorEnvelope>("probe-lab/run-error", ({ payload }) => {
      const pending = pendingErrorRef.current[payload.run_id] ?? [];
      pending.push(payload);
      pendingErrorRef.current[payload.run_id] = pending;
      if (currentRunIdRef.current === payload.run_id) {
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

  const runBenchmark = async () => {
    if (!benchmark || isRunning || !listenersReady) {
      return;
    }
    setRunState("starting");
    setRunError(null);
    setResult(null);
    setTelemetry([]);
    currentRunIdRef.current = null;
    try {
      const id = await invoke<string>("run_benchmark", {
        benchmark_id: benchmark.benchmark_id,
        model_config: { strategy: selectedStrategy },
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

  const systemTone = systemStatus === "DEGRADED" ? "warning" : systemStatus === "RUNNING" ? "active" : "stable";

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="brand-lockup">
          <div className="brand-mark" aria-hidden="true">P//L</div>
          <div>
            <div className="brand-name">PROBE<span>//</span>LAB</div>
            <div className="brand-subtitle">AI CAPABILITY &amp; AGENT EVALUATION</div>
          </div>
        </div>
        <div className="system-status" aria-live="polite">
          <span className={`status-light ${systemTone}`} aria-hidden="true" />
          <span>SYSTEM {systemStatus}</span>
          <span className="status-divider" aria-hidden="true" />
          <span className="muted">LOCAL RUNTIME</span>
        </div>
      </header>

      <main className="main-content">
        <section className="hero-panel panel">
          <div className="hero-copy">
            <p className="eyebrow">VERTICAL SLICE / CONTROL DECK</p>
            <h1>Measure what the model<br /><span>actually sees.</span></h1>
            <p className="hero-description">
              Run a deterministic agent through a local, capability-restricted evaluation suite.
              Observations stay local. Evidence is durable.
            </p>
          </div>
          <div className="hero-readout">
            <div className="readout-label">ACTIVE OBJECTIVE</div>
            <div className="readout-value">GRID-WORLD PLANNING</div>
            <div className="readout-detail">TRUSTED SANDBOX / LOCAL TELEMETRY</div>
          </div>
        </section>

        <section className="control-grid" aria-label="Benchmark controls">
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
            {benchmark && (
              <div className="benchmark-listing">
                <div className="benchmark-row benchmark-row-active">
                  <div className="benchmark-index">01</div>
                  <div className="benchmark-info">
                    <strong>{benchmark.title}</strong>
                    <span>{benchmark.description}</span>
                    <div className="benchmark-meta">
                      <span>v{benchmark.benchmark_version}</span>
                      <span>SEED {benchmark.seed}</span>
                      <span>{benchmark.max_turns} TURNS</span>
                    </div>
                  </div>
                  <span className="ready-pill">SELECTED</span>
                </div>
              </div>
            )}
            <div className="card-footer">
              <span>CATALOG SOURCE</span>
              <strong>STATIC / EMBEDDED</strong>
            </div>
          </div>

          <div className="panel model-card">
            <div className="panel-heading">
              <div>
                <p className="eyebrow">MODEL STATUS</p>
                <h2>Deterministic mock</h2>
              </div>
              <span className="model-badge">LOCAL</span>
            </div>
            <div className="model-readout">
              <div className="model-orbit" aria-hidden="true"><span /></div>
              <div>
                <strong>deterministic-mock-v1</strong>
                <span>SEEDED PROVIDER / NO NETWORK</span>
              </div>
            </div>
            <div className="field-group">
              <label htmlFor="strategy">STRATEGY SELECTOR</label>
              <select
                id="strategy"
                value={selectedStrategy}
                onChange={(event) => setSelectedStrategy(event.target.value as Strategy)}
                disabled={isRunning}
              >
                {strategies.map((item) => (
                  <option key={item.value} value={item.value}>{item.label}</option>
                ))}
              </select>
            </div>
            <p className="field-description">{strategy.description}</p>
            <div className="card-footer">
              <span>PROVIDER SEED</span>
              <strong>{benchmark?.seed ?? "—"}</strong>
            </div>
          </div>

          <div className="panel action-card">
            <div className="panel-heading">
              <div>
                <p className="eyebrow">EXECUTION</p>
                <h2>Launch probe</h2>
              </div>
              <span className={`run-state ${runState}`}>{formatStatus(runState)}</span>
            </div>
            <div className="action-copy">
              <span className="action-index">01</span>
              <p>Execute the selected strategy against the real embedded catalog definition.</p>
            </div>
            <button
              className="run-button"
              type="button"
              onClick={() => void runBenchmark()}
              disabled={!benchmark || !listenersReady || isRunning}
            >
              <span>{isRunning ? "RUN IN PROGRESS" : "RUN BENCHMARK"}</span>
              <span className="button-arrow" aria-hidden="true">→</span>
            </button>
            <div className="run-id-line">
              <span>RUN ID</span>
              <code>{runId ?? "AWAITING REQUEST"}</code>
            </div>
          </div>
        </section>

        <section className="workspace-grid">
          <div className="panel progress-card">
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
                <span className="progress-caption">EPISODE PROGRESS</span>
              </div>
              <div className="turn-readout">
                <span>TURN</span>
                <strong>{latestEvent?.turn.toString().padStart(2, "0") ?? "00"}</strong>
                <span>/ {maxTurns.toString().padStart(2, "0")}</span>
              </div>
            </div>
            <div className="progress-track" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={progress} aria-label="Episode progress">
              <div className="progress-fill" style={{ width: `${progress}%` }} />
            </div>
            <div className="progress-footer">
              <span>{latestEvent ? `EPISODE ${latestEvent.episode_id}` : "WAITING FOR FIRST OBSERVATION"}</span>
              <span>{latestEvent ? `${latestEvent.reward >= 0 ? "+" : ""}${latestEvent.reward} REWARD` : "NO REWARD YET"}</span>
            </div>
          </div>

          <div className="panel visualization-card">
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
                      <span>{event.integrity_event ? "DENIED" : event.timeout ? "LIMIT" : "OK"}</span>
                    </div>
                  </div>
                ))}
              </div>
            )}
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
              <span className="result-status">{result ? "VERIFIED" : "PENDING"}</span>
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
                  <div><span>INTEGRITY EVENTS</span><strong>{Object.values(result.integrity).reduce((total, value) => total + value, 0)}</strong></div>
                </div>
                <div className="result-details">
                  <span>MODEL / {result.model.strategy}</span>
                  <span>SEED / {result.seed}</span>
                  <span>DIGEST / {result.replay_digest.slice(0, 12)}…</span>
                </div>
              </>
            )}
            <div className="card-footer">
              <span>REPOSITORY</span>
              <strong>LOCAL SQLITE</strong>
            </div>
          </div>
        </section>
      </main>

      <footer className="app-footer">
        <span>PROBE//LAB / DESKTOP VERTICAL SLICE</span>
        <span>NO AUTH · NO CLOUD · NO NETWORK FEATURES</span>
      </footer>
    </div>
  );
};

export default App;
