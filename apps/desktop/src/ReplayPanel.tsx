/**
 * The Replay panel.
 *
 * # What this panel does, precisely
 *
 * It steps through a *recording*. It does not re-run the match or the experiment, and it cannot: the
 * replay command reconstructs from stored frames and holds no engine, no provider, and no broker. A
 * "replay" that re-queried a model would differ every time while claiming to be a reproduction, which
 * would defeat the entire purpose of having one.
 *
 * # The capability banner is the important part
 *
 * A recording that captured every intermediate board is *re-executable*: you can step it. A recording
 * that captured only an event stream is *inspection only*: you can read what was reported and when,
 * but the intermediate environment states were never recorded and are not reconstructed. Those are
 * different claims, and the panel states which one it has rather than implying both.
 *
 * Limitations are shown in full. A replay that quietly filled a gap would be fabricating history, which
 * is the one thing this subsystem exists to prevent.
 */

import { useState } from "react";

import type { Replay, ReplayFrame } from "./api";

/** The label for what a frame represents. */
export function frameSourceLabel(source: ReplayFrame["source"]): string {
  switch (source) {
    case "start":
      return "START";
    case "arena_ply":
      return "PLY";
    case "experiment_event":
      return "EVENT";
    default:
      return "TERMINAL";
  }
}

/** Wording for the capability, which is the field a reader should check first. */
export function capabilityLabel(capability: Replay["capability"]): string {
  return capability === "full" ? "RE-EXECUTABLE" : "INSPECTION ONLY";
}

/** An explanation of what the capability means in practice. */
export function capabilityExplanation(replay: Replay): string {
  return replay.capability === "full"
    ? "Every intermediate state was recorded, so this can be stepped frame by frame."
    : "Only an event stream was recorded. The intermediate environment states were never captured, so \
       this can be read but not re-executed.";
}

