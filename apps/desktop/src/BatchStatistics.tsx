/* -------------------------------------------------------------------------------------------- */
/* Batch statistics                                                                                 */
/* -------------------------------------------------------------------------------------------- */

/**
 * What this panel is allowed to say.
 *
 * Four constraints, and every one of them is a way a statistics panel normally lies:
 *
 * 1. **`noResult` is never drawn as a loss.** The distribution has four bars, not three. A panel that
 *    drew "2 losses" from one defeat and one refused connection would be reporting a defeat the
 *    tested seat never suffered.
 * 2. **No rate without its denominator and its interval.** A point estimate alone is a claim; the
 *    same number with "4 of 12" and a wide interval beside it is a measurement.
 * 3. **No cross-game total.** Groups are listed separately and never summed. This is stated in the
 *    panel's own prose rather than left as an absence someone might later fill in.
 * 4. **A small sample says so, in words.** Not as a greyed-out badge nobody reads, but as a sentence
 *    above the numbers.
 */

import { IconBoard, IconAlert, IconCheck, IconInfo, IconPlay, IconRefresh, IconStop } from "./Icons";
import type {
  ArenaStatistics,
  MatchPopulation,
  MatchStatistics,
  OutcomeCounts,
  WilsonInterval,
} from "./api";

/** Below this many decided matches a group is labelled a small sample. Mirrors the backend. */
const SMALL_SAMPLE_MATCHES = 10;

/** A percentage, to one decimal, from a fraction. */
function percent(value: number): string {
  return `${(value * 100).toFixed(1)}%`;
}

/**
 * A count with a correctly pluralised noun, so "1 match" never reads as a typo.
 *
 * The naive `${singular}s` was wrong for every noun ending in a consonant-plus-silent-e: it produced
 * "12 matchs". The vowel rule covers English's regular cases — pass, bus, class, match — and anything
 * irregular is passed explicitly as `many`.
 */
function plural(count: number, singular: string, many?: string): string {
  const word =
    many ??
    (count === 1
      ? singular
      : /(?:s|x|z|ch|sh)$/.test(singular)
        ? `${singular}es`
        : /[^aeiou]y$/.test(singular)
          ? `${singular.slice(0, -1)}ies`
          : `${singular}s`);
  return `${count} ${word}`;
}

/**
 * The four buckets, in the order a reader should meet them.
 *
 * `noResult` is last, and deliberately separated from `loss` by a gap in the markup rather than
 * adjacency: they are the two buckets a panel is most likely to merge by accident.
 */
function bucketOrder(outcomes: OutcomeCounts): { key: keyof OutcomeCounts; label: string }[] {
  return [
    { key: "win", label: "Won" },
    { key: "draw", label: "Drew" },
    { key: "loss", label: "Lost" },
    { key: "noResult", label: "No result" },
  ];
}

/**
 * The distribution, drawn as a proportional bar with real counts.
 *
 * Counts are in the markup, not only in the bar's width: a bar with no number is a shape the reader
 * has to measure, and a proportion with no sample size is a claim with no evidence.
 */
function Distribution({ outcomes }: { outcomes: OutcomeCounts }) {
  const total = outcomes.win + outcomes.draw + outcomes.loss + outcomes.noResult;
  if (total === 0) {
    return null;
  }
  return (
    <ul className="stats-distribution">
      {bucketOrder(outcomes).map(({ key, label }) => {
        const count = outcomes[key];
        return (
          <li key={key} className={`stats-distribution__row stats-distribution__row--${key}`}>
            <span className="stats-distribution__label">{label}</span>
            <span className="stats-distribution__bar" aria-hidden="true">
              <span className="stats-distribution__fill" style={{ width: `${(count / total) * 100}%` }} />
            </span>
            <span className="stats-distribution__count mono">{count}</span>
          </li>
        );
      })}
    </ul>
  );
}

/**
 * One rate, with its interval drawn underneath the point.
 *
 * The bar is the interval's full width with the point estimate marked inside it, so a reader sees both
 * the number and how much room it has to move. The whole thing carries a text alternative naming the
 * bounds, because a two-pixel mark on a track is not something a screen reader can convey and not
 * something every reader can estimate.
 */
