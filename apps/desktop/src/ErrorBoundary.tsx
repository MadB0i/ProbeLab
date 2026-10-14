/**
 * Error boundaries with recovery.
 *
 * # Why this exists
 *
 * Before it, a single throw in any panel blanked the whole window. There was no boundary anywhere in
 * the application, so React unmounted the entire tree and the operator was left looking at the
 * operating system's window background with a taskbar entry that did nothing. For a desktop
 * application that holds an hour of experiment results, that is the worst possible failure mode: the
 * data is still in SQLite, but there is no way to reach it through the interface.
 *
 * # What a boundary does and does not do here
 *
 * It contains the failure to the smallest useful unit and offers a way back. It does **not** swallow
 * the error, does not pretend the panel recovered, and does not silently retry — a silent retry of a
 * component that throws deterministically just delays the same failure and hides it. The operator sees
 * what failed, in enough detail to report it, with two explicit actions.
 *
 * # Scope
 *
 * Boundaries are per *panel*, not one around everything. A panel-level boundary means a failure in the
 * Arena leaves the command center, the navigation rail and the status bar alive, which is what keeps
 * the rest of the application reachable. One global boundary would convert any failure into a total
 * outage, which is the failure mode this file exists to remove.
 *
 * # Why the detail is shown but sanitised
 *
 * An operator who cannot see the message cannot report the bug. The component stack and the message
 * are shown inside a collapsed `<details>`, so they are available without dominating the panel. They
 * are React error text, not model or provider text, so they carry no credential — the credential
 * never crosses into the renderer in the first place, which the IPC boundary tests assert
 * separately.
 */

import { Component, type ErrorInfo, type ReactNode } from "react";

/** One caught failure, in the shape the fallback renders. */
export type CaughtError = {
  /** The error's own message, or a stand-in when it has none. */
  message: string;
  /** The React component stack, which is where the fault actually is. */
  componentStack: string;
  /** How many times this boundary caught something before a reset. */
  caughtCount: number;
};

type Props = {
  /** The subtree this boundary protects. */
  children: ReactNode;
  /**
   * What to call the failing surface, shown in the fallback so the operator knows where to look.
   *
   * Required rather than optional: a boundary that says "something went wrong" with no subject is
   * barely better than the blank window it replaced.
   */
  label: string;
  /**
   * An optional recovery action specific to this surface.
   *
   * Rendered beside the built-in retry, which is what lets a run be restarted rather than merely
   * re-rendered. Deliberately not a default: the right recovery differs per panel, and a generic
   * "try again" would be a guess.
   */
  onRecover?: () => void;
  /** What to call the recovery action. */
  recoverLabel?: string;
};

type State = {
  error: CaughtError | null;
};

export class PanelErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: unknown): State {
    return {
      error: {
        message: describe(error),
        // Filled in by `componentDidCatch`, which React calls with the component stack.
        componentStack: "",
        caughtCount: 1,
      },
    };
  }

  componentDidCatch(error: unknown, info: ErrorInfo): void {
    // Logged rather than swallowed: the fallback shows the operator what happened, and this is what
    // makes it available to whoever is looking at the terminal. Both halves are deliberate — a
    // boundary that neither shows nor logs a failure is indistinguishable from one that has none.
    console.error("a panel failed to render", error, info.componentStack);
    this.setState((current) => ({
      error: {
        message: describe(error),
        componentStack: info.componentStack ?? "",
        caughtCount: (current.error?.caughtCount ?? 0) + 1,
      },
    }));
  }

  private readonly reset = () => {
    this.setState({ error: null });
  };

  render(): ReactNode {
    const { error } = this.state;
    if (!error) {
      return this.props.children;
    }
    const { label, onRecover, recoverLabel } = this.props;
    return (
      <section className="panel panel-boundary" aria-labelledby="panel-boundary-heading">
        <div className="panel-heading">
          <h2 id="panel-boundary-heading">{label} STOPPED</h2>
          <span className="tone-error">RENDER FAILED</span>
        </div>
        {/*
          `alert` because this is a terminal state for that panel: the operator must not read past it
          assuming the content below is current. It is announced, not styled as an interruption,
          because nothing here interrupts work in progress elsewhere in the window.
        */}
        <p className="panel-boundary__message" role="alert">
          {label} could not be displayed. Everything else in the window is still usable, and any run
          already in progress is unaffected.
        </p>
        {onRecover ? (
          <div className="panel-boundary__actions">
            <button type="button" className="btn btn-primary" onClick={onRecover}>
              {recoverLabel ?? "RECOVER"}
            </button>
            <button type="button" className="btn btn-secondary" onClick={this.reset}>
              RETRY RENDER
            </button>
          </div>
        ) : (
          <div className="panel-boundary__actions">
            <button type="button" className="btn btn-primary" onClick={this.reset}>
              RETRY RENDER
            </button>
          </div>
        )}
        <details className="panel-boundary__detail">
          <summary>Technical detail</summary>
          <p className="mono panel-boundary__message-text">{error.message}</p>
          {error.componentStack ? (
            <pre className="mono panel-boundary__stack">{error.componentStack.trim()}</pre>
          ) : null}
          {error.caughtCount > 1 ? (
            <p className="muted">This surface has failed {error.caughtCount} times.</p>
          ) : null}
        </details>
      </section>
    );
  }
}

/**
 * A readable message from an unknown thrown value.
 *
 * `String(error)` is the trap this replaces: a thrown object renders as `[object Object]`, which is
 * exactly the moment the operator most needs to read what went wrong.
 */
function describe(error: unknown): string {
  if (error instanceof Error) {
    return error.message === "" ? error.name : `${error.name}: ${error.message}`;
  }
  if (typeof error === "string") {
    return error;
  }
  try {
    return JSON.stringify(error) ?? String(error);
  } catch {
    return String(error);
  }
}

/**
 * Wraps a subtree, naming it.
 *
 * A thin convenience so each panel's boundary line is one expression rather than five, and so the
 * label cannot be forgotten — a boundary that does not say what it protects is the kind of thing
 * that gets deleted during a refactor because nobody remembers what it was for.
 */
export function PanelBoundary({
  label,
  onRecover,
  recoverLabel,
  children,
}: Props): ReactNode {
  return (
    <PanelErrorBoundary
      label={label}
      onRecover={onRecover}
      recoverLabel={recoverLabel}
    >
      {children}
    </PanelErrorBoundary>
  );
}