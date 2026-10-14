/**
 * Protocol compliance and action legality, as two separate numbers.
 *
 * # Why they cannot be one
 *
 * A model can answer every turn in perfect protocol and still spend the episode walking into walls.
 * "100% protocol" means every reply parsed as a well-formed action in the vocabulary the prompt
 * defines; it says nothing about whether the world would accept the move. Printed together as two
 * percentages on one card, they read as one verdict, and the flattering one is the one a reader takes
 * away.
 *
 * A real measured example, from `scripts/grid-connected-run.mjs`: `qwen2.5:0.5b` produced **11 of 11**
 * well-formed responses — a perfect protocol rate — and **8 of its 11 moves were refused** as illegal.
 * Both are true. Collapsing them into a single number would be a lie in one direction or the other.
 *
 * # What is derived and what is not
 *
 * Both rates come from counts the run already recorded:
 *
 * * protocol — `ProtocolAdherence::valid_response_rate_basis_points` from the independent evaluator;
 * * legality — the world's own `invalid_action_count`, against the turns it was asked to execute.
 *
 * Nothing here reclassifies a failure. A malformed response is a protocol failure and is *not* counted
 * as an illegal move, because the environment never saw one; an illegal move is a well-formed action the
 * world refused, and it *is* counted against legality. Moving either would change what the run
 * measured, and that is the evaluator's decision rather than a panel's.
 *
 * # Denominators are stated, not assumed
 *
 * Legality divides by model calls when there were any, because that is the number of chances to move.
 * With no calls there is no rate — `null`, not zero — and the caller renders an absence rather than a
 * `0.0%` that would read as a run where every move was refused.
 */

import type { CapabilityResult, ProtocolAdherence } from "./api";

export type ProtocolAndLegality = {
  /** Well-formed responses, as a percentage. From the evaluator's own basis points. */
  protocolPercent: number;
  /** Responses the evaluator counted as valid. */
  validResponses: number;
  /** Responses the evaluator counted as not valid. */
  invalidResponses: number;
  /** Turns the world was asked to execute. */
  turnsUsed: number;
  /** Moves the world refused as invalid. */
  refusedMoves: number;
  /** Moves the world accepted. Clamped at zero. */
  legalMoves: number;
  /** Accepted moves as a share of model calls, or `null` when there were none. */
  legalityPercent: number | null;
  /**
   * Both facts in one sentence, with the "separately" in it.
   *
   * Exists because the sentence is the part that survives being skimmed. Two bars above it can be
   * glanced at and only one read; this cannot be quoted without carrying both claims.
   */
  summary: string;
};

const formatCount = (value: number): string => value.toLocaleString("en-US");

/**
 * Reads both rates off one result.
 *
 * Takes the two structs rather than a whole `TestResult` so the arithmetic can be tested against a
 * recorded shape, and so a caller cannot pass a different run's protocol figure with this one's moves.
 */
export function protocolAndLegality(
  capability: CapabilityResult,
  adherence: ProtocolAdherence,
): ProtocolAndLegality {
  const protocolPercent = adherence.valid_response_rate_basis_points / 100;
  const refusedMoves = capability.invalid_action_count;
  // Clamped: a run that was cancelled mid-turn can report more refused moves than it reports turns, and
  // a negative count of accepted moves would be a number about nothing.
  const legalMoves = Math.max(0, capability.turns_used - refusedMoves);
  const legalityPercent =
    adherence.total_model_calls > 0
      ? (legalMoves / adherence.total_model_calls) * 100
      : null;
  const invalidResponses = Math.max(0, adherence.total_model_calls - adherence.valid_responses);

  const summary =
    `${formatCount(adherence.total_model_calls)} model call${adherence.total_model_calls === 1 ? "" : "s"}; ` +
    `${formatCount(adherence.valid_responses)} returned a response that parsed as a valid action` +
    `${invalidResponses > 0 ? `, and ${formatCount(invalidResponses)} did not` : ""}. ` +
    `Separately, ${formatCount(legalMoves)} of ${formatCount(adherence.total_model_calls)} of those moves were ` +
    `accepted by the world and ${formatCount(refusedMoves)} were refused. ` +
    `Protocol is about the shape of the reply; legality is about whether the world would let it happen.`;

  return {
    protocolPercent,
    validResponses: adherence.valid_responses,
    invalidResponses,
    turnsUsed: capability.turns_used,
    refusedMoves,
    legalMoves,
    legalityPercent,
    summary,
  };
}

/**
 * The shortest true sentence about whether the two agree.
 *
 * "Both perfect" is worth saying when it is true, because a run that is genuinely well-behaved should
 * not be made to look suspicious by a panel that only ever warns.
 */
export function agreementLabel(reading: ProtocolAndLegality): string | null {
  if (reading.protocolPercent === 100 && reading.refusedMoves === 0) {
    return "Every response was well formed and every move was legal.";
  }
  if (reading.protocolPercent === 100 && reading.refusedMoves > 0) {
    return (
      `Every response was well formed, and ${formatCount(reading.refusedMoves)} of ` +
      `${formatCount(reading.turnsUsed)} moves were still refused. A perfect protocol rate is not a ` +
      `perfect run.`
    );
  }
  if (reading.protocolPercent < 100 && reading.refusedMoves === 0) {
    return "Some responses did not parse, and every move the world saw was legal.";
  }
  return null;
}