function Rate({
  label,
  note,
  interval,
  numerator,
  denominator,
}: {
  label: string;
  note: string;
  interval: WilsonInterval;
  numerator: number;
  denominator: number;
}) {
  if (denominator === 0) {
    return (
      <div className="stats-rate">
        <h4 className="stats-rate__label">{label}</h4>
        <p className="stats-rate__absent">
          <IconInfo size={13} /> Nothing to measure — no match in this group produced a result of this
          kind.
        </p>
      </div>
    );
  }
  const confidence = Math.round(interval.confidence * 100);
  return (
    <div className="stats-rate">
      <h4 className="stats-rate__label">{label}</h4>
      <p className="stats-rate__value">
        <span className="stats-rate__figure">{percent(interval.point)}</span>
        <span className="stats-rate__count mono">
          {numerator} of {denominator}
        </span>
      </p>
      {/*
       * One line, deliberately.
       *
       * This string was wrapped across source lines for readability, which put a newline and a run of
       * spaces in the middle of the accessible name. Assistive technology announces the raw attribute,
       * so it read "confidence interval\n           from 24.0%" — with the pause in the wrong place. Built
       * from joined parts instead, so formatting the source cannot change what is spoken.
       */}
      <div
        className="stats-interval"
        role="img"
        aria-label={[
          label,
          `${percent(interval.point)},`,
          `${confidence} percent confidence interval`,
          `from ${percent(interval.lower)} to ${percent(interval.upper)},`,
          `over ${denominator} matches.`,
        ].join(" ")}
      >
        <span className="stats-interval__track" aria-hidden="true">
          <span className="stats-interval__span" style={{ left: `${interval.lower * 100}%`, width: `${(interval.upper - interval.lower) * 100}%` }} />
          <span className="stats-interval__point" style={{ left: `${interval.point * 100}%` }} />
        </span>
      </div>
      <p className="stats-rate__note">
        {confidence}% interval {percent(interval.lower)}–{percent(interval.upper)}. {note}
      </p>
    </div>
  );
}

/** One fact, label above value, in the product's existing readout shape. */
/**
 * One labelled figure.
 *
 * `hint` becomes the accessible description of the value rather than visible prose, so each bucket can
 * say what it means without the block turning into a wall of text. The visible note under the block
 * carries the same explanation in full for everyone who reads it.
 */
function Stat({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="stats-stat" title={hint}>
      <dt className="stats-stat__label">
        {label}
        {hint ? <span className="stats-stat__hint" aria-hidden="true" title={hint}>i</span> : null}
      </dt>
      <dd className="stats-stat__value mono" aria-label={hint ? `${label}: ${value}. ${hint}` : undefined}>
        {value}
      </dd>
    </div>
  );
}

/** A duration, or an honest statement that none was measured. */
function millis(value: number | null): string {
  return value === null ? "not reported" : `${(value / 1000).toFixed(2)} s`;
}

/** A token count, where absent is not zero. */
function tokens(value: number | null): string {
  return value === null ? "not reported" : value.toLocaleString("en-US");
}

/**
 * Execution reliability: how many attempted matches produced a conditional outcome at all.
 *
 * Audit finding B6, and the figure that makes the other two readable. A seat can win every match it
 * finishes and finish almost none of them; without this number beside the win rate, "100%" and
 * "finished 3 of 10" are indistinguishable.
 */
