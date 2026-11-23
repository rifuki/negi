// The open pane: its live screen, the history above it, and a hardware keyboard.

import { useNavigate } from "@tanstack/react-router";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ArrowDownIcon, FileArrowUpIcon } from "@phosphor-icons/react";
import { type KeyboardEvent, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { hush, onScreen, request, screenOf } from "@/connection/link";
import { calm } from "@/lib/anim";
import { Pose } from "@/components/State";
import { useApp } from "@/lib/store";
import { findPane, paneParam } from "@/tmux/tree";
import { cn } from "@/lib/utils";
import { columnWidth, paintRow } from "@/tmux/screen";
import { useEarlier } from "./earlier";
import { useSendFiles } from "./files";
import { take, useKeyboard, useMods } from "./mods";
import { prefixed, usePrefix } from "./prefix";

const DESK_KEYS: Record<string, string> = {
  Enter: "Enter", Escape: "Escape", Tab: "Tab", Backspace: "BSpace", Delete: "DC",
  ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right",
  Home: "Home", End: "End", PageUp: "PPage", PageDown: "NPage", " ": "Space",
};

/** The text size, in px, at which a window sized for the phone is shown. */
const FIT_PX = 13;

/** What the unseen field always holds, so that a Backspace on an empty line is still a change. */
const HELD = "\u200b";

const send = (message: object) => request(message).catch((e: Error) => toast.error(e.message));

export function Terminal({ pane }: { pane: string }) {
  const stage = useRef<HTMLDivElement>(null);
  const host = useRef<HTMLDivElement>(null);
  const [ready, setReady] = useState(false);
  const [size, setSize] = useState(12);
  const [settling, setSettling] = useState(false);
  const zoom = useApp((s) => s.zoom);
  const { lines: history, quiet, answered, reload } = useEarlier(pane);
  // A session no computer shows has no size of its own worth keeping, so it fills this screen.
  const fitted = useApp((s) => s.view === "fit" || findPane(s.sessions, pane)?.session.attached === false);
  // Fitted, the text is sized so whole rows fill the box's height, with no strip left under the last.
  const [fitPx, setFitPx] = useState<number | null>(null);

  /** Sizes the text so the pane's columns fill the width, then applies the chosen zoom.
   * The fitted size never goes under 9px; a pane wider than that scrolls sideways. */
  const fit = useCallback(() => {
    const screen = screenOf(pane);
    const el = stage.current;
    if (!screen || !el) return;
    const width = (el.clientWidth - 24) / (screen.cols * columnWidth());
    // A full-screen program has no history to scroll to, and its wheel is its own: the whole
    // screen fits the box, so the box never scrolls under it.
    const height = screen.alt ? (el.clientHeight - 24) / (screen.rows * 1.25) : Infinity;
    // Sized for the phone, the window has as many columns as fit at this size, so the size is fixed.
    // As is, every pane reads at that same size, smaller only when it would not fit the width
    // (never under 9px: a wider one scrolls sideways); a narrower one sits in the middle.
    setSize(fitted ? (fitPx ?? FIT_PX * zoom) : Math.max(9, Math.min(FIT_PX, width, height)) * zoom);
  }, [pane, zoom, fitted, fitPx]);

  // "Fit": the window in tmux is given as many columns and rows as this box holds, so the
  // programs in it lay themselves out for it. It is given back on leaving the pane or the mode.
  useEffect(() => {
    const el = stage.current;
    if (!fitted || !el) return;
    let asked = "";
    let timer = 0;
    const ask = () => {
      const rows = Math.floor((el.clientHeight - 24) / (FIT_PX * zoom * 1.25));
      // The size that makes those rows fill the height exactly: at most 1/rows over the chosen one.
      const px = (el.clientHeight - 24) / (Math.max(rows, 1) * 1.25);
      // One column fewer than fits: a rounding of a fraction of a pixel must not cost a scroll bar.
      const cols = Math.floor((el.clientWidth - 24) / (px * columnWidth())) - 1;
      if (cols < 20 || rows < 5) return;
      setFitPx(px);
      if (asked === `${cols}x${rows}`) return;
      asked = `${cols}x${rows}`;
      // Until the window has its new size the screen is the old, wider one; it is kept out
      // of sight for that moment, so the reflow is not watched. Asked again at the size it
      // already has (after a reconnect), there is no reflow, and nothing is hidden.
      const now = screenOf(pane);
      const same = now?.cols === cols && now.rows === rows;
      if (!same) setSettling(true);
      hush(pane);
      const settled = () => setSettling(false);
      // tmux rewraps what scrolled off to the new width; what was fetched before is too wide.
      request({ t: "fit", pane, cols, rows }).then(
        () => !same && setTimeout(() => (reload(), settled()), 300),
        (e: Error) => (settled(), toast.error(e.message)),
      );
    };
    // A keyboard coming up or the phone turning changes the box many times in a row.
    const observer = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = window.setTimeout(ask, 250);
    });
    observer.observe(el);
    ask();
    // A socket that dropped (Chrome throttles a tab out of sight) took its fit with it; the
    // server gave the window back, so every new socket asks again even at the same size.
    const off = useApp.subscribe((now, before) => {
      if (now.greeted !== before.greeted) {
        asked = "";
        ask();
      }
    });
    return () => {
      off();
      observer.disconnect();
      clearTimeout(timer);
      hush(pane);
      request({ t: "unfit", pane }).then(() => setTimeout(reload, 300), () => {});
    };
  }, [fitted, pane, zoom, reload]);

  // The view follows the output while it is at the bottom, and stops when scrolled away or touched.
  const pinned = useRef(true);
  const touching = useRef(false);
  // Far from the end, a button offers the way back; swiped down, it stays away until the end is reached again.
  const [away, setAway] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const toEnd = useCallback(() => {
    const el = stage.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    pinned.current = true;
    setAway(false);
  }, []);

  // The screen is painted outside React; this mounts its element and keeps it in view.
  useEffect(() => {
    const attach = (whole: boolean) => {
      const screen = screenOf(pane);
      const el = stage.current;
      if (!screen || !host.current || !el) return;
      const stick = whole || pinned.current;
      if (screen.el.parentNode !== host.current) {
        host.current.replaceChildren(screen.el);
        setReady(true);
      }
      if (whole) fit();
      if (whole || (stick && !touching.current)) requestAnimationFrame(toEnd);
    };
    attach(true);
    return onScreen(pane, attach);
  }, [pane, fit, toEnd]);

  useEffect(() => {
    const el = stage.current;
    if (!el) return;
    const observer = new ResizeObserver(fit);
    observer.observe(el);
    return () => observer.disconnect();
  }, [fit]);

  useLayoutEffect(fit, [fit]);

  // Earlier lines, a new text size or a resize move the last row without a scroll; while
  // following, the view is put back at the bottom.
  const content = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = content.current;
    if (!el) return;
    let height = el.offsetHeight;
    const observer = new ResizeObserver(() => {
      const grown = el.offsetHeight - height;
      height = el.offsetHeight;
      if (touching.current) return;
      if (pinned.current) toEnd();
      // Read from further up, what changed is the earlier output above: move by as much, and what is being read stays put.
      else if (stage.current) stage.current.scrollTop += grown;
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, [toEnd]);

  const rowHeight = size * 1.25;
  const rows = useVirtualizer({
    count: history?.length ?? 0,
    getScrollElement: () => stage.current,
    estimateSize: () => rowHeight,
    overscan: 30,
  });
  useEffect(() => rows.measure(), [rowHeight, rows]);

  // Asked for from the sheet, earlier output is landed on, just above the live screen.
  // Fetched unasked, it is simply there above, and the view is left where it is.
  useEffect(() => {
    if (!history || quiet) return;
    if (!history.length) {
      toast("This pane has no history. A full-screen program keeps none.");
      return;
    }
    const el = stage.current;
    if (!el) return;
    pinned.current = false;
    el.scrollTop = Math.max(0, history.length * rowHeight - el.clientHeight / 2);
    // Only when a new history arrives, not when the text size changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [history]);

  /** A hardware keyboard, with the terminal focused: keys go straight to the pane. */
  const navigate = useNavigate();
  // A pane opened from the keyboard keeps the keyboard: the new terminal takes the focus.
  useEffect(() => {
    if (!usePrefix.getState().carry) return;
    usePrefix.setState({ carry: false });
    stage.current?.focus({ preventScroll: true });
  }, []);
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.metaKey || e.nativeEvent.isComposing) return;
    const go = (id: string) => {
      if (id === pane) return;
      usePrefix.setState({ carry: true });
      void navigate({ to: "/pane/$pane", params: { pane: paneParam(id) } });
    };
    if (prefixed(e, pane, go)) {
      e.preventDefault();
      return;
    }
    let key: string | null = DESK_KEYS[e.key] ?? (/^F\d{1,2}$/.test(e.key) ? e.key : null);
    if (key === "Tab" && e.shiftKey) key = "BTab";
    const printable = e.key.length === 1 && e.key !== " ";
    if (!key && !printable) return;
    e.preventDefault();
    const mods = (e.ctrlKey ? "C-" : "") + (e.altKey ? "M-" : "");
    if (key) {
      const shift = e.shiftKey && key !== "BTab" && key.length > 1 ? "S-" : "";
      void send({ t: "keys", pane, keys: [mods + shift + key] });
    } else if (mods && /^[!-~]$/.test(e.key)) {
      void send({ t: "keys", pane, keys: [mods + e.key.toLowerCase()] });
    } else {
      void send({ t: "text", pane, text: e.key, enter: false });
    }
    toEnd();
  };

  // An unseen field brings up the phone's keyboard and passes typing to the pane. It holds
  // one invisible character so a Backspace on nothing is still heard.
  const swipe = useRef<number | null>(null);
  // "Latest" scrolls down to the end. A coasting fling would fight it; it stops only when the
  // box is, for one frame, not a scroller.
  const jumping = useRef(false);
  const jump = () => {
    const el = stage.current;
    if (!el) return;
    if (calm()) return toEnd();
    jumping.current = true;
    setAway(false);
    el.style.overflowY = "hidden";
    requestAnimationFrame(() => {
      el.style.overflowY = "";
      el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
    });
    // If the end is never reached, say because the reader took over, the button may come back.
    setTimeout(() => (jumping.current = false), 2500);
  };
  const drag = useRef({ y: 0, spare: 0 });
  // A trackpad sends dozens of small wheel events a second, and keeps sending as it coasts:
  // their distance is added up and goes out a whole row at a time, at most once a frame.
  const spin = useRef({ spare: 0, lines: 0, frame: 0 });
  const roll = (e: WheelEvent) => {
    if (!screenOf(pane)?.alt) return;
    // The wheel is the program's: the box itself must not scroll too, or the two move against
    // each other and the screen shakes.
    e.preventDefault();
    // A sideways swipe (back, another tab or Space) carries a little up-and-down jitter; read
    // as rows it would scroll the program up, down, up.
    if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) return;
    const s = spin.current;
    s.spare += e.deltaMode === 0 ? e.deltaY : e.deltaY * rowHeight;
    const lines = Math.trunc(s.spare / rowHeight);
    if (!lines) return;
    s.spare -= lines * rowHeight;
    s.lines += lines;
    s.frame ||= requestAnimationFrame(() => {
      if (s.lines) wheel(s.lines);
      s.lines = 0;
      s.frame = 0;
    });
  };
  // React listens to wheel and touchmove passively, where preventDefault does nothing; these
  // are added to the box directly so a full-screen program's scroll stays out of the page's.
  const rollRef = useRef(roll);
  rollRef.current = roll;
  useEffect(() => {
    const el = stage.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => rollRef.current(e);
    let start = { x: 0, y: 0 };
    const onStart = (e: TouchEvent) => (start = { x: e.touches[0].clientX, y: e.touches[0].clientY });
    const onMove = (e: TouchEvent) => {
      const dx = Math.abs(e.touches[0].clientX - start.x);
      const dy = Math.abs(e.touches[0].clientY - start.y);
      // An up-and-down drag is the program's; a sideways one still pans a pane wider than the phone.
      if (screenOf(pane)?.alt && dy > dx) e.preventDefault();
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    el.addEventListener("touchstart", onStart, { passive: true });
    el.addEventListener("touchmove", onMove, { passive: false });
    return () => {
      el.removeEventListener("wheel", onWheel);
      el.removeEventListener("touchstart", onStart);
      el.removeEventListener("touchmove", onMove);
    };
  }, [pane]);
  const wheel = (lines: number) => {
    const screen = screenOf(pane);
    if (screen) void request({ t: "wheel", pane, x: Math.floor(screen.cols / 2), y: Math.floor(screen.rows / 2), lines }).catch(() => {});
  };
  const typist = useRef<HTMLTextAreaElement>(null);
  const live = useKeyboard((s) => s.typing);
  useEffect(() => {
    useKeyboard.setState({
      toggle: () => (document.activeElement === typist.current ? typist.current?.blur() : typist.current?.focus({ preventScroll: true })),
    });
    return () => useKeyboard.setState({ typing: false, toggle: () => {} });
  }, []);
  const typed = (value: string) => {
    if (typist.current) typist.current.value = HELD;
    if (value.length < HELD.length) return void send({ t: "keys", pane, keys: [take("BSpace") ?? "BSpace"] });
    const text = value.replace(HELD, "");
    if (!text) return;
    const latched = Object.values(useMods.getState().held).some(Boolean);
    for (const part of text.split(/(\n)/)) {
      if (part === "\n") void send({ t: "keys", pane, keys: [take("Enter") ?? "Enter"] });
      else if (latched && /^[!-~]$/.test(part)) void send({ t: "keys", pane, keys: [take(part) ?? part] });
      else if (part) void send({ t: "text", pane, text: part, enter: false });
    }
    toEnd();
  };

  // Files dragged in from this computer, or pasted, are sent to the host as the attach button does.
  const { send: sendFiles } = useSendFiles(pane);
  const [dropping, setDropping] = useState(false);
  const carriesFiles = (e: React.DragEvent) => e.dataTransfer.types.includes("Files");
  const onPaste = (e: React.ClipboardEvent) => {
    const files = [...e.clipboardData.files];
    const text = e.clipboardData.getData("text/plain");
    if (!files.length && !text) return;
    e.preventDefault();
    if (files.length) void sendFiles(files);
    else void send({ t: "text", pane, text, enter: false });
    toEnd();
  };

  return (
    <div
      className={cn("relative flex min-h-0 flex-1 rounded-lg", live && "ring-2 ring-ring")}
      onDragOver={(e) => {
        if (!carriesFiles(e)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
        setDropping(true);
      }}
      onDragLeave={(e) => {
        // Moving over a child is not leaving.
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropping(false);
      }}
      onDrop={(e) => {
        if (!carriesFiles(e)) return;
        e.preventDefault();
        setDropping(false);
        void sendFiles([...e.dataTransfer.files]);
        stage.current?.focus({ preventScroll: true });
      }}
    >
      <textarea
        ref={typist}
        aria-label="Type into the pane"
        defaultValue={HELD}
        autoCapitalize="none"
        autoCorrect="off"
        autoComplete="off"
        spellCheck={false}
        enterKeyHint="enter"
        onPaste={onPaste}
        onFocus={() => useKeyboard.setState({ typing: true })}
        onBlur={() => useKeyboard.setState({ typing: false })}
        // While a word is being composed the field holds half of it; it is sent when it is whole.
        onChange={(e) => !(e.nativeEvent as InputEvent).isComposing && typed(e.currentTarget.value)}
        onCompositionEnd={(e) => typed(e.currentTarget.value)}
        // 16 px, or iOS zooms the page in on focus.
        className="pointer-events-none absolute bottom-0 left-0 h-px w-px resize-none opacity-0 text-[16px]"
      />
    <div
      ref={stage}
      tabIndex={0}
      onClick={(e) => {
        // A link a program printed opens in the browser and is not a click for the program.
        if ((e.target as HTMLElement).closest("a.term-link")) return;
        // A tap is a mouse click for a program that takes the mouse; the server knows whether it
        // does. It does not bring up the keyboard: that has its own button.
        const grid = host.current?.querySelector(".term-screen")?.getBoundingClientRect();
        if (!grid) return;
        const x = Math.floor((e.clientX - grid.left - 12) / (size * columnWidth()));
        const y = Math.floor((e.clientY - grid.top - 12) / (size * 1.25));
        if (x >= 0 && y >= 0 && e.clientY < grid.bottom) void send({ t: "click", pane, x, y });
      }}
      aria-label="Terminal. Type here to send keys."
      onKeyDown={onKeyDown}
      onPaste={onPaste}
      onScroll={(e) => {
        const el = e.currentTarget;
        const fromEnd = el.scrollHeight - el.scrollTop - el.clientHeight;
        pinned.current = fromEnd < 32;
        if (pinned.current) jumping.current = false;
        // On its way to the end after a press on "Latest", the button stays gone.
        setAway(fromEnd > 240 && !jumping.current);
        if (pinned.current) setDismissed(false);
      }}
      onTouchStart={(e) => {
        touching.current = true;
        drag.current = { y: e.touches[0].clientY, spare: 0 };
      }}
      // A full-screen program keeps its own scrollback, and there is none here to scroll. A
      // drag is passed to it as turns of the mouse wheel: a finger moving down shows what is above.
      onTouchMove={(e) => {
        if (!screenOf(pane)?.alt) return;
        e.stopPropagation();
        const y = e.touches[0].clientY;
        drag.current.spare += y - drag.current.y;
        drag.current.y = y;
        const lines = Math.trunc(drag.current.spare / rowHeight);
        if (lines) {
          drag.current.spare -= lines * rowHeight;
          wheel(-lines);
        }
      }}
      onTouchEnd={() => (touching.current = false)}
      onTouchCancel={() => (touching.current = false)}
      style={{ "--fs": `${size.toFixed(2)}px`, viewTransitionName: "stage" } as React.CSSProperties}
      data-fitted={fitted || undefined}
      data-asis={!fitted || undefined}
      className={cn(
        "slim-scroll min-h-0 min-w-0 flex-1 overflow-y-auto overscroll-none rounded-lg [overflow-anchor:none] bg-term text-term-fg outline-none dark:ring-1 dark:ring-border focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring",
        // A fitted window has no more columns than the box, so there is nothing to scroll sideways to.
        fitted ? "overflow-x-hidden" : "overflow-x-auto",
      )}
    >
      <div ref={content} className={cn("term-content", (settling || !answered) && "opacity-0")}>
      {history && history.length > 0 && (
        <div aria-label="History" className="term-history relative" style={{ height: rows.getTotalSize() }}>
          {rows.getVirtualItems().map((item) => (
            <div
              key={item.key}
              className="term-row absolute left-3"
              style={{ top: item.start }}
              ref={(el) => {
                if (el) paintRow(el, history[item.index]);
              }}
            />
          ))}
        </div>
      )}
      <div ref={host} />
      </div>
      {(!ready || settling || !answered) && (
        <p role="status" className="appear-late absolute inset-0 grid place-content-center justify-items-center gap-1 text-xs font-medium text-[#8a93b8]">
          <Pose art="loading" className="size-24" />
          Reading the screen
        </p>
      )}
    </div>
      {dropping && (
        <div aria-hidden className="drop-zone pointer-events-none absolute inset-0 grid place-content-center justify-items-center gap-2 rounded-lg border-2 border-dashed border-primary bg-term/90 text-center text-term-fg">
          <FileArrowUpIcon weight="duotone" size={44} className="text-primary" />
          <p className="text-base font-semibold">Drop to send to your Mac</p>
          <p className="text-sm opacity-70">Its path goes on the line, ready for the program</p>
        </div>
      )}
      {away && !dismissed && (
        <button
          type="button"
          aria-label="Jump to the latest output"
          onClick={jump}
          // Swiped down, it goes away.
          onPointerDown={(e) => (swipe.current = e.clientY)}
          onPointerMove={(e) => swipe.current !== null && e.clientY - swipe.current > 16 && (setDismissed(true), (swipe.current = null))}
          onPointerUp={() => (swipe.current = null)}
          className="absolute bottom-3 left-1/2 inline-flex h-ctl -translate-x-1/2 touch-none items-center gap-1.5 rounded-full bg-primary pr-4 pl-3 text-sm font-semibold text-primary-foreground shadow-[0_8px_24px_rgb(0_0_0/0.35)]"
        >
          <ArrowDownIcon weight="bold" size={16} />
          Latest
        </button>
      )}
    </div>
  );
}
