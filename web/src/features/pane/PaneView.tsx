// An open pane: its header, the windows of its session, the terminal, the keys.

import { ArrowsOutIcon, CaretLeftIcon, SidebarSimpleIcon, DotsThreeIcon, FolderIcon, type Icon, PlusIcon, StackIcon, TerminalWindowIcon, XIcon } from "@phosphor-icons/react";
import { useNavigate } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { useShallow } from "zustand/react/shallow";
import { BusyMark, WindowChip } from "@/components/bits";
import { ConfirmButton } from "@/components/Confirm";
import { Tip } from "@/components/Tip";
import { KeyDock, PaneSoftKeys } from "@/features/pane/KeyDock";
import { useKeyboard } from "@/features/pane/mods";
import { usePrefix } from "@/features/pane/prefix";
import { LinkState } from "@/connection/LinkState";
import { Terminal } from "@/features/pane/Terminal";
import { Tile } from "@/components/Tile";
import { Button } from "@/components/ui/button";
import { useAsk } from "@/connection/api";
import { useApp } from "@/lib/store";
import { programIcon } from "@/tmux/programs";
import { activePane, findPane, paneParam, previewPane, shortPath } from "@/tmux/tree";
import { cn } from "@/lib/utils";

/** One fact about the pane: a glyph that says which, then the value. */
function Meta({ icon: Glyph, label, text, className }: { icon: Icon; label: string; text: string; className?: string }) {
  return (
    <span title={label} className={cn("flex min-w-0 items-center gap-1", className)}>
      <Glyph weight="fill" size={15} className="shrink-0" aria-label={label} />
      <span className="truncate">{text}</span>
    </span>
  );
}

/** The round tile a notice carries at its left. */
function ToastIcon({ icon: Glyph }: { icon: Icon }) {
  return (
    <span className="grid size-9 place-items-center rounded-full bg-soft-strong text-foreground">
      <Glyph weight="bold" size={18} />
    </span>
  );
}