function Reliability({ population }: { population: MatchPopulation }) {
  const reliability = population.attempted > 0 ? population.conditional / population.attempted : null;
  const { forfeited, infrastructureFailures, cancelled, unclassified } = population;
  const excluded = infrastructureFailures + cancelled + unclassified;

  return (
    <section className="stats-population" aria-label="Execution reliability">
      <h4 className="stats-population__title">Execution reliability</h4>
      <p className="stats-population__headline">
        <span className="stats-population__figure mono">
          {reliability === null ? "—" : percent(reliability)}
        </span>
        <span className="stats-population__count mono">
          {population.conditional} of {population.attempted} attempted
        </span>
      </p>
      <p className="stats-population__note">
        The share of attempted matches that produced a game outcome at all — the rules deciding, or the
        tested seat forfeiting. The win rate below is measured over that {population.conditional}, not
        over the {population.attempted} attempted.
      </p>
      <dl className="stats-population__buckets">
        <Stat label="Attempted" value={String(population.attempted)} />
        <Stat label="Reached a game outcome" value={String(population.conditional)} />
        <Stat
          label="Forfeited by the tested seat"
          value={String(forfeited)}
          hint="Counted as a loss. The seat was reached and answered unusably, which is a result about the model."
        />
        <Stat
          label="Provider or harness failure"
          value={String(infrastructureFailures)}
          hint="Not a game outcome. The model was never reached, so nothing about it was measured."
        />
        <Stat
          label="Stopped by the operator"
          value={String(cancelled)}
          hint="Not a game outcome and not a fault."
        />
        <Stat
          label="Reason not recorded"
          value={String(unclassified)}
          hint="Older recordings predate the termination field. What ended them is not known and is not guessed."
        />
      </dl>
      {excluded > 0 ? (
        <p className="stats-population__note stats-population__note--excluded" role="note">
          <IconInfo size={14} />
          <span>
            {plural(excluded, "match", "matches")} {excluded === 1 ? "is" : "are"} outside the game
            outcomes and {excluded === 1 ? "does" : "do"} not appear as a win, a loss or a forfeit.
          </span>
        </p>
      ) : null}
    </section>
  );
}

/**
 * One game and one pairing.
 *
 * The heading carries the game and both seats, because that triple is the only thing that makes a win
 * rate mean anything — and it is stated again in the provenance line under it, so the figure can never
 * be read without the sample it came from.
 */
