/**
 * Protocol compliance and action legality must never be reported as one number.
 *
 * Every case here is a shape a real run can produce, and the assertions are about what the panel is
 * *allowed to imply* rather than about arithmetic — arithmetic a wrong implementation gets wrong
 * visibly, and an implication gets wrong invisibly.
 *
 * The measured case the module exists for: `qwen2.5:0.5b` on the grid world, 11 of 11 responses well
 * formed, 8 of 11 moves refused. Both facts are in `apps/desktop` and the run is reproducible with
 * `scripts/grid-connected-run.mjs`.
 */

import { describe, expect, it } from "vitest";

import { agreementLabel, protocolAndLegality } from "./capabilityRates";
import type { CapabilityResult, ProtocolAdherence } from "./api";

function adherence(overrides: Partial<ProtocolAdherence> = {}): ProtocolAdherence {
  return {
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
    ...overrides,
  };
}

function capability(overrides: Partial<CapabilityResult> = {}): CapabilityResult {
  return {
    completion_status: "turn_limit",
    task_success: false,
    turns_used: 0,
    total_reward: 0,
    invalid_action_count: 0,
    protocol_adherence: adherence(),
    ...overrides,
  } as CapabilityResult;
}

describe("the measured case: perfect protocol, mostly illegal", () => {
  // qwen2.5:0.5b, one grid episode, 11 committed turns.
  const reading = protocolAndLegality(
    capability({ turns_used: 11, invalid_action_count: 8 }),
    adherence({ total_model_calls: 11, valid_responses: 11, valid_response_rate_basis_points: 10000 }),
  );

  it("reports the protocol rate at full marks, because it was", () => {
    expect(reading.protocolPercent).toBe(100);
  });

  it("reports the legality rate well below it, because that is a different fact", () => {
    expect(reading.legalityPercent).toBeCloseTo((3 / 11) * 100, 5);
    expect(reading.legalityPercent).toBeLessThan(50);
  });

  it("never lets the two collapse into one figure", () => {
    expect(reading.protocolPercent).not.toBe(reading.legalityPercent);
  });

  it("says so in words, and says it before anyone has to do the division", () => {
    expect(reading.summary).toContain("11 model calls");
    expect(reading.summary).toContain("8 were refused");
    expect(reading.summary).toContain("Separately");
    expect(reading.summary).toContain("Protocol is about the shape of the reply");
  });

  it("names the case explicitly, because a 100% with refused moves is the one that misleads", () => {
    const label = agreementLabel(reading);
    expect(label).toContain("A perfect protocol rate is not a perfect run.");
  });
});

describe("denominators", () => {
  it("reports no legality rate at all when no model was called", () => {
    // Zero would read as "every move was refused", which is not what a run with no calls means.
    const reading = protocolAndLegality(
      capability({ turns_used: 0, invalid_action_count: 0 }),
      adherence(),
    );
    expect(reading.legalityPercent).toBeNull();
    expect(reading.legalMoves).toBe(0);
  });

  it("counts a non-valid response as a protocol failure and not as an illegal move", () => {
    // The environment never saw an action, so it cannot have refused one. Counting it twice would
    // inflate the illegal-move figure with failures the world never heard about.
    const reading = protocolAndLegality(
      capability({ turns_used: 5, invalid_action_count: 1 }),
      adherence({
        total_model_calls: 5,
        valid_responses: 3,
        parse_failures: 2,
        valid_response_rate_basis_points: 6000,
      }),
    );
    expect(reading.invalidResponses).toBe(2);
    expect(reading.refusedMoves).toBe(1);
    expect(reading.legalMoves).toBe(4);
    expect(reading.legalityPercent).toBeCloseTo(80, 5);
  });

  it("clamps accepted moves at zero when a stopped run reports more refusals than turns", () => {
    // A run cancelled mid-turn can land here. A negative count of accepted moves is a number about
    // nothing, and it would render as a negative percentage.
    const reading = protocolAndLegality(
      capability({ turns_used: 2, invalid_action_count: 5 }),
      adherence({ total_model_calls: 4, valid_responses: 4, valid_response_rate_basis_points: 10000 }),
    );
    expect(reading.legalMoves).toBe(0);
    expect(reading.legalityPercent).toBe(0);
  });
});

describe("the cases a panel should not make suspicious", () => {
  it("says nothing extra when both really were clean", () => {
    const reading = protocolAndLegality(
      capability({ turns_used: 7, invalid_action_count: 0 }),
      adherence({ total_model_calls: 7, valid_responses: 7, valid_response_rate_basis_points: 10000 }),
    );
    expect(reading.protocolPercent).toBe(100);
    expect(reading.legalityPercent).toBe(100);
    expect(agreementLabel(reading)).toBe("Every response was well formed and every move was legal.");
  });

  it("distinguishes unparseable replies from legal ones", () => {
    const reading = protocolAndLegality(
      capability({ turns_used: 7, invalid_action_count: 0 }),
      adherence({
        total_model_calls: 7,
        valid_responses: 5,
        parse_failures: 2,
        valid_response_rate_basis_points: 7142,
      }),
    );
    expect(agreementLabel(reading)).toBe(
      "Some responses did not parse, and every move the world saw was legal.",
    );
  });

  it("has nothing to add when both problems are present", () => {
    const reading = protocolAndLegality(
      capability({ turns_used: 7, invalid_action_count: 3 }),
      adherence({
        total_model_calls: 7,
        valid_responses: 5,
        parse_failures: 2,
        valid_response_rate_basis_points: 7142,
      }),
    );
    expect(agreementLabel(reading)).toBeNull();
    expect(reading.summary).toContain("2 did not");
  });
});