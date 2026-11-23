// Text a program in a pane copied (a yank in nvim, a copy in tmux) lands on the clipboard
// of the device that was typing, not on every device that happens to be open.

import { toast } from "sonner";

/** How long after its last key a device still counts as the one that copied. */
const TYPED_WITHIN = 5000;

let typedAt = 0;

/** Marks this device as the one in use; called for every key and text sent to a pane. */
export function typed() {
  typedAt = Date.now();
}

function preview(text: string) {
  const lines = text.trim().split("\n");
  const line = lines[0] ?? "";
  const more = lines.length === 1 ? "" : lines.length === 2 ? " and 1 more line" : ` and ${lines.length - 1} more lines`;
  return (line.length > 60 ? `${line.slice(0, 60)}…` : line) + more;
}

export async function copied(text: string) {
  if (!text || !document.hasFocus() || Date.now() - typedAt > TYPED_WITHIN) return;
  try {
    await navigator.clipboard.writeText(text);
    toast("Copied", { description: preview(text) });
  } catch {
    // Safari writes to the clipboard only inside a tap; the button is that tap.
    toast("Copied on your Mac", {
      description: preview(text),
      duration: 8000,
      action: { label: "Copy here", onClick: () => void navigator.clipboard.writeText(text).then(() => toast("Copied")) },
    });
  }
}