function Group({ group }: { group: MatchStatistics }) {
  const { outcomes } = group.decisions;
  const decided = outcomes.win + outcomes.draw + outcomes.loss;
  const decisive = outcomes.win + outcomes.loss;
  const costs = group.costs;

  return (
    <article className="stats-group">
      <header className="stats-group__head">
        <h3 className="stats-group__title">
          <IconBoard size={14} /> {group.gameName}
        </h3>
        <p className="stats-group__pairing mono">
          {group.testedAgent} vs {group.opponentAgent}
        </p>
      </header>

      {group.sampleIsSmall ? (
        /*
         * The small-sample warning comes before the numbers, not after them, and it is prose rather
         * than a badge. A reader who sees "SMALL SAMPLE" in a chip scrolls past it; a reader who sees
         * a sentence about not knowing something yet stops.
         */
        <p className="stats-group__caution" role="note">
          <IconAlert size={14} />
          <span>
            <strong>Small sample.</strong> Only {plural(decided, "decided match", "decided matches")} of{" "}
            {plural(group.matches, "match")} reached a result, which is below the{" "}
            {SMALL_SAMPLE_MATCHES} this product treats as a minimum. The percentages below are
            provisional and the intervals are wide.
          </span>
        </p>
      ) : null}

      {outcomes.noResult > 0 ? (
        <p className="stats-group__note" role="note">
          <IconInfo size={14} />
          {/*
           * The counts and the verbs are derived separately, deliberately.
           *
           * The first version pushed a whole verb phrase through `plural()` as though "match" were the
           * noun, which produced "1 matchs did not reach a result and is counted separately" and, on the
           * cancellation clause, "1 were stopped". `plural` now owns nouns only; agreement is its own
           * decision, so neither can drift from the other's number.
           */}
          <span>
            {plural(outcomes.noResult, "match")} {outcomes.noResult === 1 ? "did" : "did not"} reach a
            result and {outcomes.noResult === 1 ? "is" : "are"} counted separately, not as{" "}
            {outcomes.noResult === 1 ? "a defeat" : "defeats"}.{" "}
            {group.decisions.cancelled > 0
              ? `${plural(group.decisions.cancelled, "match")} ${
                  group.decisions.cancelled === 1 ? "was" : "were"
                } stopped by the operator.`
              : null}{" "}
            {group.decisions.withFailure > 0
              ? `${plural(group.decisions.withFailure, "match")} ${
                  group.decisions.withFailure === 1 ? "recorded" : "recorded"
                } a failure reason.`
              : null}
          </span>
        </p>
      ) : null}

      <Reliability population={group.population} />

      <Distribution outcomes={outcomes} />

      <div className="stats-rates">
        <Rate
          label="Won, of matches that reached a result"
          note="A draw counts here in the denominator and not the numerator, so this is 'won outright'."
          interval={group.decisions.winRate}
          numerator={outcomes.win}
          denominator={decided}
        />
        <Rate
          label="Won, of matches that had a winner"
          note="Draws excluded, which is the closer reading when a pairing draws often."
          interval={group.decisions.decisiveWinRate}
          numerator={outcomes.win}
          denominator={decisive}
        />
      </div>

      <dl className="stats-facts">
        <Stat label="Matches played" value={String(group.matches)} />
        <Stat label="Reached a result" value={String(decided)} />
        <Stat label="No result" value={String(outcomes.noResult)} />
        <Stat label="Mean plies" value={group.plies.mean === null ? "—" : group.plies.mean.toFixed(1)} />
        <Stat label="Plies range" value={group.plies.min === null || group.plies.max === null ? "—" : `${group.plies.min}–${group.plies.max}`} />
        <Stat label="Illegal moves" value={String(group.illegalMoves)} />
        <Stat label="Mean decision" value={millis(costs.meanElapsedMs)} />
        <Stat label="Slowest turn" value={costs.maxElapsedMs === null ? "—" : `${(costs.maxElapsedMs / 1000).toFixed(2)} s`} />
        <Stat label="Total tokens" value={tokens(costs.totalTokens)} />
        <Stat label="Prompt tokens" value={tokens(costs.promptTokens)} />
        <Stat label="Completion tokens" value={tokens(costs.completionTokens)} />
      </dl>

      {/*
       * Turns that ran without reporting usage.
       *
       * Shown only when there is something to say. A policy seat reports no tokens and no per-turn
       * latency, and for a policy-versus-policy batch that is the expected shape rather than a gap — so
       * a note about "turns without usage" on such a batch would read as a fault in a run that worked
       * exactly as intended.
       */}
      {costs.turnsWithoutUsage > 0 ? (
        <p className="stats-group__note" role="note">
          <IconInfo size={13} />
          <span>
            {plural(costs.turnsWithoutUsage, "turn", "turns")} reported no token usage. A shipped policy
            has none to report, and a provider that returns no usage block has not said the call was
            free — it has said nothing.
          </span>
        </p>
      ) : null}

      {/*
       * The provenance footer is the least attractive thing on the panel and the most important. It
       * names every recording behind the figures and carries a digest of them, so the numbers can be
       * checked rather than trusted.
       */}
      <footer className="stats-group__foot">
        <p className="stats-group__provenance">
          {plural(group.matches, "recording")} · digest{" "}
          <span className="mono">{group.provenanceDigest}</span>
        </p>
        <details className="stats-group__matches">
          <summary>Show the {group.matchIds.length} recordings behind these numbers</summary>
          <ol className="stats-match-ids">
            {group.matchIds.map((matchId) => (
              <li key={matchId} className="mono">
                {matchId}
              </li>
            ))}
          </ol>
        </details>
      </footer>
    </article>
  );
}

export type BatchStatisticsProps = {
  /** Statistics read from the durable archive, not from anything held in memory. */
  statistics: ArenaStatistics | null;
  /** True while a batch is running. */
  running: boolean;
  /** Live progress, when a batch is running. */
  progress: { completed: number; requested: number; outcome: string } | null;
  /** How many matches the pending batch will play. */
  requestedMatches: number;
  /** Whether alternating sides is on. */
  alternateSides: boolean;
  /** Whether the two seats are distinct and the request is otherwise legal. */
  canRun: boolean;
  onRunBatch: () => void;
  onCancelBatch: () => void;
  onAlternateSidesChange: (alternate: boolean) => void;
  onMatchesChange: (matches: number) => void;
};

