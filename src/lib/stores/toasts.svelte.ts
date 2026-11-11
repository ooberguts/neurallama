export interface Toast {
  id: number;
  kind: "error" | "info" | "ok";
  text: string;
}

let next = 1;

class Toasts {
  list = $state<Toast[]>([]);

  push(kind: Toast["kind"], text: string, ms = kind === "error" ? 9000 : 3200) {
    const id = next++;
    this.list = [...this.list.slice(-3), { id, kind, text }];
    if (ms > 0) setTimeout(() => this.close(id), ms);
  }
  error(e: unknown) {
    this.push("error", e instanceof Error ? e.message : String(e));
  }
  info(text: string) {
    this.push("info", text);
  }
  ok(text: string) {
    this.push("ok", text);
  }
  close(id: number) {
    this.list = this.list.filter((t) => t.id !== id);
  }
}

export const toasts = new Toasts();

/** Copy text to the clipboard with a short confirmation toast. */
export async function copy(text: string, what = "Copied") {
  try {
    await navigator.clipboard.writeText(text);
    toasts.ok(what);
  } catch {
    // Fallback for contexts without clipboard permission.
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    if (ok) toasts.ok(what);
    else toasts.push("error", "Clipboard is not available here.");
  }
}
