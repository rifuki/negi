// tmux's prefix on a computer keyboard: Ctrl+B, then a key that acts on panes and windows the
// way tmux's own default bindings do. Ctrl+B twice sends Ctrl+B to the program.

import { toast } from "sonner";
import { create } from "zustand";
import { request } from "@/connection/link";
import { useApp } from "@/lib/store";
import { activePane, findPane } from "@/tmux/tree";

type Armed = null | "keys" | "kill";

/** `carry`: the next terminal to open takes the keyboard focus. */
export const usePrefix = create<{ armed: Armed; carry: boolean }>(() => ({ armed: null, carry: false }));

const MOVES: Record<string, string> = {
  h: "L", ArrowLeft: "L", j: "D", ArrowDown: "D", k: "U", ArrowUp: "U", l: "R", ArrowRight: "R",
};

/** What the prefix offers, for the help that `?` shows. */
export const PREFIX_HELP = [
  ["← ↓ ↑ → or h j k l", "Move to the pane that way"],
  ["%  /  \"", "Split side by side / one above the other"],
  ["c", "New window"],
  ["n  /  p", "Next / previous window"],
  ["0 – 9", "Window by number"],
  ["x", "Close the pane (y to confirm)"],
  ["s", "Show or hide the sessions"],
  ["Ctrl+B", "Send Ctrl+B to the program"],
] as const;

const fail = (e: Error) => toast.error(e.message);

/**
 * Handles a key from the terminal. Answers true when the key was the prefix's, so the caller
 * does not send it on. `go` opens a pane.
 */
export function prefixed(e: { key: string; ctrlKey: boolean; altKey: boolean; metaKey: boolean; shiftKey: boolean }, pane: string, go: (pane: string) => void): boolean {
  if (!useApp.getState().prefixKeys) return false;
  const armed = usePrefix.getState().armed;
  const isPrefix = e.ctrlKey && !e.altKey && !e.metaKey && e.key.toLowerCase() === "b";
  if (!armed) {
    if (!isPrefix) return false;
    usePrefix.setState({ armed: "keys" });
    return true;
  }
  // Shift on its way to % or ", and the other modifiers alone, are not the next key.
  if (["Shift", "Control", "Alt", "Meta", "CapsLock"].includes(e.key)) return true;
  usePrefix.setState({ armed: null });

  if (armed === "kill") {
    if (e.key === "y") void request({ t: "kill-pane", pane }).catch(fail);
    return true;
  }
  if (isPrefix) {
    void request({ t: "keys", pane, keys: ["C-b"] }).catch(fail);
    return true;
  }
  const where = findPane(useApp.getState().sessions, pane);
  const windows = where ? [...where.session.windows].sort((a, b) => a.index - b.index) : [];
  const at = where ? windows.findIndex((w) => w.id === where.window.id) : -1;
  const toWindow = (i: number) => {
    const w = windows[(i + windows.length) % windows.length];
    if (w) go(activePane(w).id);
  };
  const opened = (ack: { pane?: string }) => ack.pane && go(ack.pane);

  if (MOVES[e.key]) {
    void request({ t: "select-pane", pane, dir: MOVES[e.key] }).then(opened, fail);
  } else if (e.key === "%" || e.key === '"') {
    void request({ t: "split", pane, across: e.key === "%" }).then(opened, fail);
  } else if (e.key === "c" && where) {
    void request({ t: "new-window", session: where.session.id }).then(opened, fail);
  } else if (e.key === "n" && at >= 0) {
    toWindow(at + 1);
  } else if (e.key === "p" && at >= 0) {
    toWindow(at - 1);
  } else if (/^[0-9]$/.test(e.key)) {
    const w = windows.find((w) => w.index === Number(e.key));
    if (w) go(activePane(w).id);
    else toast(`No window ${e.key}`);
  } else if (e.key === "x") {
    usePrefix.setState({ armed: "kill" });
    toast("Close this pane?", { description: "Press y to close it, any other key to keep it" });
  } else if (e.key === "s") {
    useApp.getState().toggleSidebar();
  } else if (e.key === "?") {
    toast("Ctrl+B, then", {
      duration: 10000,
      description: (
        <dl className="mt-1 grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-xs">
          {PREFIX_HELP.map(([keys, what]) => (
            <div key={keys} className="contents">
              <dt className="font-mono font-bold whitespace-nowrap">{keys}</dt>
              <dd>{what}</dd>
            </div>
          ))}
        </dl>
      ),
    });
  }
  return true;
}