export function ReplayPanel({
  replay,
  error,
  onLoad,
  loading,
  canLoad,
  matchId,
}: {
  replay: Replay | null;
  error: string | null;
  onLoad: () => void;
  loading: boolean;
  canLoad: boolean;
  matchId: string;
}) {
  // Stepping state lives here because it is a *view* concern -- the recording is immutable and the
  // backend is not consulted when the user moves between frames. That is what makes navigation instant
  // and provably free of side effects.
  const [position, setPosition] = useState(0);

  if (error) {
    return (
      <section className="panel replay-panel" aria-labelledby="replay-heading">
        <div className="panel-heading">
          <h2 id="replay-heading">REPLAY</h2>
          <span className="tone-error">COMMAND FAILED</span>
        </div>
        <p className="brain-error" role="alert">
          {error}
        </p>
      </section>
    );
  }

  if (replay === null) {
    return (
      <section className="panel replay-panel" aria-labelledby="replay-heading">
        <div className="panel-heading">
          <h2 id="replay-heading">REPLAY</h2>
          <span className="muted">NO RECORDING LOADED</span>
        </div>
        {/*
          * The guarantee, stated before any data. A user deciding whether to trust a replay needs to
          * know it cannot have called a model, not infer that from the absence of a button.
          */}
        <p className="replay-authority" role="note">
          Replay reconstructs recorded data. It does not re-run a match, re-query a model, or contact
          any provider — so it reproduces the same frames every time, or it fails.
        </p>
        <div className="replay-empty">
          <h3>NO RECORDING LOADED</h3>
          <p>
            Play a match in the Arena, then load it here to step through every recorded position. Replay
            reads the stored frames and nothing else.
          </p>
        </div>
        <div className="replay-controls">
          <button
            type="button"
            className="brain-run-button"
            onClick={onLoad}
            disabled={!canLoad || loading}
          >
            {loading ? "LOADING…" : "LOAD RECORDING"}
          </button>
          {!canLoad ? (
            <p className="replay-control-note">
              No match identifier available yet — play a match first.
            </p>
          ) : null}
        </div>
      </section>
    );
  }

  const total = replay.frames.length;

  /*
   * A recording with no frames is a real state -- a match that recorded nothing -- and the previous
   * clamp produced "Frame 0 of 0" with PREV enabled and doing nothing, because `current` went to -1 and
   * `current === 0` was false. A nonsense readout and a live-looking control that cannot move.
   *
   * The comment claimed the clamp "keeps a stale position from rendering nothing", which is precisely
   * what it did: `Math.min(position, -1)` is -1, and `frames[-1]` is `undefined`.
   */
  if (total === 0) {
    return (
      <section className="panel replay-panel" aria-labelledby="replay-heading">
        <div className="panel-heading">
          <h2 id="replay-heading">REPLAY</h2>
          <span className="replay-capability replay-capability--partial">
            {capabilityLabel(replay.capability)}
          </span>
        </div>

        <dl className="replay-facts">
          <div>
            <dt>Recording</dt>
            <dd className="mono">{replay.recordingId}</dd>
          </div>
          <div>
            <dt>Kind</dt>
            <dd>{replay.kind === "arena_match" ? "Arena match" : "Experiment"}</dd>
          </div>
          <div>
            <dt>Frames</dt>
            <dd className="mono">0</dd>
          </div>
          <div>
            <dt>Digest</dt>
            <dd className="mono">{replay.digest}</dd>
          </div>
        </dl>

        {/*
          Stated as a fact about the recording rather than as an error. Nothing failed: the backend
          reconstructed it, and the reconstruction has no frames to step through. Every limitation the
          backend reported is still shown, because the reason it is empty is usually in there.
        */}
        <div className="replay-empty">
          <h3>NO FRAMES TO STEP THROUGH</h3>
          <p>
            This recording contains no frames, so there is nothing to step. The digest above is the
            recording&apos;s own; what could not be reconstructed is listed below.
          </p>
        </div>

        {replay.limitations.length > 0 ? (
          <div className="replay-limitations">
            <h3>What could not be reconstructed</h3>
            <ul>
              {replay.limitations.map((limitation) => (
                <li key={limitation}>{limitation}</li>
              ))}
            </ul>
          </div>
        ) : null}
      </section>
    );
  }

  // Clamped for display only. Stepping past the end is prevented by disabling the control, so this
  // cannot mask a bug in the frame list; it only guards a position left over from a longer recording.
  const current = Math.min(position, total - 1);
  const frame = replay.frames[current];

  return (
    <section className="panel replay-panel" aria-labelledby="replay-heading">
      <div className="panel-heading">
        <h2 id="replay-heading">REPLAY</h2>
        <span className={replay.capability === "full" ? "replay-capability replay-capability--full" : "replay-capability replay-capability--partial"}>
          {capabilityLabel(replay.capability)}
        </span>
      </div>

      <dl className="replay-facts">
        <div>
          <dt>Recording</dt>
          <dd className="mono">{replay.recordingId}</dd>
        </div>
        <div>
          <dt>Kind</dt>
          <dd>{replay.kind === "arena_match" ? "Arena match" : "Experiment"}</dd>
        </div>
        <div>
          <dt>Frames</dt>
          <dd className="mono">{total}</dd>
        </div>
        <div>
          <dt>Digest</dt>
          <dd className="mono">{replay.digest}</dd>
        </div>
      </dl>

      <p className="replay-capability-note">{capabilityExplanation(replay)}</p>

      <div className="replay-stepper">
        <button
          type="button"
          className="replay-step"
          onClick={() => setPosition(Math.max(0, current - 1))}
          disabled={current === 0}
          aria-label="Previous frame"
        >
          ◀ PREV
        </button>
        <span className="replay-position" aria-live="polite">
          Frame {current + 1} of {total}
        </span>
        <button
          type="button"
          className="replay-step"
          onClick={() => setPosition(Math.min(total - 1, current + 1))}
          disabled={current >= total - 1}
          aria-label="Next frame"
        >
          NEXT ▶
        </button>
      </div>

      {frame ? (
        <div className="replay-frame">
          <span className="replay-frame-source">{frameSourceLabel(frame.source)}</span>
          <span className="replay-frame-label">{frame.label}</span>
          {frame.digest ? (
            <span className="replay-frame-digest mono">digest {frame.digest}</span>
          ) : null}
        </div>
      ) : null}

      {/*
        * Limitations are rendered in full and unconditionally when present. A replay that quietly
        * omitted what it could not reconstruct would be indistinguishable from one that had
        * reconstructed everything.
        */}
      {replay.limitations.length > 0 ? (
        <div className="replay-limitations">
          <h3>What could not be reconstructed</h3>
          <ul>
            {replay.limitations.map((limitation) => (
              <li key={limitation}>{limitation}</li>
            ))}
          </ul>
        </div>
      ) : null}

      <div className="replay-timeline">
        <h3>All frames</h3>
        <ol className="replay-frame-list">
          {replay.frames.map((entry) => (
            <li
              key={entry.index}
              className={`replay-frame-row${entry.index === current ? " replay-frame-row--current" : ""}`}
            >
              <button
                type="button"
                onClick={() => setPosition(entry.index)}
                aria-current={entry.index === current ? "true" : undefined}
              >
                <span className="replay-row-index">{String(entry.index).padStart(2, "0")}</span>
                <span className="replay-row-source">{frameSourceLabel(entry.source)}</span>
                <span className="replay-row-label">{entry.label}</span>
              </button>
            </li>
          ))}
        </ol>
      </div>
    </section>
  );
}
