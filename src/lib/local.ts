import { api } from "./api";

/**
 * The app's saved state: query history, open query tabs, saved queries,
 * filters and layout. It lives encrypted on disk (see src-tauri/src/vault.rs),
 * never in the webview's storage, so other programs can't read it from
 * Tabla's files. Loaded once before the app renders; writes are batched.
 */
let state: Record<string, unknown> = {};
let writable = false;
let timer: ReturnType<typeof setTimeout> | undefined;

/** Why saved state couldn't be loaded (e.g. a locked keychain), if it couldn't. */
export let localProblem: string | null = null;

async function flush() {
  if (!writable) return;
  clearTimeout(timer);
  timer = undefined;
  try {
    await api.saveAppState(state);
  } catch (e) {
    console.error("[tabla] saving app state failed", e);
  }
}

export async function hydrateLocal() {
  try {
    state = await api.loadAppState();
    writable = true;
  } catch (e) {
    // Nothing is written this session, so the saved state can't be overwritten.
    localProblem = e instanceof Error ? e.message : String(e);
    state = {};
  }
  // Older versions kept this in localStorage, in plain text: move it over, then erase it.
  try {
    const legacy = Object.keys(localStorage).filter((k) => k.startsWith("tabla."));
    if (legacy.length && writable) {
      for (const k of legacy) {
        if (!(k in state)) {
          try {
            state[k] = JSON.parse(localStorage.getItem(k) ?? "null");
          } catch {
            /* skip unreadable entries */
          }
        }
      }
      await api.saveAppState(state);
      for (const k of legacy) localStorage.removeItem(k);
    }
  } catch {
    /* storage unavailable */
  }
  window.addEventListener("pagehide", () => void flush());
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "hidden") void flush();
  });
}

export const local = {
  get<T>(key: string, fallback: T): T {
    return key in state ? (state[key] as T) : fallback;
  },
  set(key: string, value: unknown) {
    state[key] = value;
    if (!writable) return;
    clearTimeout(timer);
    timer = setTimeout(() => void flush(), 300);
  },
};