export function PaneView({ pane }: { pane: string }) {
  const navigate = useNavigate();
  const where = useApp(useShallow((s) => findPane(s.sessions, pane)));
  const hasTree = useApp((s) => s.hasTree);
  const working = useApp((s) => s.busy);
  const busy = working.includes(pane);
  const openSheet = useApp((s) => s.openSheet);
  const soft = useKeyboard((s) => s.soft);
  const ask = useAsk();
  const sidebar = useApp((s) => s.sidebar);
  const toggleSidebar = useApp((s) => s.toggleSidebar);
  const prefixKeys = useApp((s) => s.prefixKeys);
  const armed = usePrefix((s) => s.armed);
  // The pane last seen in the tree, and whose it was.
  const listed = useRef<{ pane: string; session: string; name: string } | null>(null);

  // Only a pane listed before and missing now has closed; the phone then stays in the session
  // on tmux's next window, and returns to the list only when the session has ended.
  useEffect(() => {
    if (where) {
      listed.current = { pane, session: where.session.id, name: where.session.name };
      return;
    }
    const was = listed.current;
    if (!hasTree || was?.pane !== pane) return;
    listed.current = null;
    const { sessions, last } = useApp.getState();
    const session = sessions.find((s) => s.id === was.session);
    const next = session && previewPane(session, last);
    const window = next && session.windows.find((w) => w.panes.includes(next));
    if (next && window) {
      toast("Closed", { description: `Now in ${window.index} ${window.name}`, icon: <ToastIcon icon={XIcon} /> });
      void navigate({ to: "/pane/$pane", params: { pane: paneParam(next.id) }, replace: true, viewTransition: true });
    } else {
      toast(`${was.name} ended`, { description: "Its last window was closed", icon: <ToastIcon icon={StackIcon} /> });
      void navigate({ to: "/", replace: true, viewTransition: true });
    }
  }, [where, hasTree, pane, navigate]);

  // Opening a pane makes it the one its session's row opens next time.
  const remember = useApp((s) => s.remember);
  const session = where?.session.id;
  useEffect(() => {
    if (session) remember(session, pane);
  }, [session, pane, remember]);

  // The pane a press asked for, until it shows: the chip answers at once. No view transition,
  // which would hold the page still and read as a press not heard.
  const [asking, setAsking] = useState<string | null>(null);
  // Once the pane shown is the one asked for, the asking is over.
  const asked = asking === pane ? null : asking;
  const go = (id: string) => {
    setAsking(id);
    return navigate({ to: "/pane/$pane", params: { pane: paneParam(id) } });
  };
  const shown = asked ?? pane;

  return (
    <div className="flex h-full min-w-0 flex-1 flex-col gap-3 px-3 pt-[max(12px,var(--safe-top))] pb-[max(12px,env(safe-area-inset-bottom))] lg:px-5 land:gap-2 land:pt-2 land:pb-2">
      {/* On a phone on its side the name and the window chips share one row. */}
      <div className="flex shrink-0 flex-col gap-3 land:flex-row land:items-center land:gap-2">
      <header className="flex shrink-0 items-center gap-2 land:max-w-[45%]">
        <Button variant="soft" size="icon" aria-label="Back to sessions"
          // The corner of a phone is hard to hit: the button answers a little beyond its own edge.
          className="relative before:absolute before:-top-3 before:right-0 before:-bottom-2 before:-left-3 lg:hidden" onClick={() => navigate({ to: "/", viewTransition: true })}>
          <CaretLeftIcon weight="bold" size={20} />
        </Button>
        <Tip label={sidebar ? "Hide sessions" : "Show sessions"} hint={prefixKeys ? "Ctrl+B, then s" : undefined} side="bottom">
          <Button variant="soft" size="icon" aria-label={sidebar ? "Hide the session list" : "Show the session list"} aria-pressed={!sidebar} className="hidden lg:inline-flex" onClick={toggleSidebar}>
            <SidebarSimpleIcon weight={sidebar ? "bold" : "fill"} size={20} />
          </Button>
        </Tip>
        {where && <Tile icon={programIcon(where.pane.command)} tone="accent" seed={where.session.name} className="land:hidden" />}
        <div className="min-w-0 flex-1 px-1">
          {/* The page's one h1 is the session list's; this names a part of it. */}
          <h2 className="truncate text-lg leading-tight font-semibold">{where?.window.name ?? "Opening…"}</h2>
          {where && (
            <p className="mt-0.5 flex items-center gap-3 text-sm text-muted-foreground land:hidden">
              <Meta icon={StackIcon} label="Session" text={where.session.name} />
              <Meta icon={TerminalWindowIcon} label="Program" text={where.pane.command} />
              <Meta icon={FolderIcon} label="Folder" text={shortPath(where.pane.path)} className="hidden sm:flex" />
              <Meta icon={ArrowsOutIcon} label="Size" text={`${where.pane.cols}×${where.pane.rows}`} className="hidden md:flex" />
            </p>
          )}
        </div>
        {armed && (
          // tmux shows the waiting prefix in its status line; this is that.
          <kbd role="status" aria-label="Waiting for a tmux key" className="rounded-sm bg-primary px-2 py-1 font-mono text-xs font-bold text-primary-foreground">
            {armed === "kill" ? "close? y" : "C-b"}
          </kbd>
        )}
        {busy && <BusyMark />}
        <LinkState />
        {where && (
          <Tip label="Window and pane" hint="Rename, history, text size, close" side="bottom">
            <Button variant="soft" size="icon" aria-label="Window and pane actions" onClick={() => openSheet({ kind: "pane", pane })}>
              <DotsThreeIcon weight="bold" size={22} />
            </Button>
          </Tip>
        )}
      </header>
      {where && (
        <div className="no-scrollbar -m-1.5 flex shrink-0 gap-2 overflow-x-auto p-1.5 land:min-w-0 land:flex-1">
          {where.session.windows.map((w) => (
            <WindowChip
              key={w.id}
              window={w}
              current={w.panes.some((p) => p.id === shown)}
              loading={asked !== null && w.panes.some((p) => p.id === asked)}
              busy={w.panes.some((p) => working.includes(p.id))}
              onClick={() => !w.panes.some((p) => p.id === shown) && void go(activePane(w).id)}
              onMenu={() => openSheet({ kind: "pane", pane: activePane(w).id })}
            />
          ))}
          {where.window.panes.length > 1 &&
            where.window.panes.map((p) => (
              <WindowChip key={p.id} window={{ ...where.window, index: p.index, name: `pane · ${p.command}` }} current={p.id === pane} onClick={() => p.id !== pane && void go(p.id)} />
            ))}
          {/* After the last window: one more. It asks first, like everything that changes tmux. */}
          <Tip label="New window" hint="Press twice to add it" side="bottom">
            <ConfirmButton
              variant="soft"
              size="sm"
              aria-label="New window"
              confirmLabel="Tap again"
              className="size-ctl-sm shrink-0 !px-0 pointer-coarse:size-ctl"
              onConfirm={() => ask.mutate({ t: "new-window", session: where.session.id }, { onSuccess: (made) => made.pane && void go(made.pane) })}
            >
              <PlusIcon weight="bold" size={18} />
            </ConfirmButton>
          </Tip>
        </div>
      )}
      </div>
      <Terminal key={pane} pane={pane} />
      <KeyDock pane={pane} />
      {soft && <PaneSoftKeys pane={pane} />}
    </div>
  );
}
