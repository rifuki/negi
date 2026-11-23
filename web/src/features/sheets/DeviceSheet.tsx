import { CircleHalfIcon, CopyIcon, DeviceMobileIcon, KeyIcon, MoonIcon, ProhibitIcon, SignOutIcon, SunIcon } from "@phosphor-icons/react";
import { type ReactNode } from "react";
import { toast } from "sonner";
import { Switch } from "@/components/Switch";
import { Tip } from "@/components/Tip";
import { CHARACTERS, characterOf } from "@/features/sessions/characters";
import { Tile } from "@/components/Tile";
import { Button } from "@/components/ui/button";
import { signOut } from "@/connection/link";
import { useApp } from "@/lib/store";
import { switchAccent, switchTheme } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { DangerRow, Frame, Group, Row } from "./parts";
import { close } from "./shared";

const THEMES = [
  ["system", "System", CircleHalfIcon],
  ["light", "Light", SunIcon],
  ["dark", "Dark", MoonIcon],
] as const;

const COLOURS = [
  ["miku", "Miku", "#39c5bb"],
  ["aqua", "Aqua", "#36a3d9"],
  ["sky", "Sky", "#2b65c9"],
  ["sakura", "Sakura", "#f2a3b8"],
] as const;

const RING = "shadow-[0_0_0_2px_var(--card),0_0_0_4px_var(--foreground)]";

/** Where a click was, so the theme and colour spread out from it. */
const from = (e: React.MouseEvent<HTMLElement>) => {
  const box = e.currentTarget.getBoundingClientRect();
  return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
};

export function DeviceSheet() {
  const device = useApp((s) => s.device);
  const version = useApp((s) => s.version);
  const os = useApp((s) => s.os);
  const character = useApp((s) => s.character);
  const motion = useApp((s) => s.motion);
  const setMotion = useApp((s) => s.setMotion);
  const prefixKeys = useApp((s) => s.prefixKeys);
  const setPrefixKeys = useApp((s) => s.setPrefixKeys);
  const setCharacter = useApp((s) => s.setCharacter);
  const theme = useApp((s) => s.theme);
  const accent = useApp((s) => s.accent);

  const danger = (
    <Group>
      <DangerRow icon={SignOutIcon} label="Forget this device here" sure="Tap again to forget" onConfirm={() => (close(), signOut())} />
      {/* Forgetting only empties this browser. Shutting the device out is done on the Mac, with this. */}
      <Row
        icon={ProhibitIcon}
        tone="quiet"
        label="Block it on your Mac"
        sub={<code className="mt-1 inline-block rounded-xs bg-soft-strong px-1.5 py-0.5 font-mono text-xs font-bold text-foreground [word-spacing:-0.25em]">negi revoke</code>}
        end={
          <Tip label="Copy the command" side="left">
            <Button variant="ghost" size="icon" aria-label="Copy the command" onClick={() => void navigator.clipboard.writeText("negi revoke").then(() => toast("Copied. Run it in a terminal on your Mac."))}>
              <CopyIcon weight="bold" size={18} />
            </Button>
          </Tip>
        }
      />
    </Group>
  );

  return (
    <Frame title="This device" lead={<Tile icon={DeviceMobileIcon} tone="accent" seed="device" size="sm" />}>
      <div className="grid gap-3 md:grid-cols-2 md:items-start">
        <div className="grid gap-3">
          <Group>
            <Row icon={KeyIcon} tone="quiet" label={device || "This device"} sub={`Paired here · negi ${version} on your ${os === "macos" ? "Mac" : "computer"}`} />
          </Group>

          <Card title="Appearance">
            <Line label="Theme">
              <div role="group" aria-label="Theme" className="flex gap-0.5 rounded-full bg-soft p-0.5">
                {THEMES.map(([value, label, Glyph]) => (
                  <Tip key={value} label={label}>
                    <button
                      type="button"
                      aria-label={label}
                      aria-pressed={theme === value}
                      onClick={(e) => switchTheme(value, from(e))}
                      className="grid size-11 place-items-center rounded-full text-muted-foreground transition-colors hover:text-foreground aria-pressed:bg-background aria-pressed:text-foreground aria-pressed:shadow-sm"
                    >
                      <Glyph weight="fill" size={18} />
                    </button>
                  </Tip>
                ))}
              </div>
            </Line>
            <Line label="Colour">
              <div role="group" aria-label="Colour" className="flex">
                {COLOURS.map(([value, label, colour]) => (
                  <Tip key={value} label={label}>
                    <button type="button" aria-label={label} aria-pressed={accent === value} onClick={(e) => switchAccent(value, from(e))} className="group grid size-11 place-items-center rounded-full">
                      <span className={cn("size-7 rounded-full transition-[box-shadow,transform] duration-500 ease-spring group-active:scale-90", accent === value && RING)} style={{ backgroundColor: colour }} />
                    </button>
                  </Tip>
                ))}
              </div>
            </Line>
            <Line label="Animations" sub={motion ? "She blinks and waves" : "Everything holds still"}>
              <Switch label="Animations" checked={motion} onChange={setMotion} className="mr-1" />
            </Line>
            <Line label="tmux keys" sub={prefixKeys ? "Ctrl+B, then a key, as in tmux. ? lists them" : "Ctrl+B goes to the program"}>
              <Switch label="tmux keys" checked={prefixKeys} onChange={setPrefixKeys} className="mr-1" />
            </Line>
          </Card>

        </div>

        <div className="grid gap-3">
          <Card title="Character">
            <div role="group" aria-label="Character" className="flex gap-3 px-2 pt-1 pb-2">
              {CHARACTERS.map(({ id, label, sheet }) => {
                const chosen = characterOf(character).id === id;
                return (
                  <Pick key={id} label={label} chosen={chosen} name={`${label} character`} onPick={() => setCharacter(id)}>
                    {/* Her first face, on the card's own colour, as she will be seen. */}
                    <span className={cn("block size-14 rounded-md bg-hero bg-no-repeat", chosen && RING)} style={{ backgroundImage: `url(${sheet})`, backgroundSize: "400% 200%", backgroundPosition: "0% 0%" }} />
                  </Pick>
                );
              })}
            </div>
          </Card>


          {danger}
        </div>
      </div>
    </Frame>
  );
}

/** A group of settings with a small heading, and an optional note under it. */
function Card({ title, note, children }: { title: string; note?: string; children: ReactNode }) {
  return (
    <section className="grid gap-0.5 rounded-md bg-card p-1">
      <h3 className="px-2 pt-2 text-xs font-bold text-muted-foreground">{title}</h3>
      {children}
      {note && <p className="px-2 pb-2 text-xs text-muted-foreground">{note}</p>}
    </section>
  );
}

/** One setting: its name on the left, its control on the right. */
function Line({ label, sub, children }: { label: string; sub?: string; children: ReactNode }) {
  return (
    <div className="flex min-h-ctl-lg items-center gap-3 rounded-sm px-2 py-1.5">
      <span className="min-w-0 flex-1 font-semibold">
        {label}
        {sub && <small className="block text-xs font-medium text-muted-foreground">{sub}</small>}
      </span>
      {children}
    </div>
  );
}

function Pick({ label, name, chosen, onPick, children }: { label: string; name: string; chosen: boolean; onPick: () => void; children: ReactNode }) {
  return (
    <button type="button" aria-label={name} aria-pressed={chosen} onClick={onPick} className="grid justify-items-center gap-1.5 text-xs font-semibold text-muted-foreground transition-transform duration-500 ease-spring active:scale-95 aria-pressed:text-foreground">
      {children}
      {label}
    </button>
  );
}
