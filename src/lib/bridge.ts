import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** Les previews sont explicitement activées et ne font aucun appel natif. */
export const isPreview = !isTauri() && new URLSearchParams(location.search).has("demo");

export async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (isPreview) return (await import("./demo")).demoCall<T>(command, args);
  return invoke<T>(command, args);
}

export async function subscribe<T>(event: string, receive: (data: T) => void): Promise<() => void> {
  if (isPreview) return (await import("./demo")).demoListen(event, receive);
  return listen<T>(event, e => receive(e.payload));
}
