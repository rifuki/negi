// What travels over the one WebSocket to the Mac. The connection itself, and its
// retries, belong to react-use-websocket in components/Link.tsx; this file turns messages
// into store updates, owns the live screens, and decides which panes the server sends.

import { copied, typed } from "@/lib/clipboard";
import { useApp } from "@/lib/store";
import { type RowsMessage, Screen, type ScreenMessage } from "@/tmux/screen";
import type { Session } from "@/tmux/tree";

interface Ack {
  pane?: string;
  lines?: string[];
}

type Waiter = { resolve: (value: Ack) => void; reject: (error: Error) => void };

let sendNow: ((message: object) => void) | null = null;
let nextId = 1;
let failures = 0;
const waiting = new Map<number, Waiter>();

/** Pane id to whether it is the open pane (true) or only a preview (false). */
let watched = new Map<string, boolean>();
let wanted = new Map<string, boolean>();

/** The live screens, kept outside React: they are painted row by row. */
const screens = new Map<string, Screen>();
const listeners = new Map<string, Set<(whole: boolean) => void>>();

export function screenOf(pane: string): Screen | undefined {
  return screens.get(pane);
}

/** Calls `listener` whenever the pane's screen changes; `whole` is true for a full screen. */
export function onScreen(pane: string, listener: (whole: boolean) => void): () => void {
  let set = listeners.get(pane);
  if (!set) listeners.set(pane, (set = new Set()));
  set.add(listener);
  return () => {
    set.delete(listener);
    if (!set.size) listeners.delete(pane);
  };
}

/** Sends a request the server acknowledges. Rejects with the server's reason. */
export function request(message: object): Promise<Ack> {
  return new Promise((resolve, reject) => {
    if (!sendNow) return reject(new Error("Not connected to your Mac"));
    const id = nextId++;
    const timeout = setTimeout(() => {
      waiting.delete(id);
      reject(new Error("Your Mac did not answer"));
    }, 8000);
    waiting.set(id, {
      resolve: (value) => (clearTimeout(timeout), resolve(value)),
      reject: (error) => (clearTimeout(timeout), reject(error)),
    });
    const t = (message as { t?: string }).t;
    if (t === "keys" || t === "text") typed();
    sendNow({ ...message, id });
  });
}

/** Declares which panes the page shows: previews at home, one open pane. */
export function watch(panes: Map<string, boolean>) {
  wanted = panes;
  sync();
}

function sync() {
  if (!sendNow || useApp.getState().phase !== "ready") return;
  for (const [pane, open] of wanted) {
    if (watched.get(pane) !== open) sendNow({ t: "watch", pane, open });
  }
  for (const pane of watched.keys()) {
    if (!wanted.has(pane)) {
      sendNow({ t: "unwatch", pane });
      screens.delete(pane);
    }
  }
  watched = new Map(wanted);
}

export function signOut(notice = "") {
  screens.clear();
  for (const timer of starting.values()) clearTimeout(timer);
  starting.clear();
  useApp.setState({ token: null, phase: "connecting", problem: "", notice, hasTree: false, sessions: [], busy: [], sheet: null });
}

export function signIn(token: string) {
  failures = 0;
  useApp.setState({ token, phase: "connecting", problem: "", notice: "" });
}

/** The build this page is, as the server wrote it into the page. */
const loaded = document.querySelector<HTMLMetaElement>('meta[name="negi-build"]')?.content ?? "";

/** Reloads the page when the server carries another build: this page is the old one. */
export function outdated(build?: string) {
  if (!build || !loaded || build === loaded) return false;
  location.reload();
  return true;
}

// Working from the server's first report, except a redraw this page caused by resizing.
const starting = new Map<string, number>();

// Until when a pane's output is known to be a redraw this page caused, by resizing its window.
const hushed = new Map<string, number>();

/** Call when this page resizes a pane's window: what the pane prints in answer is not work. */
export function hush(pane: string) {
  hushed.set(pane, Date.now() + 4000);
  clearTimeout(starting.get(pane));
  starting.delete(pane);
  const { busy } = useApp.getState();
  if (busy.includes(pane)) useApp.setState({ busy: busy.filter((p) => p !== pane) });
}

