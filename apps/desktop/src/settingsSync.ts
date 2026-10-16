/**
 * Notifies the rest of the application that the stored settings changed.
 *
 * # Why this exists
 *
 * The settings were read exactly once, in an effect with no dependencies, and every execution default —
 * the episode count, the seed behaviour, the provider defaults — was copied into `App` state at that
 * moment. `SettingsPanel` could save a new document and the next run would still use the values the
 * application happened to have started with.
 *
 * That is audit finding B11, and it is worse than a stale display: the operator changes a setting, the
 * panel confirms "They will be used for future experiments and runs", and the future experiment uses
 * something else. A confirmation that is not true is worse than no confirmation.
 *
 * # Why a module and not a prop
 *
 * The panel and the execution state live in different parts of the tree — the panel under Settings, the
 * state in `App` — and routing a callback between them means threading it through `AppShell` and every
 * intermediate component. There is one writer and one reader, they are both top-level, and the
 * alternative is a prop chain for a single monotonic value.
 *
 * Deliberately a counter rather than the settings document itself: `App` already knows how to read
 * settings, and re-reading from the backend means the values it applies are the values the backend
 * actually stored — including any the backend adjusted or refused on the way in.
 */

type Listener = () => void;

const listeners = new Set<Listener>();

/**
 * Announces that the stored settings have changed.
 *
 * Called only after a successful write. Announcing on failure would re-read the unchanged document and
 * make a rejected save look like it had taken effect.
 */
export function notifySettingsChanged(): void {
  for (const listener of [...listeners]) {
    listener();
  }
}

/**
 * Subscribes to settings changes, returning the unsubscribe function.
 *
 * The listener is invoked asynchronously on subscribe, so a subscriber that registers during the first
 * render still gets the initial document without needing a separate initial read.
 */
export function subscribeToSettingsChanges(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** How many subscribers there are. Exposed for tests that assert cleanup rather than behaviour. */
export function settingsSubscriberCount(): number {
  return listeners.size;
}