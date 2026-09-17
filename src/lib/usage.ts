// Types miroir du Snapshot Rust (camelCase via serde `rename_all`).

export type QuotaKind = "5h" | "weekly" | "opus" | "other";

export interface QuotaWindow {
  kind: QuotaKind | string;
  usedPercent: number;
  resetsAt: number; // epoch secondes
}

/** Notes d'état d'un provider (dégradation propre, pas de crash). */
export type ProviderNote = "token_expired" | "rate_limited" | string;

export interface ProviderSnapshot {
  id: string;
  prefix: string;
  windows: QuotaWindow[];
  active: boolean;
  dataTs: number; // epoch secondes
  note?: ProviderNote;
  /** Modèle utilisé, nom brut ("claude-opus-5", "gpt-5.6-sol"). Absent si inconnu. */
  model?: string;
}

export interface Snapshot {
  providers: ProviderSnapshot[];
  fetchedAt: number; // epoch secondes
}

export type UsageState = "ok" | "warn" | "crit";

/** Seuils : ok < 70, warn 70-90 inclus, crit > 90. */
export function stateOf(percent: number): UsageState {
  if (percent > 90) return "crit";
  if (percent >= 70) return "warn";
  return "ok";
}

const WEEKDAYS_FR = ["dim", "lun", "mar", "mer", "jeu", "ven", "sam"];

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/**
 * Formatte le libellé de reset d'une fenêtre de quota.
 * - "5h" (et autres fenêtres courtes) : temps restant, ex. "reset 2h14".
 * - "weekly" et "opus" (reset hebdomadaire) : jour court FR + heure locale du reset,
 *   ex. "reset lun 09:00".
 */
export function formatCountdown(kind: string, resetsAt: number): string {
  const resetDate = new Date(resetsAt * 1000);

  if (kind === "weekly" || kind === "opus") {
    const day = WEEKDAYS_FR[resetDate.getDay()];
    return `reset ${day} ${pad2(resetDate.getHours())}:${pad2(resetDate.getMinutes())}`;
  }

  const nowSec = Date.now() / 1000;
  const remaining = Math.max(0, Math.round(resetsAt - nowSec));
  const hours = Math.floor(remaining / 3600);
  const minutes = Math.floor((remaining % 3600) / 60);
  return `reset ${hours}h${pad2(minutes)}`;
}

/**
 * Reset au format HUD : identique à `formatCountdown` sans le préfixe « reset ».
 * Dans une ligne unique façon compteur FPS, le mot est redondant et coûte la place
 * qui manque au nom du modèle. Ex. "2h14" · "lun 09:00".
 */
export function formatHudReset(kind: string, resetsAt: number): string {
  return formatCountdown(kind, resetsAt).replace(/^reset /, "");
}

/**
 * Heure EXACTE de reset pour le mode étendu (locale).
 * - Reset aujourd'hui : "reset 16:30", + temps restant pour la fenêtre 5h → "reset 16:30 (2h14)".
 * - Reset un autre jour : "reset 09:00 · lun 21/07".
 */
export function formatResetExact(kind: string, resetsAt: number): string {
  const resetDate = new Date(resetsAt * 1000);
  const hm = `${pad2(resetDate.getHours())}:${pad2(resetDate.getMinutes())}`;
  const now = new Date();
  const sameDay = resetDate.toDateString() === now.toDateString();

  if (!sameDay) {
    const day = WEEKDAYS_FR[resetDate.getDay()];
    return `reset ${hm} · ${day} ${pad2(resetDate.getDate())}/${pad2(resetDate.getMonth() + 1)}`;
  }
  if (kind === "5h") {
    const remaining = Math.max(0, Math.round(resetsAt - now.getTime() / 1000));
    const hours = Math.floor(remaining / 3600);
    const minutes = Math.floor((remaining % 3600) / 60);
    return `reset ${hm} (${hours}h${pad2(minutes)})`;
  }
  return `reset ${hm}`;
}

const STALE_THRESHOLD_SECONDS = 600;

/**
 * Mesure périmée ? Même règle que `ProviderSnapshot::is_stale` côté Rust (tray).
 * - Claude est interrogé toutes les 5 min : au-delà de 10 min, une mesure a manqué.
 * - Codex ne change que quand il tourne : sa mesure reste exacte jusqu'au reset d'une
 *   de ses fenêtres (la règle des 10 min le ferait passer pour déconnecté).
 */
