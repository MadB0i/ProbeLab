/**
 * Settings panel tests.
 *
 * These test state transitions, not the presence of controls. A settings panel whose Save button
 * exists but whose save does nothing looks identical to one that works, right up until someone relies
 * on it — which is why every test below drives a change through to a rendered consequence.
 *
 * The properties that matter most:
 *
 * 1. **The panel renders from the backend, not from its own form state.** After a save it displays the
 *    document the backend returned, so a value the backend refused cannot linger on screen.
 * 2. **A rejected save leaves the stored settings alone and shows the error against its field.**
 * 3. **Save is disabled until something actually changes**, and says why.
 * 4. **There is no credential input.** Asserted by searching for one, because "we didn't add a field"
 *    is a claim that a test can check.
 */

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SettingsPanel } from "./SettingsPanel";
import * as api from "./api";
import type { LabSettings, SettingsView } from "./api";

const defaults = (): LabSettings => ({
  schemaVersion: 1,
  experimentDefaults: { episodes: 5, seedBehaviour: "planner_chosen", rootSeed: 0 },
  providerDefaults: {
    provider: "openai_compatible",
    baseUrl: "http://127.0.0.1:11434/v1",
    apiKeyEnv: "OPENAI_COMPATIBLE_API_KEY",
    model: "local-model",
    timeoutSeconds: 30,
  },
  defaultRunMode: "mock",
});

const view = (settings: LabSettings = defaults()): SettingsView => ({
  settings,
  experimentDefaults: {
    episodes: { id: "experiment_defaults.episodes", value: settings.experimentDefaults.episodes, scope: "editable", readOnlyReason: null },
    seedBehaviour: { id: "experiment_defaults.seedBehaviour", value: settings.experimentDefaults.seedBehaviour, scope: "editable", readOnlyReason: null },
    rootSeed: { id: "experiment_defaults.root_seed", value: settings.experimentDefaults.rootSeed, scope: "editable", readOnlyReason: null },
    maxEpisodes: 1000,
    effectiveRootSeed:
      settings.experimentDefaults.seedBehaviour === "fixed"
        ? settings.experimentDefaults.rootSeed
        : null,
  },
  providerDefaults: {
    provider: { id: "provider_defaults.provider", value: "openai_compatible", scope: "editable", readOnlyReason: null },
    baseUrl: { id: "provider_defaults.base_url", value: settings.providerDefaults.baseUrl, scope: "editable", readOnlyReason: null },
    apiKeyEnv: { id: "provider_defaults.api_key_env", value: settings.providerDefaults.apiKeyEnv, scope: "editable", readOnlyReason: null },
    model: { id: "provider_defaults.model", value: settings.providerDefaults.model, scope: "editable", readOnlyReason: null },
    timeoutSeconds: { id: "provider_defaults.timeout_seconds", value: settings.providerDefaults.timeoutSeconds, scope: "editable", readOnlyReason: null },
    state: {
      provider: "openai_compatible",
      model: settings.providerDefaults.model,
      configured: false,
      status: "not_configured",
      message: "no credential in the environment",
      credentialSource: "environment",
      credentialPresent: false,
    },
    secretPolicy:
      "The key is read from the process environment by the backend at run time. This page never receives it, cannot store it, and will not display it.",
  },
  defaultRunMode: { id: "default_run_mode", value: settings.defaultRunMode, scope: "editable", readOnlyReason: null },
  readOnlyFacts: [
    {
      id: "storage.results",
      label: "Results, telemetry, and experiment history",
      value: "C:/data/probelab.sqlite3",
      reason: "Fixed to the application data directory.",
    },
    {
      id: "logging.level",
      label: "Log verbosity",
      value: "RUST_LOG, read once at start-up",
      reason: "The tracing filter is built before a window exists.",
    },
  ],
  isDefault: true,
});

let stored: SettingsView = view();