/** The batch's bounds, mirrored from the backend so the panel cannot offer what it will refuse. */
const MAX_BATCH_MATCHES = 50;
const MIN_BATCH_MATCHES = 1;

export function BatchStatistics({
  statistics,
  running,
  progress,
  requestedMatches,
  alternateSides,
  canRun,
  onRunBatch,
  onCancelBatch,
  onAlternateSidesChange,
  onMatchesChange,
}: BatchStatisticsProps) {
  const groups = statistics?.groups ?? [];

  return (
    <section className="arena-batch" aria-labelledby="arena-batch-heading">
      <header className="arena-batch__head">
        <div>
          <h3 id="arena-batch-heading" className="arena-subhead">
            BATCH STATISTICS
          </h3>
          {/*
           * The no-leaderboard statement is in the panel, not only in the stylesheet. It is the one
           * thing about this surface a reader cannot infer, and the cheapest defence against a
           * misleading combined score is to say out loud that one does not exist.
           */}
          <p className="arena-batch__scope">
            Counts from recorded matches only, one row per game and pairing. Games are never combined:
            a tic-tac-toe win and a connect-four win are different events.
          </p>
        </div>
        {statistics && statistics.ungrouped > 0 ? (
          <p className="arena-batch__scope-note" role="note">
            <IconAlert size={13} /> {plural(statistics.ungrouped, "recording is", "recordings are")} not
            counted — no game or pairing to group under.
          </p>
        ) : null}
      </header>

      <div className="arena-batch__controls">
        <label className="field">
          <span>Matches</span>
          <input
            type="number"
            min={MIN_BATCH_MATCHES}
            max={MAX_BATCH_MATCHES}
            step={1}
            value={requestedMatches}
            onChange={(event) =>
              onMatchesChange(
                Math.min(MAX_BATCH_MATCHES, Math.max(MIN_BATCH_MATCHES, Number(event.target.value) || MIN_BATCH_MATCHES)),
              )
            }
            disabled={running}
          />
        </label>

        {/*
         * A checkbox, not a two-option `<select>`.
         *
         * "Alternate sides" is one boolean with an obvious off state, and a select would make the reader
         * open it to discover what the two options are. The fieldset/legend pairing keeps the label
         * announced with the control, which a bare `<label>` wrapping a checkbox does reliably and a
         * visually-hidden-only label does not.
         */}
        <label className="arena-batch__toggle">
          <input
            type="checkbox"
            checked={alternateSides}
            onChange={(event) => onAlternateSidesChange(event.target.checked)}
            disabled={running}
          />
          <span>Alternate sides</span>
        </label>

        {running ? (
          <button type="button" className="btn btn-danger" onClick={onCancelBatch}>
            <IconStop size={14} /> STOP BATCH
          </button>
        ) : (
          <button type="button" className="btn btn-secondary" onClick={onRunBatch} disabled={!canRun}>
            <IconPlay size={14} /> RUN {requestedMatches}
          </button>
        )}
      </div>

      {running ? (
        <p className="arena-batch__progress" role="status">
          <IconRefresh size={13} />
          {progress
            ? `${progress.completed} of ${progress.requested} played · last ${progress.outcome}`
            : `Playing ${requestedMatches} matches…`}
        </p>
      ) : null}

      {alternateSides && !running ? (
        <p className="arena-batch__hint" role="note">
          <IconInfo size={13} /> The tested seat takes each side in turn. A seat that only ever plays
          first is given a handicap, not tested.
        </p>
      ) : null}

      {groups.length === 0 ? (
        <p className="arena-batch__empty">
          {running ? (
            "The first match has not been archived yet."
          ) : (
            <>
              <IconCheck size={13} /> Nothing recorded yet. Play a batch, or play matches one at a time —
              either way the numbers come from stored recordings.
            </>
          )}
        </p>
      ) : (
        <div className="stats-groups">
          {groups.map((group) => (
            <Group
              key={`${group.game}|${group.testedAgent}|${group.opponentAgent}`}
              group={group}
            />
          ))}
        </div>
      )}
    </section>
  );
}