export function isStale(
  provider: Pick<ProviderSnapshot, "id" | "dataTs" | "windows">,
  nowSec: number = Date.now() / 1000,
): boolean {
  if (provider.id === "codex") return provider.windows.some((w) => w.resetsAt < nowSec);
  return nowSec - provider.dataTs > STALE_THRESHOLD_SECONDS;
}

/** Libellé d'ancienneté, ex. "# il y a 12 min", "# il y a 3 h", "# il y a 2 j". */
export function staleLabel(dataTs: number, nowSec: number = Date.now() / 1000): string {
  const minutes = Math.max(0, Math.floor((nowSec - dataTs) / 60));
  if (minutes < 60) return `# il y a ${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 48) return `# il y a ${hours} h`;
  return `# il y a ${Math.floor(hours / 24)} j`;
}

/**
 * Abrège un nom de modèle brut pour l'affichage compact du HUD :
 * - retire le préfixe fournisseur "claude-" ;
 * - retire un suffixe de date "-AAAAMMJJ" ;
 * - recolle une version scindée par des tirets ("haiku-4-5" → "haiku-4.5").
 * Exemples : "claude-opus-5" → "opus-5" · "claude-haiku-4-5-20251001" → "haiku-4.5"
 * · "gpt-5.6-sol" → "gpt-5.6-sol" (inchangé).
 */
export function shortModelName(model: string): string {
  let name = model.replace(/^claude-/, "").replace(/-\d{8}$/, "");
  name = name.replace(/-(\d+)-(\d+)$/, "-$1.$2");
  return name;
}

/** Priorité d'affichage : la première fenêtre présente devient la primaire. */
const WINDOW_PRIORITY = ["5h", "weekly", "opus"];

/** Fenêtres présentes, ordonnées 5h > weekly > opus. Une fenêtre absente n'existe pas à l'écran. */
export function orderedWindows(windows: QuotaWindow[]): QuotaWindow[] {
  const priority = (kind: string) => { const i = WINDOW_PRIORITY.indexOf(kind); return i < 0 ? 99 : i; };
  return [...windows].sort((a,b) => priority(a.kind) - priority(b.kind));
}

export function providerStatus(provider: ProviderSnapshot): string | null {
  if (provider.note === "token_expired") return "Connexion expirée";
  if (provider.note === "rate_limited") return "Actualisation en pause";
  if (provider.note === "unavailable") return "Source indisponible";
  if (isStale(provider)) return "Données anciennes";
  return null;
}

/** Durée d'affichage d'un provider avant rotation, quand plusieurs sont actifs. */
export const HUD_ROTATION_MS = 5 * 60 * 1000;

/**
 * Provider affiché par le HUD, parmi ceux qui ont au moins une fenêtre de quota.
 *
 * La priorité suit l'activité, pas la charge : sinon une fenêtre hebdo durablement
 * haute figerait le HUD sur le même provider.
 *
 * 1. Un seul agent actif → c'est lui qui intéresse, quel que soit son pourcentage.
 * 2. Plusieurs agents actifs → rotation toutes les 5 minutes, calée sur l'horloge (donc
 *    stable entre deux rendus : deux appels dans la même fenêtre donnent le même provider).
 * 3. Aucun agent actif → le provider dont la donnée est la plus fraîche (`dataTs`).
 *
 * L'ordre du tableau (Claude puis Codex, garanti côté Rust par `provider_order`) tranche
 * les égalités et fixe l'ordre de rotation.
 */
export function pickHudProvider(
  providers: ProviderSnapshot[],
  nowMs: number = Date.now(),
): ProviderSnapshot | undefined {
  const candidats = providers.filter((p) => orderedWindows(p.windows).length > 0);
  if (candidats.length <= 1) return candidats[0];

  const actifs = candidats.filter((p) => p.active);
  if (actifs.length === 1) return actifs[0];
  if (actifs.length > 1) {
    const index = Math.floor(nowMs / HUD_ROTATION_MS) % actifs.length;
    return actifs[index];
  }

  return candidats.reduce((a, b) => (b.dataTs > a.dataTs ? b : a));
}
