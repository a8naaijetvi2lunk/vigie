// Fixtures accessibles uniquement via ?demo, sans réseau ni donnée personnelle.
import { DEFAULT_CONFIG, mergeConfig, type ConfigPatch } from "./config";
import type { Session } from "./sessions";
import type { Snapshot } from "./usage";
import { isoDayLocal } from "./viz";
const params = new URLSearchParams(location.search);
let config = { ...DEFAULT_CONFIG, hud: params.has("hud"), theme: params.has("dark") ? "dark" : "light", animations: !params.has("still") };
const listeners = new Map<string, Set<(data: unknown) => void>>();
const now = () => Math.floor(Date.now() / 1000);
const sessions = (): Session[] => [
  { id: "demo-1", providerId: "claude", project: "vigie", model: "claude-opus-5", startedAt: now()-1800, lastActiveAt: now()-5, active: true },
  { id: "demo-2", providerId: "codex", project: "api-facturation", model: "gpt-5.6-sol", startedAt: now()-7200, lastActiveAt: now()-10, active: true },
  { id: "demo-3", providerId: "codex", project: "Un projet au nom volontairement très long pour vérifier les débordements", model: "gpt-5.6-sol", startedAt: now()-10800, lastActiveAt: now()-4000, active: false },
];
const snapshot = (): Snapshot => ({ fetchedAt: now(), providers: [
  { id: "claude", prefix: "$ claude", active: true, dataTs: now(), model: "claude-opus-5", windows: [{ kind: "5h", usedPercent: 62, resetsAt: now()+8040 }, { kind:"weekly", usedPercent: 41, resetsAt:now()+200000 }] },
  { id: "codex", prefix: "$ codex", active: true, dataTs: now(), model: "gpt-5.6-sol", windows: [{ kind:"weekly", usedPercent:88, resetsAt:now()+400000 }] },
] });
export function demoListen<T>(event: string, receive: (data: T) => void) {
  const callback = (data: unknown) => receive(data as T);
  const set = listeners.get(event) ?? new Set(); set.add(callback); listeners.set(event,set);
  return () => { set.delete(callback); };
}
export async function demoCall<T>(command: string, args?: Record<string,unknown>): Promise<T> {
  let result: unknown;
  switch(command) {
    case "get_config": result = config; break;
    case "patch_config": config = mergeConfig(config, args?.patch as ConfigPatch); listeners.get("config-updated")?.forEach(fn => fn(config)); result = config; break;
    case "get_sessions": result = new URLSearchParams(location.search).has("empty") ? [] : sessions(); break;
    case "get_snapshot": result = new URLSearchParams(location.search).has("empty") ? { providers: [], fetchedAt: 0 } : snapshot(); break;
    case "get_history": result = Array.from({length:24},(_,i)=>({ts:now()-(24-i)*3600,pct:12+i*2+Math.sin(i)*5})); break;
    case "get_heatmap": result = [18,34,52,47,12,0,6,41,63,58,72,29,8,0,55,68,81,44,37,11,4,49,62,77,90,58,15,9,46,62].map((pct,i)=>({day:isoDayLocal(new Date(Date.now()-(29-i)*86400000)),pct})); break;
    case "open_sessions": location.hash = "sessions"; location.reload(); break;
    case "set_view": document.documentElement.dataset.view = String(args?.view); break;
  }
  return result as T;
}
