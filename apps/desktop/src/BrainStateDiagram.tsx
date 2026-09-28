/**
 * The Brain's own state, drawn from the read model.
 *
 * # What this is not
 *
 * This is not a neural network, a mind, or a thinking process, and it is deliberately not drawn to
 * look like one. `LabBrain` in `crates/lab/src/brain.rs` is a deterministic orchestrator: a state
 * field, a plan, and a `BTreeMap<ObservedCategory, u32>` of counts. There is no model inside it, no
 * inference, and nothing resembling reasoning. A diagram of glowing nodes and firing synapses would
 * assert a capability the product does not have, and this project's first rule is that a panel must
 * never show a thing the backend did not produce.
 *
 * So this draws the two things the Brain genuinely is: which of its own lifecycle states it is in,
 * and what it is currently counting. Every value below comes from the `BrainReadModel` the backend
 * projected. Nothing is estimated, and nothing appears that the backend did not send.
 *
 * # The state list is shorter than the enum, and that is the point
 *
 * `BrainState` has six variants. Only five are ever reachable: `LabBrain` assigns `Planning` at
 * construction and then `Preparing`, `Observing`, `Analyzing`, and `Completed` — nothing anywhere in
 * the workspace assigns `Running` to the Brain's state. The sixth variant exists in the `label()` match
 * and in two read-model tests, which is why it looks live.
 *
 * Drawing all six would put a node on screen that the product cannot enter, and a diagram that shows
 * a state the machine never reaches is a small lie about how the system works. `REACHABLE_STATES`
 * therefore lists five, and `the_brain_never_enters_running` in `BrainPanel.test.tsx` fails if that
 * ever stops being true — at which point this file should be updated rather than the test weakened.
 *
 * The order is the machine's real order. `Preparing` prepares the environment, `Observing` is entered
 * by `LabBrain::observe` on every event from the runner, and there is no separate "episodes are
 * executing" state because the Brain does not have one.
 */
import type { BrainReadModel, SignalView } from "./api";

/**
 * The states `LabBrain` can actually be in, in the order it moves through them.
 *
 * See the module comment for why `Running` is absent. If that changes, change this.
 */
export const REACHABLE_STATES = ["planning", "preparing", "observing", "analyzing", "completed"] as const;

export type ReachableState = (typeof REACHABLE_STATES)[number];

function isReachable(state: string): state is ReachableState {
  return (REACHABLE_STATES as readonly string[]).includes(state);
}

/** Uppercase label for a lifecycle state, matching `BrainState::label`. */
function stateLabel(state: string): string {
  return state.replace(/_/g, " ").toUpperCase();
}

/**
 * Where a state sits in the sequence, for deciding what to mark as already passed.
 *
 * `null` for a state the backend sent that is not in the list above — which should not happen, and is
 * rendered as "unknown" rather than silently treated as the last step. A state this component does
 * not recognise is a fact worth showing, not a gap worth hiding.
 */
function stateIndex(state: string): number | null {
  return isReachable(state) ? REACHABLE_STATES.indexOf(state) : null;
}

/** The tone for a counted signal. Mirrors `signalTone` in `BrainPanel`, kept local to avoid a cycle. */
function signalToneClass(signal: SignalView): string {
  switch (signal.interpretation) {
    case "violation":
      return "tone-violation";
    case "correct_refusal":
      return "tone-refusal";
    case "protocol_shortfall":
      return "tone-warning";
    case "operational":
      return "tone-error";
    default:
      return "tone-info";
  }
}

/**
 * What an interpretation means, so the colour is never the only thing distinguishing a correct
 * refusal from a real violation.
 */
function interpretationText(signal: SignalView): string {
  switch (signal.interpretation) {
    case "correct_refusal":
      return "enforcement working";
    case "violation":
      return "boundary violation";
    case "protocol_shortfall":
      return "response-format shortfall";
    case "operational":
      return "harness or provider";
    default:
      return "routine";
  }
}

export function BrainStateDiagram({ model }: { model: BrainReadModel }) {
  const current = model.brainState;
  const currentIndex = stateIndex(current);
  const known = currentIndex !== null;

  /*
   * Bars are scaled to the largest count in this run, not to some fixed maximum. A fixed ceiling
   * would make a run with 40 successes look identical to one with 4, and the panel's job is to show
   * what this run actually did. A run where every count is zero renders no bars at all, because a
   * full-width bar for "nothing observed" is the opposite of the truth.
   */
  const counts = model.signals.map((signal) => signal.count);
  const largest = counts.length > 0 ? Math.max(...counts) : 0;

  return (
    <section className="brain-section brain-diagram" aria-labelledby="brain-diagram-heading">
      <h3 id="brain-diagram-heading">What the Brain is</h3>
      <p className="section-note">
        A state machine and a set of counters — not a model. Every value here is one the backend
        reported; nothing is inferred.
      </p>

      <ol className="brain-lifecycle" aria-label="Brain lifecycle">
        {REACHABLE_STATES.map((state, index) => {
          /*
           * Three distinct visual states, and the difference matters: `current` is where the Brain is
           * now, `passed` is behind it, and `ahead` has not been entered. A single "active" highlight
           * with everything else dim would not distinguish a finished run from an idle one.
           */
          const isCurrent = known && index === currentIndex;
          const isPassed = known && index < currentIndex;
          const isAhead = known && index > currentIndex;
          return (
            <li
              key={state}
              className={[
                "brain-lifecycle-step",
                isCurrent ? "is-current" : "",
                isPassed ? "is-passed" : "",
                isAhead ? "is-ahead" : "",
              ]
                .filter(Boolean)
                .join(" ")}
              aria-current={isCurrent ? "step" : undefined}
            >
              <span className="lifecycle-dot" aria-hidden="true" />
              <span className="lifecycle-label">{stateLabel(state)}</span>
            </li>
          );
        })}
      </ol>

      {/*
        A state outside the five above means the backend sent something this component does not model.
        Saying so is the honest response; drawing it as the last step would misreport where the Brain
        actually is.
      */}
      {!known ? (
        <p className="brain-diagram-unknown">
          The backend reported a state this panel does not model:{" "}
          <strong>{stateLabel(current)}</strong>
        </p>
      ) : null}

      {/*
        The empty case is the wording the old "Monitoring" section used, kept deliberately: a clean run
        shows no rows rather than a page of zeros. A grid of empty bars would imply the Brain counted
        nothing *separately for each category*, which is a different and more specific claim than
        "nothing was counted".
      */}
      {model.signals.length > 0 ? (
        <div className="brain-counters">
          <h4>Counted since the run started</h4>
          <ul>
            {model.signals.map((signal) => (
              <li key={signal.id} className={signalToneClass(signal)}>
                <div className="counter-head">
                  <span className="counter-label">{signal.label}</span>
                  <span className="counter-value">{signal.count}</span>
                </div>
                {/*
                  `aria-hidden` on the bar: the count is already a text node immediately before it, so
                  a screen reader reading the bar too would announce the same number twice with no
                  added meaning. The bar is a visual comparison aid, and the text is the fact.
                */}
                <span
                  className="counter-bar"
                  aria-hidden="true"
                  style={{
                    width: largest > 0 ? `${Math.round((signal.count / largest) * 100)}%` : "0%",
                  }}
                />
                <span className="counter-meaning">{interpretationText(signal)}</span>
              </li>
            ))}
          </ul>
        </div>
      ) : (
        <p className="panel-note">
          Nothing to report. Signals appear only when there is something to count — a clean run shows
          no rows rather than a page of zeros.
        </p>
      )}
    </section>
  );
}
