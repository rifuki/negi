// The pages the router shows. Route definitions live in router.tsx.

import { ArrowClockwiseIcon, PlusIcon } from "@phosphor-icons/react";
import { Outlet, useNavigate, useParams } from "@tanstack/react-router";
import { useEffect, useMemo, useRef } from "react";
import { Link } from "@/connection/Socket";
import { List } from "@/features/sessions/List";
import { PaneView } from "@/features/pane/PaneView";
import { Sheets } from "@/features/sheets";
import { State } from "@/components/State";
import { Button } from "@/components/ui/button";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { watch } from "@/connection/link";
import { useApp } from "@/lib/store";
import { paneId, previewPane } from "@/tmux/tree";
import { cn } from "@/lib/utils";

export function Root() {
  const navigate = useNavigate();
  const token = useApp((s) => s.token);
  // Pairing and forgetting change where the app belongs; the routes' own guards only run
  // on navigation. Not on load: the guards have placed the page already, a reload included.
  const had = useRef(Boolean(token));
  useEffect(() => {
    if (had.current === Boolean(token)) return;
    had.current = Boolean(token);
    void navigate({ to: token ? "/" : "/pair", replace: true });
    // Only when the token appears or disappears.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [Boolean(token)]);
  return (
    <TooltipProvider delayDuration={350} skipDelayDuration={200}>
      <Link />
      <Outlet />
      <Sheets />
      <Toaster />
    </TooltipProvider>
  );
}

/** Whatever stands between the app and a session list: no answer yet, or no tmux. */
function useBlocker() {
  const hasTree = useApp((s) => s.hasTree);
  const problem = useApp((s) => s.problem);
  const server = useApp((s) => s.server);
  const openSheet = useApp((s) => s.openSheet);
  if (!hasTree) {
    return problem ? (
      <State art="lost" title="Can't reach your Mac" text={problem}>
        <Button className="w-full" onClick={() => location.reload()}>
          <ArrowClockwiseIcon weight="bold" size={20} />
          Try again
        </Button>
      </State>
    ) : (
      <State art="laptop" title="Connecting to your Mac" text="Asking tmux for your sessions…" />
    );
  }
  if (!server) {
    return (
      <State art="sleep" title="tmux is not running" text="There is no tmux server on your Mac right now. Start a session and it will appear here.">
        <Button className="w-full" onClick={() => openSheet({ kind: "new-session" })}>
          <PlusIcon weight="bold" size={20} />
          Start a session
        </Button>
      </State>
    );
  }
  return null;
}

/** The app at `/` and `/pane/3`: the list or a pane on a phone, both side by side on a wide screen. */
export function AppPage() {
  const params = useParams({ strict: false }) as { pane?: string };
  const open = params.pane ? paneId(params.pane) : null;
  const sessions = useApp((s) => s.sessions);
  const blocker = useBlocker();
  const sidebar = useApp((s) => s.sidebar);
  // Hidden only beside an open pane; with nothing open the list is the page.
  const tucked = Boolean(open) && !sidebar;

  // One pane per session is watched as a preview, and the open pane as open.
  const last = useApp((s) => s.last);
  const previews = useMemo(() => sessions.map((s) => previewPane(s, last)?.id).filter(Boolean).join(","), [sessions, last]);
  useEffect(() => {
    const panes = new Map<string, boolean>(previews.split(",").filter(Boolean).map((id) => [id, false]));
    if (open) panes.set(open, true);
    watch(panes);
  }, [previews, open]);

  if (blocker) return <main className="h-full">{blocker}</main>;
  return (
    <main className="flex h-full">
      <div
        aria-hidden={tucked || undefined}
        inert={tucked || undefined}
        className={cn(
          "sidebar h-full w-full shrink-0 lg:block lg:w-[400px] lg:overflow-hidden lg:border-r",
          open && "hidden",
          tucked && "lg:w-0 lg:border-r-0",
        )}
      >
        {/* The list keeps its width while the column narrows, so it slides away whole. */}
        <div className={cn("h-full lg:w-[400px]", tucked && "lg:-translate-x-8 lg:opacity-0")}>
          <List open={open} />
        </div>
      </div>
      {open ? (
        <PaneView pane={open} />
      ) : (
        <div className="hidden flex-1 place-items-center lg:grid">
          <p className="text-lg text-muted-foreground">Pick a session to open it here.</p>
        </div>
      )}
    </main>
  );
}
