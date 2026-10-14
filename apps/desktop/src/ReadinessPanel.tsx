/**
 * Readiness panel: what this machine can actually run.
 *
 * Discovery verifies rather than assumes, and this panel renders the
 * verification: available executors say what was found, unavailable ones say
 * why not and what would change it. An unavailable executor is a fact, not a
 * disabled promise — there is no control here that claims to work and does
 * not.
 */

import { useMemo, useState } from "react";

import type { ExecutorDescriptor, GuestIsolation, PreflightReport } from "./api";
import { inspectGuestIsolation, preflightExecutor } from "./api";
import { IconChevronDown, IconLock, IconScan } from "./Icons";

const MODE_ORDER = ["offline_local", "coding_agent", "guided_external", "remote_provider"] as const;

/**
 * Whether an executor is blocked by policy rather than by a missing dependency.
 *
 * These are different problems and the page has to keep them apart. "No API key is set" is something
 * the user fixes by setting a key. "Automatic execution of this agent is disabled by security policy"
 * is not fixable by the user at all, and presenting the two with the same grey card and the same
 * remedy box would send someone looking for a setting that does not exist.
 */
export function isRestricted(executor: ExecutorDescriptor): boolean {
  if (executor.availability.state === "available") {
    return false;
  }
  return /policy|disabled|refus|not permitted/i.test(executor.availability.reason);
}

/** A filled pip. The one shape that carries state without relying on hue. */
function StatusPip({ tone }: { tone: "ok" | "restricted" | "off" }) {
  return <span className={`status-pip status-pip--${tone}`} aria-hidden="true" />;
}

export function modeLabel(mode: string): string {
  switch (mode) {
    case "offline_local":
      return "OFFLINE LOCAL";
    case "coding_agent":
      return "CODING AGENT";
    case "guided_external":
      return "GUIDED EXTERNAL";
    case "remote_provider":
      return "REMOTE PROVIDER";
    default:
      return mode.toUpperCase();
  }
}

/** Groups executors by mode in the stable display order. */
export function groupByMode(executors: ExecutorDescriptor[]): { mode: string; items: ExecutorDescriptor[] }[] {
  return MODE_ORDER.map((mode) => ({
    mode,
    items: executors.filter((executor) => executor.mode === mode),
  })).filter((group) => group.items.length > 0);
}

function AvailabilityNote({ executor }: { executor: ExecutorDescriptor }) {
  const availability = executor.availability;
  if (availability.state === "available") {
    return (
      <div className="executor-note">
        <dt>Evidence</dt>
        <dd>{availability.evidence}</dd>
      </div>
    );
  }
  return (
    <>
      <div className="executor-note">
        <dt>Reason</dt>
        <dd>{availability.reason}</dd>
      </div>
      {availability.remedy ? (
        <div className="executor-note is-remedy">
          <dt>
            {isRestricted(executor) ? (
              <>
                <IconLock size={12} /> Path
              </>
            ) : (
              "Remedy"
            )}
          </dt>
          <dd>{availability.remedy}</dd>
        </div>
      ) : null}
    </>
  );
}