beforeEach(() => {
  stored = view();
  vi.spyOn(api, "readSettings").mockImplementation(async () => stored);
  vi.spyOn(api, "applySettings").mockImplementation(async (settings: LabSettings) => {
    stored = { ...view(settings), isDefault: false };
    return stored;
  });
  vi.spyOn(api, "resetSettings").mockImplementation(async () => {
    stored = view();
    return stored;
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

const saveButton = () => screen.getByRole("button", { name: /save changes/i });
const resetButton = () => screen.getByRole("button", { name: /restore defaults/i });

describe("loading", () => {
  it("renders the values the backend returned", async () => {
    render(<SettingsPanel />);
    expect(await screen.findByLabelText("EPISODES")).toHaveValue(5);
    expect(screen.getByLabelText("BASE URL")).toHaveValue("http://127.0.0.1:11434/v1");
    expect(screen.getByLabelText("MODEL")).toHaveValue("local-model");
    expect(screen.getByLabelText("TIMEOUT (S)")).toHaveValue(30);
  });

  it("says the values are the built-in defaults", async () => {
    render(<SettingsPanel />);
    await screen.findByLabelText("EPISODES");
    // Scoped to the heading: the same words appear in the state line and in the save-area note.
    const heading = screen.getByRole("heading", { name: /settings/i }).closest(".panel-heading");
    expect(within(heading as HTMLElement).getByText(/built-in defaults/i)).toBeInTheDocument();
  });

  it("reports a read failure and offers a retry rather than an empty form", async () => {
    vi.spyOn(api, "readSettings").mockRejectedValue({ kind: "storage", detail: { reason: "the file is locked" } });
    render(<SettingsPanel />);
    expect(await screen.findByRole("alert")).toHaveTextContent("the file is locked");
    expect(screen.getByRole("button", { name: /try again/i })).toBeInTheDocument();
    // And crucially, no editable form, because a form that never loaded would overwrite what it never
    // saw the moment someone pressed save.
    expect(screen.queryByLabelText("EPISODES")).toBeNull();
  });

  it("retries on demand", async () => {
    vi.spyOn(api, "readSettings")
      .mockRejectedValueOnce({ kind: "storage", detail: { reason: "locked" } })
      .mockImplementation(async () => stored);
    render(<SettingsPanel />);
    fireEvent.click(await screen.findByRole("button", { name: /try again/i }));
    expect(await screen.findByLabelText("EPISODES")).toHaveValue(5);
  });
});

describe("editing", () => {
  it("will not offer to save when nothing has changed", async () => {
    render(<SettingsPanel />);
    await screen.findByLabelText("EPISODES");
    expect(saveButton()).toBeDisabled();
    expect(screen.getByText(/these are the built-in defaults/i)).toBeInTheDocument();
  });

  it("enables save once a value differs, and says the change is unsaved", async () => {
    render(<SettingsPanel />);
    fireEvent.change(await screen.findByLabelText("EPISODES"), { target: { value: "9" } });
    expect(saveButton()).toBeEnabled();
    expect(screen.getByText(/unsaved changes/i)).toBeInTheDocument();
  });

  it("re-disables save when a change is undone", async () => {
    render(<SettingsPanel />);
    const episodes = await screen.findByLabelText("EPISODES");
    fireEvent.change(episodes, { target: { value: "9" } });
    expect(saveButton()).toBeEnabled();
    fireEvent.change(episodes, { target: { value: "5" } });
    expect(saveButton()).toBeDisabled();
  });

  /**
   * The seed control is the one place where a stored value and an effective value differ, and showing
   * a seed next to "chosen by the planner" would imply the seed is in use.
   */
  it("disables the seed input while the planner chooses, and enables it when fixed", async () => {
    render(<SettingsPanel />);
    expect(await screen.findByLabelText("ROOT SEED")).toBeDisabled();
    fireEvent.change(screen.getByLabelText("SEED"), { target: { value: "fixed" } });
    expect(screen.getByLabelText("ROOT SEED")).toBeEnabled();
    expect(screen.getByText(/used for every new experiment/i)).toBeInTheDocument();
  });

  it("says a seed is not in use while the planner chooses it", async () => {
    render(<SettingsPanel />);
    expect(await screen.findByText(/the planner chooses each run's seed/i)).toBeInTheDocument();
  });
});

describe("saving", () => {
  it("sends the edited document to the backend", async () => {
    render(<SettingsPanel />);
    fireEvent.change(await screen.findByLabelText("EPISODES"), { target: { value: "9" } });
    fireEvent.click(saveButton());
    await waitFor(() => expect(api.applySettings).toHaveBeenCalled());
    expect(api.applySettings).toHaveBeenCalledWith(
      expect.objectContaining({
        experimentDefaults: expect.objectContaining({ episodes: 9 }),
      }),
    );
  });

  /**
   * The central property. What is displayed after a save is what the backend *stored*, so a value it
   * adjusted or refused cannot keep showing as though it took effect.
   */
  it("renders the document the backend stored, not the one that was typed", async () => {
    // The backend stores something different from what was submitted.
    vi.spyOn(api, "applySettings").mockImplementation(async () => {
      stored = { ...view({ ...defaults(), experimentDefaults: { episodes: 5, seedBehaviour: "planner_chosen", rootSeed: 0 } }), isDefault: true };
      return stored;
    });
    render(<SettingsPanel />);
    fireEvent.change(await screen.findByLabelText("EPISODES"), { target: { value: "9" } });
    fireEvent.click(saveButton());
    await waitFor(() => expect(screen.getByLabelText("EPISODES")).toHaveValue(5));
    expect(screen.queryByText(/unsaved changes/i)).toBeNull();
  });

  it("confirms the save and that it persists", async () => {
    render(<SettingsPanel />);
    fireEvent.change(await screen.findByLabelText("EPISODES"), { target: { value: "9" } });
    fireEvent.click(saveButton());
    expect(await screen.findByRole("status")).toHaveTextContent(/settings saved/i);
    expect(screen.getByRole("status")).toHaveTextContent(/future experiments/i);
  });

  it("reports the value the backend stored in its own words", async () => {
    render(<SettingsPanel />);
    fireEvent.change(await screen.findByLabelText("EPISODES"), { target: { value: "9" } });
    fireEvent.click(saveButton());
    await screen.findByRole("status");
    await waitFor(() =>
      expect(screen.getByText(/customised/i)).toBeInTheDocument(),
    );
  });

  it("disables save again once the change is stored", async () => {
    render(<SettingsPanel />);
    fireEvent.change(await screen.findByLabelText("EPISODES"), { target: { value: "9" } });
    fireEvent.click(saveButton());
    await screen.findByRole("status");
    expect(saveButton()).toBeDisabled();
  });
});

describe("validation", () => {
  /**
   * Backend validation is the real check; the panel's own `min`/`max` are a convenience. This test
   * drives an invalid value past the browser's own constraint and asserts the *backend's* error is
   * what a person sees, placed against the field that produced it.
   */
  it("shows the backend's rejection beside the field it names", async () => {
    vi.spyOn(api, "applySettings").mockRejectedValue({
      kind: "invalid",
      detail: { field: "experiment_defaults.episodes", reason: "default episodes must be between 1 and 1000, not 0" },
    });
    render(<SettingsPanel />);
    fireEvent.change(await screen.findByLabelText("EPISODES"), { target: { value: "0" } });
    fireEvent.click(saveButton());

    const message = await screen.findByText(/default episodes must be between/i);
    expect(message).toHaveAttribute("role", "alert");
    // Placed with its control, not only in a banner at the top of the page.
    const group = message.closest(".field-group") as HTMLElement;
    expect(within(group).getByLabelText("EPISODES")).toHaveAttribute("aria-invalid", "true");
  });

  it("marks only the offending field as invalid", async () => {
    vi.spyOn(api, "applySettings").mockRejectedValue({
      kind: "invalid",
      detail: { field: "provider_defaults.base_url", reason: "base_url must use HTTPS except for loopback HTTP" },
    });
    render(<SettingsPanel />);
    fireEvent.change(await screen.findByLabelText("BASE URL"), {
      target: { value: "http://example.com/v1" },
    });
    fireEvent.click(saveButton());

    await screen.findByText(/must use HTTPS/i);
    expect(screen.getByLabelText("BASE URL")).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByLabelText("EPISODES")).toHaveAttribute("aria-invalid", "false");
  });

  it("keeps the edit after a rejection, so the fix is one edit away", async () => {
    vi.spyOn(api, "applySettings").mockRejectedValue({
      kind: "invalid",
      detail: { field: "experiment_defaults.episodes", reason: "out of range" },
    });
    render(<SettingsPanel />);
    const episodes = await screen.findByLabelText("EPISODES");
    fireEvent.change(episodes, { target: { value: "0" } });
    fireEvent.click(saveButton());
    await screen.findByText(/out of range/i);
    expect(episodes).toHaveValue(0);
    expect(saveButton()).toBeEnabled();
  });

  it("shows a storage failure as one message rather than blaming a field", async () => {
    vi.spyOn(api, "applySettings").mockRejectedValue({
      kind: "storage",
      detail: { reason: "the settings file is locked" },
    });
    render(<SettingsPanel />);
    fireEvent.change(await screen.findByLabelText("EPISODES"), { target: { value: "9" } });
    fireEvent.click(saveButton());
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("the settings file is locked");
    // No field is at fault, so none is marked.
    expect(screen.getByLabelText("EPISODES")).toHaveAttribute("aria-invalid", "false");
  });
});

describe("reset", () => {
  it("restores the defaults and says what it did not touch", async () => {
    vi.spyOn(api, "readSettings").mockImplementation(async () => {
      stored = { ...view({ ...defaults(), experimentDefaults: { episodes: 9, seedBehaviour: "fixed", rootSeed: 4 } }), isDefault: false };
      return stored;
    });
    render(<SettingsPanel />);
    expect(await screen.findByLabelText("EPISODES")).toHaveValue(9);
    fireEvent.click(resetButton());

    const confirmation = await screen.findByRole("status");
    expect(confirmation).toHaveTextContent(/restored to their defaults/i);
    // The reassurance is the point: a reset that could reach results would be unsafe to offer, so the
    // panel states plainly that it did not.
    expect(confirmation).toHaveTextContent(/results, telemetry, and replay data were not touched/i);
    expect(screen.getByLabelText("EPISODES")).toHaveValue(5);
  });

  it("is unavailable when there is nothing to reset to", async () => {
    render(<SettingsPanel />);
    await screen.findByLabelText("EPISODES");
    expect(resetButton()).toBeDisabled();
  });

  it("is available once the settings have been customised", async () => {
    vi.spyOn(api, "readSettings").mockImplementation(async () => {
      stored = { ...view({ ...defaults(), experimentDefaults: { episodes: 9, seedBehaviour: "planner_chosen", rootSeed: 0 } }), isDefault: false };
      return stored;
    });
    render(<SettingsPanel />);
    await screen.findByLabelText("EPISODES");
    expect(resetButton()).toBeEnabled();
  });

  it("restores defaults even when there are unsaved edits, because both are discarded", async () => {
    render(<SettingsPanel />);
    fireEvent.change(await screen.findByLabelText("EPISODES"), { target: { value: "9" } });
    fireEvent.change(screen.getByLabelText("MODEL"), { target: { value: "edited" } });
    fireEvent.click(resetButton());
    await screen.findByRole("status");
    expect(api.applySettings).not.toHaveBeenCalled();
    expect(screen.getByLabelText("MODEL")).toHaveValue("local-model");
  });

  it("reports a reset that failed and does not pretend it worked", async () => {
    vi.spyOn(api, "resetSettings").mockRejectedValue({
      kind: "storage",
      detail: { reason: "the settings file is locked" },
    });
    render(<SettingsPanel />);
    fireEvent.change(await screen.findByLabelText("EPISODES"), { target: { value: "9" } });
    fireEvent.click(resetButton());
    expect(await screen.findByRole("alert")).toHaveTextContent("the settings file is locked");
    expect(screen.getByLabelText("EPISODES")).toHaveValue(9);
  });
});

describe("read-only facts", () => {
  it("shows the real storage path with the reason it cannot be changed", async () => {
    render(<SettingsPanel />);
    expect(await screen.findByText("C:/data/probelab.sqlite3")).toBeInTheDocument();
    expect(screen.getByText(/fixed to the application data directory/i)).toBeInTheDocument();
  });

  it("shows log configuration as read-only rather than offering a control", async () => {
    render(<SettingsPanel />);
    expect(await screen.findByText("RUST_LOG, read once at start-up")).toBeInTheDocument();
    // No input exists for it: an environment variable read at start-up cannot be changed by a field
    // in a page that loads after start-up.
    expect(screen.queryByLabelText(/log/i)).toBeNull();
  });

  /**
   * Read-only means no control. Asserted by the absence of any input in that section, because a
   * disabled-looking control that a test cannot tell from an editable one is not a guarantee.
   */
  it("offers no editable control in the read-only section", async () => {
    render(<SettingsPanel />);
    const section = (await screen.findByRole("region", { name: /storage and fixed/i })) as HTMLElement;
    expect(within(section).queryAllByRole("textbox")).toHaveLength(0);
    expect(within(section).queryAllByRole("spinbutton")).toHaveLength(0);
    expect(within(section).queryAllByRole("button")).toHaveLength(0);
  });
});

describe("secret safety", () => {
  /**
   * The strongest available assertion: there is no field into which a credential could be typed. A
   * test that checked the panel "does not display secrets" would still pass if someone added a
   * password input; this one would not.
   */
  it("has no control that could accept a credential", async () => {
    const { container } = render(<SettingsPanel />);
    await screen.findByLabelText("API KEY ENV VAR (NAME ONLY)");
    expect(container.querySelector('input[type="password"]')).toBeNull();
    // Every label that mentions a credential must belong to the one field that records a variable
    // *name*. Anything else would be a place a secret could be typed.
    const suspicious = [
      /api key/i,
      /secret/i,
      /token/i,
      /password/i,
      /credential/i,
    ];
    for (const pattern of suspicious) {
      for (const match of screen.queryAllByLabelText(pattern)) {
        expect(match).toHaveAttribute("id", "setting-key-env");
      }
    }
  });

  it("states that the key is never received or displayed", async () => {
    render(<SettingsPanel />);
    expect(await screen.findByText(/never receives it, cannot store it, and will not display it/i)).toBeInTheDocument();
  });

  it("reports credential presence as a fact, without a value", async () => {
    vi.spyOn(api, "readSettings").mockImplementation(async () => {
      const withCredential = view();
      withCredential.providerDefaults.state = {
        ...withCredential.providerDefaults.state,
        configured: true,
        status: "connected",
        message: "Credential found in the environment.",
        credentialPresent: true,
      };
      return withCredential;
    });
    render(<SettingsPanel />);
    expect(await screen.findByText(/a credential is currently present/i)).toBeInTheDocument();
    expect(screen.getByText("CONFIGURED")).toBeInTheDocument();
  });

  it("says which variable to export when no credential is present", async () => {
    render(<SettingsPanel />);
    expect(
      await screen.findByText(/no credential was found in OPENAI_COMPATIBLE_API_KEY/i),
    ).toBeInTheDocument();
  });
});

describe("provider state", () => {
  it("does not claim a provider is ready when it is not configured", async () => {
    render(<SettingsPanel />);
    expect(await screen.findByText("NOT CONFIGURED")).toBeInTheDocument();
    expect(screen.queryByText("CONFIGURED")).toBeNull();
    expect(screen.getByText(/no credential in the environment/i)).toBeInTheDocument();
  });

  it("reports a configured provider with the reason it is usable", async () => {
    vi.spyOn(api, "readSettings").mockImplementation(async () => {
      const ready = view();
      ready.providerDefaults.state = {
        ...ready.providerDefaults.state,
        configured: true,
        status: "connected",
        message: "Credential found in the environment. A connected run is ready to start.",
        credentialPresent: true,
      };
      return ready;
    });
    render(<SettingsPanel />);
    expect(await screen.findByText("CONFIGURED")).toBeInTheDocument();
    expect(screen.getByText(/a connected run is ready to start/i)).toBeInTheDocument();
  });
});

describe("persistence across a reload", () => {
  /**
   * The whole point of the milestone. The previous defaults lived in React state and were lost on
   * every launch, so a remount reading the stored values is the property that makes this real.
   */
  it("shows the stored values after a remount", async () => {
    const { unmount } = render(<SettingsPanel />);
    fireEvent.change(await screen.findByLabelText("EPISODES"), { target: { value: "9" } });
    fireEvent.change(screen.getByLabelText("MODEL"), { target: { value: "persisted-model" } });
    fireEvent.click(saveButton());
    await screen.findByRole("status");
    unmount();

    render(<SettingsPanel />);
    expect(await screen.findByLabelText("EPISODES")).toHaveValue(9);
    expect(screen.getByLabelText("MODEL")).toHaveValue("persisted-model");
  });
});
