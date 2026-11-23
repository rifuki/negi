// Files from this device, kept on the host, with their paths typed into the pane: a space
// after each and no Enter, since a path is something to say more about.

import { useState } from "react";
import { toast } from "sonner";
import { request } from "@/connection/link";
import { upload } from "@/connection/api";

/** A path as a shell reads it: as it is when it holds nothing a shell would split or expand. */
export function shellPath(path: string) {
  return /^[\w./~-]+$/.test(path) ? path : `'${path.replace(/'/g, `'\\''`)}'`;
}

export function useSendFiles(pane: string) {
  const [sending, setSending] = useState(false);
  const send = async (files: File[]) => {
    if (!files.length) return;
    setSending(true);
    const id = toast.loading(files.length === 1 ? `Sending ${files[0].name}` : `Sending ${files.length} files`);
    try {
      const paths = [];
      for (const file of files) paths.push(shellPath(await upload(file)));
      await request({ t: "text", pane, text: `${paths.join(" ")} `, enter: false });
      toast.success(files.length === 1 ? "Sent" : `${files.length} files sent`, { id, description: "The path is on the line" });
    } catch (e) {
      toast.error((e as Error).message, { id });
    } finally {
      setSending(false);
    }
  };
  return { send, sending };
}