export function ReadinessPanel({
  executors,
  error,
  loading,
  onRefresh,
}: {
  executors: ExecutorDescriptor[];
  error: string | null;
  loading: boolean;
  onRefresh: () => void;
}) {
const [reports, setReports] = useState<Record<string, PreflightReport>>({});
  const [checking, setChecking] = useState<string | null>(null);
  const [checkError, setCheckError] = useState<string | null>(null);
  const [guest, setGuest] = useState<GuestIsolation | null>(null);
  const [guestChecking, setGuestChecking] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);

  const groups = groupByMode(executors);
  const ready = executors.filter((executor) => executor.availability.state === "available").length;
  const restricted = executors.filter(isRestricted).length;

  /*
    * The remedy that would unblock the most unavailable executors.
    *
    * Chosen by how many cards carry the same remedy, so a single missing credential is surfaced once
    * with its real reach rather than nine times in nine cards. An environment-variable remedy names
    * the variable so it can be copied rather than transcribed.
    *
    * This is a hook, so it runs unconditionally. It used to sit *after* an early return for the
    * discovery-error case, which meant the hook count changed the moment `error` cleared — and React
    * throws "Rendered more hooks than during the previous render" rather than reconciling. That turned
    * a recoverable discovery failure into a permanently broken panel: retrying successfully, the one
    * thing the retry button exists to do, crashed the view. Every hook in this component is now above
    * every early return, and the panel's own test pins that ordering.
    */
  const nextStep = useMemo(() => {
    const byRemedy = new Map<
      string,
      { remedy: string; envVar: string | null; count: number }
    >();
    for (const executor of executors) {
      // `remedy` only exists on the unavailable variant; narrowing rather than reading it off
      // the union keeps this from silently treating `null` on an available executor as a blocker.
      const availability = executor.availability;
      if (availability.state !== "unavailable") {
        continue;
      }
      const remedy = availability.remedy;
      if (!remedy) {
        continue;
      }
      const envVar = /environment variable|API_KEY|api key/i.test(remedy)
        ? (remedy.match(/[A-Z][A-Z0-9_]{5,}/)?.[0] ?? null)
        : null;
      const existing = byRemedy.get(remedy);
      if (existing) {
        existing.count += 1;
      } else {
        byRemedy.set(remedy, { remedy, envVar, count: 1 });
      }
    }
    const best = [...byRemedy.values()].sort((a, b) => b.count - a.count)[0];
    if (!best) {
      return null;
    }
    return {
      text: `${best.count} of ${executors.length - ready} unavailable executors share one blocker: ${best.remedy}`,
      envVar: best.envVar,
      route: best.envVar ? ("settings" as const) : ("lab" as const),
      routeLabel: best.envVar ? "SETTINGS" : "THE LAB",
    };
  }, [executors, ready]);

  const copyEnvVar = async (name: string) => {
    try {
      await navigator.clipboard.writeText(name);
      setCopied(name);
      setTimeout(() => setCopied(null), 1800);
    } catch {
      setCopied(null);
    }
  };

  if (error) {
    return (
      <section className="panel readiness-panel" aria-labelledby="readiness-heading">
        <div className="panel-heading">
          <h2 id="readiness-heading">READINESS</h2>
          <span className="tone-error">DISCOVERY FAILED</span>
        </div>
        <p className="brain-error" role="alert">
          {error}
        </p>
        <button type="button" className="button-quiet" onClick={onRefresh} disabled={loading}>
          RETRY DISCOVERY
        </button>
      </section>
    );
  }

  const runPreflight = async (id: string) => {
    setChecking(id);
    setCheckError(null);
    try {
      const report = await preflightExecutor(id);
      setReports((previous) => ({ ...previous, [id]: report }));
    } catch (err) {
      setCheckError(err instanceof Error ? err.message : String(err));
    } finally {
      setChecking(null);
    }
  };

  return (
    <section className="panel readiness-panel" aria-labelledby="readiness-heading">
      {/*
        The Scan action lives in the panel heading, where a user looks for it.

        It was a lone button below a paragraph, which is the least findable position a control can
        occupy -- it read as part of the prose rather than as the thing to press. It is also the one
        action on this page: discovery is what the page is for, so it gets the primary treatment and
        the panel's only filled button.
      */}
      <div className="panel-heading">
        <div>
          <p className="eyebrow">Preflight</p>
          <h2 id="readiness-heading">Readiness</h2>
        </div>
        <div className="toolbar-actions">
          <span className={`readiness-tally${ready > 0 ? " is-ready" : ""}`}>
            {ready} of {executors.length} ready
          </span>
          <button type="button" className="btn btn-primary btn-sm" onClick={onRefresh} disabled={loading}>
            <IconScan size={15} />
            {loading ? "Scanning…" : "Scan"}
          </button>
        </div>
      </div>

      {/*
        The overview strip: one sentence, plus the three states that actually exist here. A reader
        should learn what is runnable and what is not without opening a single card.
      */}
      <div className="readiness-overview">
        <div className={`readiness-stat${ready > 0 ? " is-ready" : ""}`}>
          <span className="readiness-stat-value">{ready}</span>
          <span className="readiness-stat-label">Can run now</span>
        </div>
        <div className={`readiness-stat${executors.length - ready > 0 ? " is-warn" : ""}`}>
          <span className="readiness-stat-value">{executors.length - ready}</span>
          <span className="readiness-stat-label">Unavailable</span>
        </div>
        <div className={`readiness-stat${restricted > 0 ? " is-off" : ""}`}>
          <span className="readiness-stat-value">{restricted}</span>
          <span className="readiness-stat-label">Restricted by policy</span>
        </div>
        <p className="readiness-overview-note">
          Verified on this machine, just now — not what the documentation says it could run somewhere.
          Offline, agent and guided modes need no API key.
        </p>
      </div>

      {/*
        The single next action.

        Every card below carries its own remedy, so the facts are all present — but that puts the
        answer in nine places and none of them is *the* answer. A reader who arrives asking "what do I
        do about the eight that are unavailable" has to collect eight remedies and decide which one
        unblocks the most. This names the one with the broadest effect and offers the control that
        performs it, so the page answers a question rather than only reporting state.

        It is deliberately the most common remedy, not the first one. Taking the first would be simpler
        to write and would help whoever happens to be alphabetically first.
      */}
      {nextStep ? (
        <div className="readiness-next" role="note">
          <span className="readiness-next-label">Next step</span>
          <p>{nextStep.text}</p>
          <div className="readiness-next-actions">
            {nextStep.envVar ? (
              <button
                type="button"
                className="btn btn-secondary btn-sm"
                onClick={() => void copyEnvVar(nextStep.envVar as string)}
              >
                {copied === nextStep.envVar ? "COPIED" : `COPY ${nextStep.envVar}`}
              </button>
            ) : null}
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              onClick={() => {
                window.location.hash = "#/settings";
              }}
            >
              OPEN SETTINGS
            </button>
            {nextStep.route ? (
              <button
                type="button"
                className="btn btn-secondary btn-sm"
                onClick={() => {
                  window.location.hash = `#/${nextStep.route}`;
                }}
              >
                GO TO {nextStep.routeLabel ?? nextStep.route.toUpperCase()}
              </button>
            ) : null}
          </div>
        </div>
      ) : null}

      {groups.map((group) => (
        <section key={group.mode} className="readiness-group">
          <h3 className="readiness-group-title">
            {modeLabel(group.mode)}
            <span className="agent-group-count">{group.items.length}</span>
          </h3>
          <ul className="executor-grid">
            {group.items.map((executor) => {
              const report = reports[executor.id];
              const available = executor.availability.state === "available";
              const restricted = isRestricted(executor);
              return (
                <li
                  key={executor.id}
                  className={`executor-card${available ? " is-available" : " is-unavailable"}${
                    restricted ? " is-restricted" : ""
                  }`}
                >
                  {/*
                    The verdict is first and is stated three ways: a pip on the card edge, a word in a
                    tinted chip, and the position of the card's own layout. Colour alone would fail
                    anyone who cannot separate the state colours, and this page exists to answer "can I
                    run this" before anything else.
                  */}
                  <div className="executor-head">
                    <div className="executor-identity">
                      <span className="executor-label">
                        <StatusPip tone={available ? "ok" : restricted ? "restricted" : "off"} />
                        {executor.label}
                      </span>
                      <span className="executor-id mono">{executor.id}</span>
                    </div>
                    <span className={`executor-state ${available ? "tone-ok" : restricted ? "tone-deferred" : "tone-neutral"}`}>
                      {available ? "Ready" : restricted ? "Restricted" : "Unavailable"}
                    </span>
                  </div>

                  <p className="executor-detail">{executor.detail}</p>

                  {/*
                    Technical metadata sits behind a disclosure.

                    Capability lists and executor identifiers are what a reader needs while debugging a
                    setup, and are noise while deciding what to run. Keeping them collapsed is what lets
                    the card above stay a card instead of becoming a specification table.
                  */}
                  <details className="disclosure">
                    <summary>
                      Technical detail
                      <IconChevronDown size={14} />
                    </summary>
                    <dl className="executor-facts">
                      <div>
                        <dt>Capabilities</dt>
                        <dd className="mono">
                          {executor.capabilities.join(", ") || "none stated"}
                        </dd>
                      </div>
                      <div>
                        <dt>Mode</dt>
                        <dd>{modeLabel(executor.mode)}</dd>
                      </div>
                      <AvailabilityNote executor={executor} />
                    </dl>
                  </details>

                  <div className="executor-actions">
                    <button
                      type="button"
                      className="btn btn-secondary btn-sm"
                      onClick={() => void runPreflight(executor.id)}
                      disabled={checking === executor.id}
                    >
                      {checking === executor.id ? "Checking…" : "Preflight"}
                    </button>
                  </div>

                  {report ? (
                    <ul className="preflight-checks" aria-label={`Preflight for ${executor.label}`}>
                      {report.checks.map((check) => (
                        <li key={check.name} className={check.passed ? "tone-ok" : "tone-neutral"}>
                          <span className="mono">{check.name}</span>:{" "}
                          {check.passed ? "pass" : "fail"} — {check.detail}
                        </li>
                      ))}
                    </ul>
                  ) : null}
                </li>
              );
            })}
          </ul>
        </section>
      ))}

      {checkError ? (
        <p className="brain-error" role="alert">
          {checkError}
        </p>
      ) : null}

      {/*
        Guest isolation, inspected read-only. The verdict is always guided-only:
        a present sandbox feature is not a tested boundary, and an absent one
        is nowhere to run. Either way the supported path for installed agents
        is guided packages, and this section says which reason applies here.
      */}
      <div className="readiness-group">
        <h3>GUEST ISOLATION</h3>
        {guest === null ? (
          <>
            <p className="panel-note">
              Whether this machine could host a disposable guest for agent execution — checked
              without changing anything. Automatic agent execution stays disabled regardless;
              this only determines which reason applies.
            </p>
            <button
              type="button"
              className="button-quiet"
              onClick={() => {
                setGuestChecking(true);
                inspectGuestIsolation()
                  .then(setGuest)
                  .catch((err: unknown) =>
                    setCheckError(err instanceof Error ? err.message : String(err)),
                  )
                  .finally(() => setGuestChecking(false));
              }}
              disabled={guestChecking}
            >
              {guestChecking ? "INSPECTING…" : "INSPECT GUEST SUPPORT"}
            </button>
          </>
        ) : (
          <>
            <p className="panel-note">{guest.detail}</p>
            <dl className="brain-facts">
              <div>
                <dt>Operating system</dt>
                <dd>{guest.os}</dd>
              </div>
              <div>
                <dt>Sandbox feature</dt>
                <dd>{guest.sandboxFeature}</dd>
              </div>
              <div>
                <dt>Hyper-V feature</dt>
                <dd>{guest.hypervFeature}</dd>
              </div>
              <div>
                <dt>Standing rule</dt>
                <dd>GUIDED ONLY</dd>
              </div>
            </dl>
          </>
        )}
      </div>
    </section>
  );
}
