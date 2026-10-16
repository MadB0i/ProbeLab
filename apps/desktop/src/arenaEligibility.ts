/**
 * Whether a single Arena match can actually be launched, and if not, why not.
 *
 * # Why this is not an inline expression
 *
 * It was:
 *
 * ```tsx
 * canPlay={
 *   arenaTestedSeat.kind === "policy" &&
 *   arenaOpponentSeat.kind === "policy" &&
 *   arenaGames.length > 0
 * }
 * ```
 *
 * which requires *both* seats to be reference policies. Selecting a discovered model for either seat
 * therefore disabled Play Match, making the central advertised workflow — a model competing in the Arena
 * — unreachable through the one control that starts it.
 *
 * The reason it survived is stated in `ArenaPanel.test.tsx`, which supplies `canPlay: true`: the panel
 * was tested with an eligibility it was never given, and the component that computes the real one has no
 * test at all. Extracting the rule makes it testable on its own and, more importantly, testable with the
 * *real* seat shapes rather than a boolean stub.
 *
 * The other half of the problem is that a disabled control used to explain nothing. Every refusal below
 * carries a sentence, so the UI can put the reason beside the button instead of leaving the operator to
 * infer it from a control that will not respond.
 */
import { describeBaseUrlProblem } from "./providerEndpoint";

/** The subset of a seat this rule needs. A model seat's config is not its identity. */
export type ArenaSeatLike =
  | { kind: "policy"; id: string }
  | { kind: "model"; label: string };

export type ArenaEligibilityInput = {
  /** How many games the catalog currently offers. Zero means the selector is empty. */
  gameCount: number;
  testedSeat: ArenaSeatLike;
  opponentSeat: ArenaSeatLike;
  /**
   * The resolved provider endpoint. Only consulted when a model is actually seated — a policy-versus-
   * policy match needs no endpoint, and demanding one would disable the calibration workflow that has
   * no model in it.
   */
  endpoint: string;
};

export type ArenaEligibility = {
  canPlay: boolean;
  /** Human-readable refusals. Empty when `canPlay` is true. */
  reasons: string[];
};

/**
 * A stable identity for a seat, used only to stop a seat being asked to play itself.
 *
 * Model seats are compared by label because that is what the picker can distinguish them by. This is
 * deliberately *not* a configuration fingerprint — two labels can name the same weights — and it is not
 * used for statistics grouping. It answers exactly one question: did the operator pick the same entry
 * twice?
 */
function seatIdentity(seat: ArenaSeatLike): string {
  return seat.kind === "policy" ? `policy:${seat.id.trim()}` : `model:${seat.label.trim()}`;
}

function seatProblem(label: string, seat: ArenaSeatLike): string | null {
  if (seat.kind === "model" && seat.label.trim().length === 0) {
    return `Choose which model plays the ${label} seat.`;
  }
  if (seat.kind === "policy" && seat.id.trim().length === 0) {
    return `Choose a policy for the ${label} seat.`;
  }
  return null;
}

/**
 * Decides whether a single Arena match can start.
 *
 * The conditions, in the order an operator would hit them:
 *
 * 1. There is a game to play.
 * 2. Both seats are fully chosen — a model seat with no model named is not a seat.
 * 3. The seats are not the same participant, which the backend also refuses.
 * 4. If and only if a model is seated, the provider endpoint is usable.
 */
export function arenaMatchEligibility(input: ArenaEligibilityInput): ArenaEligibility {
  const reasons: string[] = [];

  if (input.gameCount === 0) {
    reasons.push("No Arena game is available.");
  }

  const tested = seatProblem("tested", input.testedSeat);
  if (tested) reasons.push(tested);
  const opponent = seatProblem("opponent", input.opponentSeat);
  if (opponent) reasons.push(opponent);

  if (
    !tested &&
    !opponent &&
    seatIdentity(input.testedSeat) === seatIdentity(input.opponentSeat)
  ) {
    reasons.push("A seat cannot play itself. Choose two different participants.");
  }

  /*
   * Only a match with a model in it needs an endpoint. Checking unconditionally would disable the
   * reference-policy calibration workflow, which has no provider configured and does not need one.
   */
  const seatsAModel =
    input.testedSeat.kind === "model" || input.opponentSeat.kind === "model";
  if (seatsAModel) {
    const endpointProblem = describeBaseUrlProblem(input.endpoint);
    if (endpointProblem) reasons.push(endpointProblem);
  }

  return { canPlay: reasons.length === 0, reasons };
}