function printing(pane: string) {
  if (starting.has(pane) || useApp.getState().busy.includes(pane)) return;
  const wait = (hushed.get(pane) ?? 0) - Date.now();
  if (wait <= 0) {
    useApp.setState({ busy: [...useApp.getState().busy, pane] });
    return;
  }
  starting.set(
    pane,
    window.setTimeout(() => {
      starting.delete(pane);
      const { busy } = useApp.getState();
      if (!busy.includes(pane)) useApp.setState({ busy: [...busy, pane] });
    }, wait),
  );
}

export type Incoming =
  | { t: "ready"; device: string; version: string; os?: string; build?: string }
  | { t: "bye"; reason: string }
  | { t: "tree"; server: boolean; sessions: Session[]; busy: string[] }
  | ({ t: "screen"; pane: string } & ScreenMessage)
  | ({ t: "rows"; pane: string } & RowsMessage)
  | { t: "busy"; pane: string; on: boolean }
  | { t: "gone"; pane: string }
  | { t: "copied"; text: string }
  | { t: "ack"; id: number; ok: boolean; error?: string; pane?: string; lines?: string[] };

/** Called for every message the server sends. */
export function receive(msg: Incoming) {
  const state = useApp.getState();
  switch (msg.t) {
    case "ready":
      // The server was replaced by one carrying another interface: this page is the old one.
      if (outdated(msg.build)) return;
      failures = 0;
      useApp.setState((s) => ({ phase: "ready", greeted: s.greeted + 1, problem: "", device: msg.device, version: msg.version, os: msg.os ?? "" }));
      sync();
      break;
    case "bye":
      if (msg.reason === "unpaired") signOut("This device is no longer paired.");
      break;
    case "tree":
      // The tree carries every pane the server counts as working, agents working silently
      // included. Read the list back after `printing`, which has just added to it.
      for (const pane of msg.busy) printing(pane);
      for (const [pane, timer] of starting) {
        if (msg.busy.includes(pane)) continue;
        clearTimeout(timer);
        starting.delete(pane);
      }
      useApp.setState({ server: msg.server, sessions: msg.sessions, busy: useApp.getState().busy.filter((p) => msg.busy.includes(p)), hasTree: true });
      break;
    case "screen": {
      let screen = screens.get(msg.pane);
      if (!screen) screens.set(msg.pane, (screen = new Screen()));
      screen.set(msg);
      for (const listener of listeners.get(msg.pane) ?? []) listener(true);
      break;
    }
    case "rows":
      screens.get(msg.pane)?.patch(msg);
      for (const listener of listeners.get(msg.pane) ?? []) listener(false);
      break;
    case "busy":
      if (msg.on) {
        printing(msg.pane);
      } else if (starting.has(msg.pane)) {
        // It went quiet while a redraw of ours was held back: it was the redraw, not work.
        clearTimeout(starting.get(msg.pane));
        starting.delete(msg.pane);
      } else if (state.busy.includes(msg.pane)) {
        // The moment this tool exists for: something that was printing has stopped.
        useApp.setState({ busy: state.busy.filter((p) => p !== msg.pane), quiets: state.quiets + 1 });
      }
      break;
    case "gone":
      screens.delete(msg.pane);
      break;
    case "copied":
      void copied(msg.text);
      break;
    case "ack": {
      const waiter = waiting.get(msg.id);
      waiting.delete(msg.id);
      if (!waiter) break;
      if (msg.ok) waiter.resolve({ pane: msg.pane, lines: msg.lines });
      else waiter.reject(new Error(msg.error || "tmux refused that"));
      break;
    }
  }
}

/** Called when the socket opens or closes: `send` is the way out, or null when there is none. */
export function setSender(send: ((message: object) => void) | null) {
  sendNow = send;
}

/** Called when the socket closes: everything in flight has failed. */
export function dropped() {
  watched.clear();
  for (const { reject } of waiting.values()) reject(new Error("The connection dropped"));
  waiting.clear();
  const state = useApp.getState();
  if (!state.token || document.hidden) return;
  failures += 1;
  useApp.setState({
    phase: state.phase === "connecting" ? "connecting" : "lost",
    problem: failures >= 3 ? "Your Mac is not answering. Is negi running there?" : state.problem,
  });
}
