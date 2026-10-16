/**
 * The one place an OpenAI-compatible base URL is turned into a request root.
 *
 * # Why this module exists
 *
 * Two callers needed an API root and disagreed about what one was. The Lab path passed the operator's
 * configured URL through untouched. The Arena path built one by appending `/v1` to whatever it had:
 *
 * ```ts
 * baseUrl: `${baseUrl || "http://127.0.0.1:11434"}/v1`
 * ```
 *
 * The shipped default for that setting is `http://127.0.0.1:11434/v1` — it already carries the version
 * segment — so an Arena match configured with the default resolved to
 * `http://127.0.0.1:11434/v1/v1`, and the adapter then appended `/chat/completions` to produce
 * `/v1/v1/chat/completions`. No model seat could ever reach a provider.
 *
 * The reason it survived is that the two paths were tested separately and each was individually
 * correct: the adapter is right to append `/chat/completions` to a root, and the settings panel is right
 * to store a complete URL. Only the seam between them duplicated a segment, and no test crossed it.
 *
 * # The rule
 *
 * Append `/v1` only when the operator has given a bare host. A URL that already carries a path has
 * expressed intent — a reverse-proxied `/openai` prefix is a legitimate root — so it is preserved
 * exactly, minus a trailing slash.
 *
 * What this deliberately does not do is repair a URL the operator typed badly. `.../v1/v1` stays
 * `.../v1/v1`. Silently rewriting an operator's input hides a typo that would otherwise surface as a
 * clear 404 from their own endpoint, and a silent repair is worse than a visible failure. The duplication
 * this module removes is duplication *we* introduced, not one the operator asked for.
 */

/** The loopback root used when nothing is configured, matching the persisted settings default. */
export const DEFAULT_PROVIDER_BASE_URL = "http://127.0.0.1:11434/v1";

/** The version segment an OpenAI-compatible server exposes its API under. */
const API_VERSION_SEGMENT = "/v1";

/**
 * Resolves an operator-configured base URL to a single, unambiguous API root.
 *
 * @param raw The configured value. May be empty, whitespace, or absent.
 * @returns The API root. Guaranteed never to end in a slash.
 */
export function resolveProviderBaseUrl(raw: string | null | undefined): string {
  const trimmed = (raw ?? "").trim();
  if (trimmed.length === 0) {
    return DEFAULT_PROVIDER_BASE_URL;
  }

  const withoutTrailingSlash = trimmed.replace(/\/+$/, "");

  let parsed: URL;
  try {
    parsed = new URL(withoutTrailingSlash);
  } catch {
    /*
     * Not a URL we can reason about. It is passed through untouched so the backend's own validator --
     * which reports a precise reason -- is the thing that rejects it, rather than this module guessing
     * at the operator's intent with a string operation.
     */
    return withoutTrailingSlash;
  }

  /*
   * A bare host has no path. `http://127.0.0.1:11434` and `http://127.0.0.1:11434/` both report "/",
   * and neither carries the version segment, so both need one. Anything with a real path already says
   * where its API is.
   */
  const hasPath = parsed.pathname !== "" && parsed.pathname !== "/";
  if (hasPath) {
    return withoutTrailingSlash;
  }

  parsed.pathname = API_VERSION_SEGMENT;
  return parsed.toString().replace(/\/$/, "");
}

/**
 * Whether a resolved base URL will produce a request the adapter can actually route.
 *
 * Reported separately from `resolveProviderBaseUrl` so the UI can say *why* a match cannot start rather
 * than disabling a control and leaving the operator to guess.
 */
export function describeBaseUrlProblem(raw: string | null | undefined): string | null {
  const resolved = resolveProviderBaseUrl(raw);
  if (resolved.length === 0) {
    return "No provider endpoint is configured.";
  }
  try {
    const parsed = new URL(resolved);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      return `Provider endpoint must use http or https, not "${parsed.protocol.replace(":", "")}".`;
    }
    if (parsed.hostname.length === 0) {
      return "Provider endpoint has no host.";
    }
  } catch {
    return `Provider endpoint "${resolved}" is not a valid URL.`;
  }
  return null;
}