import { create } from "zustand";
import { createJSONStorage, persist, type StateStorage } from "zustand/middleware";
import type { Session } from "@/tmux/tree";

/** Which sheet is open, if any. */
export type SheetState =
  | { kind: "new-session" }
  | { kind: "session"; session: string }
  | { kind: "pane"; pane: string }
  | { kind: "device" }
  | { kind: "mac" }
  | { kind: "working" }
  | null;

interface AppState {
  token: string | null;
  /** The terminal's text size as a share of the size that fits the pane: 1 is 100%. */
  zoom: number;
  theme: "system" | "light" | "dark";
  /**
   * How an open pane is shown: "screen" as it is on the host, "fit" with its window sized
   * for this device.
   */
  view: "screen" | "fit";
  /** The pane last opened in each session, by session id. */
  last: Record<string, string>;
  /** Whether the character and the drawings move. Off is the pause for everything that loops. */
  motion: boolean;
  /** Which colour set is in use. */
  accent: "miku" | "aqua" | "sky" | "sakura";
  /** Which drawing looks over the summary card; an id from `lib/characters`. */
  character: string;
  /** On a wide screen, whether the session list stays beside an open pane. */
  sidebar: boolean;
  /** Whether Ctrl+B, as in tmux, starts a negi command instead of reaching the program. */
  prefixKeys: boolean;

  /** connecting: no answer yet. ready: talking. lost: the socket dropped and is retried. */
  phase: "connecting" | "ready" | "lost";
  /** Counts the server's greetings: it changes with every new socket, even one opened while
   * the tab was hidden, when `phase` never left "ready". */
  greeted: number;
  /** Why the Mac cannot be reached, once enough retries have failed. */
  problem: string;
  /** Shown on the pairing screen, such as "This device is no longer paired." */
  notice: string;
  device: string;
  version: string;
  /** The host's system as Rust names it ("macos", "linux"); decides how keys are labelled. */
  os: string;
  /** False when tmux is not running on the Mac. */
  server: boolean;
  sessions: Session[];
  hasTree: boolean;
  /** Ids of the panes that are printing right now. */
  busy: string[];
  /** Goes up each time a busy pane goes quiet; the character hops on it. */
  quiets: number;
  sheet: SheetState;
  /** History lines loaded for the open pane, oldest first. */
  /** A pane's earlier output. `quiet` when it was fetched unasked, as the pane opened. */
  history: { pane: string; lines: string[]; quiet?: boolean } | null;

  openSheet: (sheet: SheetState) => void;
  setZoom: (zoom: number) => void;
  setTheme: (theme: AppState["theme"]) => void;
  setMotion: (motion: boolean) => void;
  remember: (session: string, pane: string) => void;
  setView: (view: AppState["view"]) => void;
  setAccent: (accent: AppState["accent"]) => void;
  setCharacter: (character: string) => void;
  toggleSidebar: () => void;
  setPrefixKeys: (on: boolean) => void;
}

// Storage can be unavailable (private mode); the app then asks to pair on each visit.
const safeStorage: StateStorage = {
  getItem: (name) => {
    try {
      return localStorage.getItem(name);
    } catch {
      return null;
    }
  },
  setItem: (name, value) => {
    try {
      localStorage.setItem(name, value);
    } catch {
      /* nothing to do */
    }
  },
  removeItem: (name) => {
    try {
      localStorage.removeItem(name);
    } catch {
      /* nothing to do */
    }
  },
};

export const useApp = create<AppState>()(
  persist(
    (set) => ({
      token: null,
      zoom: 1,
      theme: "system",
      motion: true,
      last: {},
      view: "screen",
      accent: "miku",
      character: "miku",
      sidebar: true,
      prefixKeys: true,
      phase: "connecting",
      greeted: 0,
      problem: "",
      notice: "",
      device: "",
      version: "",
      os: "",
      server: true,
      sessions: [],
      hasTree: false,
      busy: [],
      quiets: 0,
      sheet: null,
      history: null,
      openSheet: (sheet) => set({ sheet }),
      setZoom: (zoom) => set({ zoom: Math.round(Math.min(3, Math.max(0.5, zoom)) * 10) / 10 }),
      setTheme: (theme) => set({ theme }),
      setMotion: (motion) => set({ motion }),
      remember: (session, pane) => set((s) => (s.last[session] === pane ? s : { last: { ...s.last, [session]: pane } })),
      setView: (view) => set({ view }),
      setAccent: (accent) => set({ accent }),
      setCharacter: (character) => set({ character }),
      toggleSidebar: () => set((s) => ({ sidebar: !s.sidebar })),
      setPrefixKeys: (prefixKeys) => set({ prefixKeys }),
    }),
    {
      name: "negi",
      storage: createJSONStorage(() => safeStorage),
      // Only what must survive a reload. Everything else comes from the Mac.
      // A view that no longer exists (the old reader) is shown as it is.
      merge: (kept, current) => {
        const state = { ...current, ...(kept as Partial<AppState>) };
        if (state.view !== "screen" && state.view !== "fit") state.view = "screen";
        return state;
      },
      partialize: (state) => ({ token: state.token, zoom: state.zoom, theme: state.theme, motion: state.motion, last: state.last, view: state.view, accent: state.accent, character: state.character, sidebar: state.sidebar, prefixKeys: state.prefixKeys }),
    },
  ),
);
