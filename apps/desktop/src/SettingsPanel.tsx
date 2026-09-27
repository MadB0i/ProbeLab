/**
 * The settings panel.
 *
 * # What this panel is and is not
 *
 * It renders a backend-owned document and sends changes back to be validated. It is not an
 * authoritative copy of the configuration: there is no local state that takes effect without a round
 * trip, and what is displayed after a save is what the backend *stored*, not what was typed. A panel
 * that trusted its own form state would show a value the backend refused, which is the failure this
 * one is built to avoid.
 *
 * # The three states that matter
 *
 * * **Unchanged** — the form matches what is stored, and Save is disabled because there is nothing
 *   to save. A Save button that is always live invites a no-op write and hides whether a change is
 *   actually pending.
 * * **Dirty** — the form differs from the stored document, and the difference is stated, so a person
 *   can see what they are about to change before they change it.
 * * **Saved** — after a successful write, the panel says so and re-renders from the stored document.
 *   A save that reports success from local state alone would claim a persistence it never performed.
 *
 * # Read-only values are shown, not hidden
 *
 * The storage paths, the credential policy, and the log configuration are displayed with the reason
 * they cannot be edited. Hiding a value that exists leaves a reader to conclude it does not exist,
 * and offering a control with no backend behind it is worse than either.
 *
 * # There is no credential field
 *
 * The document has no field that could hold one, and this panel has no control that could introduce
 * one. What it shows is `credentialPresent: true/false` plus the *name* of the environment variable to
 * export — which is what a person needs in order to fix a missing credential, and is not the
 * credential.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  applySettings,
  readSettings,
  resetSettings,
  type DefaultRunMode,
  type ExperimentDefaults,
  type LabSettings,
  type ProviderDefaults,
  type SeedBehaviour,
  type SettingsCommandError,
  type SettingsView,
} from "./api";

/** Formats a settings error into something a person can act on. */
function describeError(error: unknown): { field: string | null; reason: string } {
  if (error && typeof error === "object" && "kind" in error) {
    const typed = error as SettingsCommandError;
    if (typed.kind === "storage" && "reason" in typed.detail) {
      return { field: null, reason: (typed.detail as { reason: string }).reason };
    }
    if ("field" in typed.detail) {
      const detail = typed.detail as { field: string | null; reason: string };
      return { field: detail.field, reason: detail.reason };
    }
  }
  return {
    field: null,
    reason: error instanceof Error ? error.message : String(error),
  };
}

/** Whether two documents are equal, so "dirty" is a fact rather than a guess. */
function sameSettings(left: LabSettings, right: LabSettings): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

