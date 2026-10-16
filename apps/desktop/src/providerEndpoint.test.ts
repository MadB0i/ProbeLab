import { describe, expect, it } from "vitest";

import {
  DEFAULT_PROVIDER_BASE_URL,
  describeBaseUrlProblem,
  resolveProviderBaseUrl,
} from "./providerEndpoint";

/**
 * The regression tests for audit finding B2: the Arena appended `/v1` to a configured URL that already
 * carried it, producing `/v1/v1` and, once the adapter appended its own suffix, `/v1/v1/chat/completions`.
 *
 * The shipped settings default is `http://127.0.0.1:11434/v1`, so the first case below is not a
 * hypothetical: it is what every default-configured Arena model match used to send.
 */
describe("resolveProviderBaseUrl", () => {
  it("does not duplicate the version segment when the configured URL already has one", () => {
    // The exact value stored in crates/lab/src/settings.rs as the provider default.
    expect(resolveProviderBaseUrl("http://127.0.0.1:11434/v1")).toBe("http://127.0.0.1:11434/v1");
  });

  it("is idempotent, so applying it twice cannot produce a second segment", () => {
    const once = resolveProviderBaseUrl("http://127.0.0.1:11434/v1");
    expect(resolveProviderBaseUrl(once)).toBe(once);
  });

  it("appends the version segment to a bare host", () => {
    expect(resolveProviderBaseUrl("http://127.0.0.1:11434")).toBe("http://127.0.0.1:11434/v1");
  });

  it("treats a bare host with a trailing slash as a bare host", () => {
    expect(resolveProviderBaseUrl("http://127.0.0.1:8080/")).toBe("http://127.0.0.1:8080/v1");
  });

  it("strips a trailing slash from a URL that already has the version segment", () => {
    expect(resolveProviderBaseUrl("http://127.0.0.1:11434/v1/")).toBe("http://127.0.0.1:11434/v1");
  });

  it("strips repeated trailing slashes", () => {
    expect(resolveProviderBaseUrl("http://127.0.0.1:11434/v1///")).toBe("http://127.0.0.1:11434/v1");
  });

  it("preserves a custom path, because a reverse-proxied root is a real configuration", () => {
    expect(resolveProviderBaseUrl("https://gw.example/openai")).toBe("https://gw.example/openai");
  });

  it("preserves a non-v1 version segment rather than stacking a second one", () => {
    expect(resolveProviderBaseUrl("https://generativelanguage.googleapis.com/v1beta")).toBe(
      "https://generativelanguage.googleapis.com/v1beta",
    );
  });

  it("falls back to the shipped default when nothing is configured", () => {
    expect(resolveProviderBaseUrl("")).toBe(DEFAULT_PROVIDER_BASE_URL);
    expect(resolveProviderBaseUrl("   ")).toBe(DEFAULT_PROVIDER_BASE_URL);
    expect(resolveProviderBaseUrl(null)).toBe(DEFAULT_PROVIDER_BASE_URL);
    expect(resolveProviderBaseUrl(undefined)).toBe(DEFAULT_PROVIDER_BASE_URL);
  });

  /*
   * The default must itself be a fixed point. It is the value the settings panel persists, so if the
   * default ever stopped being its own normalisation, every Arena model match would break again the
   * first time an operator accepted the pre-filled value without editing it.
   */
  it("leaves the shipped default unchanged", () => {
    expect(resolveProviderBaseUrl(DEFAULT_PROVIDER_BASE_URL)).toBe(DEFAULT_PROVIDER_BASE_URL);
    expect(DEFAULT_PROVIDER_BASE_URL.endsWith("/v1")).toBe(true);
    expect(DEFAULT_PROVIDER_BASE_URL.endsWith("/v1/v1")).toBe(false);
  });

  it("preserves a non-URL string so the backend validator, not this module, reports it", () => {
    expect(resolveProviderBaseUrl("not a url")).toBe("not a url");
  });
});

/**
 * The composed endpoint, as the adapter would build it.
 *
 * Asserting the full string is what makes this a regression test rather than a unit test: the defect was
 * only ever visible in the composition, and `App.tsx` and the adapter each tested correctly on their own.
 */
describe("the composed chat-completions endpoint", () => {
  const compose = (raw: string) => `${resolveProviderBaseUrl(raw)}/chat/completions`;

  it("never contains a duplicated version segment for any supported input", () => {
    const inputs = [
      "http://127.0.0.1:11434",
      "http://127.0.0.1:11434/",
      "http://127.0.0.1:11434/v1",
      "http://127.0.0.1:11434/v1/",
      "http://127.0.0.1:8080",
      "https://gw.example/openai",
      "",
    ];
    for (const input of inputs) {
      expect(compose(input)).not.toContain("/v1/v1");
    }
  });

  it("sends the default configuration to the documented Ollama chat endpoint", () => {
    expect(compose("http://127.0.0.1:11434/v1")).toBe("http://127.0.0.1:11434/v1/chat/completions");
  });
});

describe("describeBaseUrlProblem", () => {
  it("reports no problem for a valid endpoint", () => {
    expect(describeBaseUrlProblem("http://127.0.0.1:11434/v1")).toBeNull();
    expect(describeBaseUrlProblem("https://api.example/v1")).toBeNull();
  });

  it("names the reason rather than leaving a disabled control unexplained", () => {
    expect(describeBaseUrlProblem("ftp://example.com")).toMatch(/http or https/);
    expect(describeBaseUrlProblem("not a url")).toMatch(/not a valid URL/);
  });
});