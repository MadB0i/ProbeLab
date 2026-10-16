import { describe, expect, it } from "vitest";

import { arenaMatchEligibility, type ArenaSeatLike } from "./arenaEligibility";

/**
 * Regression tests for audit finding B1.
 *
 * The defect was that `canPlay` in `App.tsx` required both seats to be reference policies, so selecting a
 * discovered model for either seat disabled Play Match. `ArenaPanel.test.tsx` supplied `canPlay: true`
 * directly, so the component under test never saw the faulty condition.
 *
 * These tests exercise the rule with real seat shapes rather than a boolean stub, which is the specific
 * gap the audit identified.
 */
const policy = (id: string): ArenaSeatLike => ({ kind: "policy", id });
const model = (label: string): ArenaSeatLike => ({ kind: "model", label });

const OLLAMA = "http://127.0.0.1:11434/v1";
const base = {
  gameCount: 2,
  testedSeat: policy("optimal_route"),
  opponentSeat: policy("greedy"),
  endpoint: OLLAMA,
};

describe("arenaMatchEligibility", () => {
  /*
   * The defect itself. Each of these three configurations was unreachable before the fix.
   */
  it("allows a model against a policy", () => {
    const result = arenaMatchEligibility({
      ...base,
      testedSeat: model("qwen2.5:0.5b"),
    });
    expect(result.reasons).toEqual([]);
    expect(result.canPlay).toBe(true);
  });

  it("allows a policy against a model", () => {
    const result = arenaMatchEligibility({
      ...base,
      opponentSeat: model("qwen2.5:0.5b"),
    });
    expect(result.canPlay).toBe(true);
  });

  it("allows a model against a different model", () => {
    const result = arenaMatchEligibility({
      ...base,
      testedSeat: model("qwen2.5:0.5b"),
      opponentSeat: model("llama3.2:1b"),
    });
    expect(result.canPlay).toBe(true);
  });

  it("still allows the reference-policy calibration match it always allowed", () => {
    const result = arenaMatchEligibility(base);
    expect(result.canPlay).toBe(true);
    expect(result.reasons).toEqual([]);
  });

  it("does not require an endpoint for a match with no model in it", () => {
    const result = arenaMatchEligibility({ ...base, endpoint: "not a url" });
    expect(result.canPlay).toBe(true);
  });

  it("requires a usable endpoint once a model is seated", () => {
    const result = arenaMatchEligibility({
      ...base,
      testedSeat: model("qwen2.5:0.5b"),
      endpoint: "ftp://example.com",
    });
    expect(result.canPlay).toBe(false);
    expect(result.reasons.join(" ")).toMatch(/http or https/);
  });

  it("refuses a seat that has not chosen a model yet", () => {
    const result = arenaMatchEligibility({ ...base, testedSeat: model("   ") });
    expect(result.canPlay).toBe(false);
    expect(result.reasons.join(" ")).toMatch(/tested seat/);
  });

  it("refuses a seat playing itself", () => {
    const result = arenaMatchEligibility({
      ...base,
      testedSeat: policy("greedy"),
      opponentSeat: policy("greedy"),
    });
    expect(result.canPlay).toBe(false);
    expect(result.reasons.join(" ")).toMatch(/cannot play itself/);
  });

  it("does not mistake a policy and a model sharing a name for the same participant", () => {
    const result = arenaMatchEligibility({
      ...base,
      testedSeat: policy("greedy"),
      opponentSeat: model("greedy"),
    });
    expect(result.canPlay).toBe(true);
  });

  it("refuses when no game is available", () => {
    const result = arenaMatchEligibility({ ...base, gameCount: 0 });
    expect(result.canPlay).toBe(false);
    expect(result.reasons.join(" ")).toMatch(/No Arena game/);
  });

  it("always explains itself, so a disabled control is never a dead end", () => {
    const failures = [
      { ...base, gameCount: 0 },
      { ...base, testedSeat: model("") },
      { ...base, testedSeat: policy("x"), opponentSeat: policy("x") },
      { ...base, testedSeat: model("m"), endpoint: "nonsense" },
    ];
    for (const input of failures) {
      const result = arenaMatchEligibility(input);
      expect(result.canPlay).toBe(false);
      expect(result.reasons.length).toBeGreaterThan(0);
      for (const reason of result.reasons) {
        expect(reason.trim().length).toBeGreaterThan(0);
      }
    }
  });
});