export function SettingsPanel() {
  const [stored, setStored] = useState<SettingsView | null>(null);
  const [draft, setDraft] = useState<LabSettings | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [error, setError] = useState<{ field: string | null; reason: string } | null>(null);
  /** The field the error belongs to, so the message can be placed next to its control. */
  const [errorField, setErrorField] = useState<string | null>(null);
  const [confirmation, setConfirmation] = useState<string | null>(null);

  const load = useCallback(() => {
    setLoading(true);
    setError(null);
    readSettings()
      .then((view) => {
        setStored(view);
        setDraft(view.settings);
        setLoading(false);
      })
      .catch((cause: unknown) => {
        const described = describeError(cause);
        setError(described);
        setLoading(false);
      });
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const dirty = useMemo(
    () => stored !== null && draft !== null && !sameSettings(stored.settings, draft),
    [stored, draft],
  );

  /**
   * Applies a partial change to one section of the document.
   *
   * Written with an explicit per-section signature rather than a generic one, because the sections
   * have different types and a generic spread over `LabSettings[K]` is a spread over a type parameter
   * that TypeScript cannot narrow. The result is three lines longer and it type-checks.
   */
  const update = useCallback(
    (
      section: "experimentDefaults" | "providerDefaults",
      patch: Partial<ExperimentDefaults> | Partial<ProviderDefaults>,
    ) => {
      setConfirmation(null);
      setError(null);
      setErrorField(null);
      setDraft((current) => {
        if (current === null) {
          return current;
        }
        if (section === "experimentDefaults") {
          return {
            ...current,
            experimentDefaults: {
              ...current.experimentDefaults,
              ...(patch as Partial<ExperimentDefaults>),
            },
          };
        }
        return {
          ...current,
          providerDefaults: {
            ...current.providerDefaults,
            ...(patch as Partial<ProviderDefaults>),
          },
        };
      });
    },
    [],
  );

  const save = useCallback(() => {
    if (draft === null) {
      return;
    }
    setSaving(true);
    setError(null);
    setErrorField(null);
    setConfirmation(null);
    applySettings(draft)
      .then((view) => {
        // The stored document, not the submitted one. If the backend adjusted or refused something,
        // this is what it will now show, which is the only thing worth displaying.
        setStored(view);
        setDraft(view.settings);
        setConfirmation("Settings saved. They will be used for future experiments and runs.");
      })
      .catch((cause: unknown) => {
        const described = describeError(cause);
        setError(described);
        setErrorField(described.field);
      })
      .finally(() => setSaving(false));
  }, [draft]);

  const reset = useCallback(() => {
    setResetting(true);
    setError(null);
    setErrorField(null);
    setConfirmation(null);
    resetSettings()
      .then((view) => {
        setStored(view);
        setDraft(view.settings);
        setConfirmation(
          "Settings restored to their defaults. Stored results, telemetry, and replay data were not touched.",
        );
      })
      .catch((cause: unknown) => {
        const described = describeError(cause);
        setError(described);
        setErrorField(described.field);
      })
      .finally(() => setResetting(false));
  }, []);

  /** The inline error for one control, shown beside it rather than only at the top. */
  const fieldError = (id: string): string | null =>
    error !== null && errorField === id ? error.reason : null;

  if (loading) {
    return (
      <section className="panel settings-panel" aria-label="Settings">
        <div className="panel-heading">
          <h2>Settings</h2>
        </div>
        <div className="state-message">Reading the stored settings…</div>
      </section>
    );
  }

  if (stored === null || draft === null) {
    return (
      <section className="panel settings-panel" aria-label="Settings">
        <div className="panel-heading">
          <h2>Settings</h2>
        </div>
        {error && (
          <div className="error-message" role="alert">
            {error.reason}
          </div>
        )}
        <p className="field-description">
          The settings could not be read, so nothing is shown. Editing a form that has not been loaded
          would risk overwriting values the panel never saw.
        </p>
        <button type="button" className="run-button run-button-secondary" onClick={load}>
          <span>TRY AGAIN</span>
        </button>
      </section>
    );
  }

  const providerState = stored.providerDefaults.state;

  return (
    <div className="settings-stack">
      <section className="panel settings-panel" aria-label="Settings">
        <div className="panel-heading">
          <h2>Settings</h2>
          <span className="muted">
            {stored.isDefault ? "BUILT-IN DEFAULTS" : "CUSTOMISED"} · SCHEMA V
            {stored.settings.schemaVersion}
          </span>
        </div>
        <p className="field-description">
          These are the defaults future experiments and runs start from. A run you start can still be
          given different values, and a result already recorded is never changed by anything here.
        </p>

        {confirmation && (
          <div className="settings-confirmation" role="status">
            {confirmation}
          </div>
        )}
        {error && errorField === null && (
          <div className="error-message" role="alert">
            {error.reason}
          </div>
        )}

        <div className="settings-actions">
          <button
            type="button"
            className="run-button"
            onClick={save}
            disabled={!dirty || saving}
          >
            <span>{saving ? "SAVING" : "SAVE CHANGES"}</span>
            <span className="button-arrow" aria-hidden="true">
              →
            </span>
          </button>
          <button
            type="button"
            className="run-button run-button-secondary"
            onClick={reset}
            disabled={resetting || (stored.isDefault && !dirty)}
          >
            <span>{resetting ? "RESTORING" : "RESTORE DEFAULTS"}</span>
          </button>
          <span className="muted">
            {dirty
              ? "Unsaved changes. They take effect only for runs started after saving."
              : stored.isDefault
                ? "These are the built-in defaults."
                : "Saved."}
          </span>
        </div>
      </section>

      <section className="panel settings-panel" aria-label="Experiment defaults">
        <div className="panel-heading">
          <h2>Experiment defaults</h2>
          <span className="muted">AFFECTS FUTURE EXPERIMENTS</span>
        </div>

        <div className="field-group">
          <label htmlFor="setting-episodes">EPISODES</label>
          <input
            id="setting-episodes"
            type="number"
            min={1}
            max={stored.experimentDefaults.maxEpisodes}
            value={draft.experimentDefaults.episodes}
            onChange={(event) =>
              update("experimentDefaults", {
                episodes: Number(event.target.value) || 0,
              })
            }
            aria-describedby="setting-episodes-help"
            aria-invalid={fieldError("experiment_defaults.episodes") !== null}
          />
          <p className="field-description" id="setting-episodes-help">
            Between 1 and {stored.experimentDefaults.maxEpisodes}. The lab's planner refuses anything
            beyond that, so a default outside the range would be a value the lab would reject.
          </p>
          {fieldError("experiment_defaults.episodes") && (
            <p className="field-error" role="alert">
              {fieldError("experiment_defaults.episodes")}
            </p>
          )}
        </div>

        <div className="field-group">
          <label htmlFor="setting-seed-behaviour">SEED</label>
          <select
            id="setting-seed-behaviour"
            value={draft.experimentDefaults.seedBehaviour}
            onChange={(event) =>
              update("experimentDefaults", {
                seedBehaviour: event.target.value as SeedBehaviour,
              })
            }
            aria-describedby="setting-seed-help"
          >
            <option value="planner_chosen">Chosen by the planner</option>
            <option value="fixed">Fixed, from the value below</option>
          </select>
          <p className="field-description" id="setting-seed-help">
            {stored.experimentDefaults.effectiveRootSeed === null
              ? "The planner chooses each run's seed, so a run is not pinned to one environment."
              : `A fixed seed of ${stored.experimentDefaults.effectiveRootSeed} will be sent with every new experiment, so each runs on the same environment.`}
          </p>
        </div>

        <div className="field-group">
          <label htmlFor="setting-root-seed">ROOT SEED</label>
          <input
            id="setting-root-seed"
            type="number"
            min={0}
            value={draft.experimentDefaults.rootSeed}
            disabled={draft.experimentDefaults.seedBehaviour !== "fixed"}
            onChange={(event) =>
              update("experimentDefaults", {
                rootSeed: Number(event.target.value) || 0,
              })
            }
            aria-describedby="setting-root-seed-help"
          />
          <p className="field-description" id="setting-root-seed-help">
            {draft.experimentDefaults.seedBehaviour === "fixed"
              ? "Used for every new experiment."
              : "Not used while the seed is chosen by the planner."}
          </p>
        </div>
      </section>

      <section className="panel settings-panel" aria-label="Providers">
        <div className="panel-heading">
          <h2>Providers</h2>
          <span className="muted">CONNECTED RUN DEFAULTS</span>
        </div>

        <div className={`settings-provider-state ${providerState.configured ? "is-ready" : "is-not-ready"}`}>
          <div>
            <span className="settings-provider-label">
              {providerState.configured ? "CONFIGURED" : "NOT CONFIGURED"}
            </span>
            <strong>{providerState.provider}</strong>
            <span className="muted"> / {providerState.model}</span>
          </div>
          <p className="field-description">{providerState.message}</p>
        </div>

        <p className="settings-secret-policy">{stored.providerDefaults.secretPolicy}</p>

        <div className="field-group">
          <label htmlFor="setting-default-mode">RUN BY DEFAULT</label>
          <select
            id="setting-default-mode"
            value={draft.defaultRunMode}
            onChange={(event) =>
              setDraft((current) =>
                current === null
                  ? current
                  : { ...current, defaultRunMode: event.target.value as DefaultRunMode },
              )
            }
            aria-describedby="setting-default-mode-help"
          >
            <option value="mock">The deterministic mock</option>
            <option value="connected">The connected provider</option>
          </select>
          <p className="field-description" id="setting-default-mode-help">
            Which one the command center offers first. A connected run still requires a credential in
            the environment; this cannot make one happen without it.
          </p>
        </div>

        <div className="field-group">
          <label htmlFor="setting-base-url">BASE URL</label>
          <input
            id="setting-base-url"
            type="text"
            spellCheck={false}
            autoComplete="off"
            value={draft.providerDefaults.baseUrl}
            onChange={(event) => update("providerDefaults", { baseUrl: event.target.value })}
            aria-describedby="setting-base-url-help"
            aria-invalid={fieldError("provider_defaults.base_url") !== null}
          />
          <p className="field-description" id="setting-base-url-help">
            Must be HTTPS, except for loopback HTTP. A URL with credentials, a query string, or a
            fragment in it is refused, because each of those is a way to smuggle a key into a request.
          </p>
          {fieldError("provider_defaults.base_url") && (
            <p className="field-error" role="alert">
              {fieldError("provider_defaults.base_url")}
            </p>
          )}
        </div>

        <div className="field-group">
          <label htmlFor="setting-model">MODEL</label>
          <input
            id="setting-model"
            type="text"
            spellCheck={false}
            autoComplete="off"
            value={draft.providerDefaults.model}
            onChange={(event) => update("providerDefaults", { model: event.target.value })}
            aria-invalid={fieldError("provider_defaults.model") !== null}
          />
          {fieldError("provider_defaults.model") && (
            <p className="field-error" role="alert">
              {fieldError("provider_defaults.model")}
            </p>
          )}
        </div>

        <div className="field-group">
          <label htmlFor="setting-timeout">TIMEOUT (S)</label>
          <input
            id="setting-timeout"
            type="number"
            min={1}
            max={600}
            value={draft.providerDefaults.timeoutSeconds}
            onChange={(event) =>
              update("providerDefaults", {
                timeoutSeconds: Number(event.target.value) || 0,
              })
            }
            aria-describedby="setting-timeout-help"
            aria-invalid={fieldError("provider_defaults.timeout_seconds") !== null}
          />
          <p className="field-description" id="setting-timeout-help">
            How long a single provider request may take. Zero is refused: that is not an instant
            request, it is one that has already failed.
          </p>
          {fieldError("provider_defaults.timeout_seconds") && (
            <p className="field-error" role="alert">
              {fieldError("provider_defaults.timeout_seconds")}
            </p>
          )}
        </div>

        <div className="field-group">
          <label htmlFor="setting-key-env">API KEY ENV VAR (NAME ONLY)</label>
          <input
            id="setting-key-env"
            type="text"
            spellCheck={false}
            autoComplete="off"
            value={draft.providerDefaults.apiKeyEnv}
            onChange={(event) => update("providerDefaults", { apiKeyEnv: event.target.value })}
            aria-describedby="setting-key-env-help"
            aria-invalid={fieldError("provider_defaults.api_key_env") !== null}
          />
          <p className="field-description" id="setting-key-env-help">
            The <em>name</em> of the environment variable to read, not its value. ProbeLab never
            accepts, stores, or displays a key — it is read from the process environment at run time.
            {providerState.credentialPresent
              ? " A credential is currently present in this process's environment."
              : ` No credential was found in ${draft.providerDefaults.apiKeyEnv} in this process.`}
          </p>
          {fieldError("provider_defaults.api_key_env") && (
            <p className="field-error" role="alert">
              {fieldError("provider_defaults.api_key_env")}
            </p>
          )}
        </div>
      </section>

      <section className="panel settings-panel" aria-label="Storage and fixed configuration">
        <div className="panel-heading">
          <h2>Storage and fixed configuration</h2>
          <span className="muted">READ-ONLY</span>
        </div>
        <p className="field-description">
          These are real values the lab uses. They are shown rather than offered as controls, because
          the architecture fixes them: a control that appeared to move them and did not would be worse
          than an honest read-only field.
        </p>
        <dl className="settings-readonly">
          {stored.readOnlyFacts.map((fact) => (
            <div key={fact.id}>
              <dt>{fact.label}</dt>
              <dd>
                <code>{fact.value}</code>
                <p className="field-description field-description-quiet">{fact.reason}</p>
              </dd>
            </div>
          ))}
        </dl>
      </section>
    </div>
  );
}
